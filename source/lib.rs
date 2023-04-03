// https://www.rfc-editor.org/rfc/rfc3501 - IMAP4rev1
// https://www.rfc-editor.org/rfc/rfc4315 - [...] UIDPLUS extension
// https://www.rfc-editor.org/rfc/rfc4549 - Synchronization Operations for Disconnected IMAP4 Clients
// https://www.rfc-editor.org/rfc/rfc7162 - [...] Quick Mailbox Resynchronization (QRESYNC)

#![allow(clippy::upper_case_acronyms)]

use anyhow::Context as _;
use std::{
  collections, error, fmt, io,
  net::{self, ToSocketAddrs as _},
  num, path, process, result, str, thread, time,
};
use zeroize::Zeroize as _;

mod imap;
pub mod maildir;
mod notmuch;
mod sync;
use sync::Open as _;

#[derive(Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum Mode {
  ConnectOnly,
  Pull,
  Push,
  // A full sync mode (pull+push) would need to invoke notmuch new --no-hooks because the pull
  // relies on notmuch new's detection of new messages.
}

fn parse_duration(argument: &str) -> Result<time::Duration, num::ParseIntError> {
  Ok(time::Duration::from_secs(argument.parse()?))
}

#[derive(clap::Args)]
#[group(skip)]
pub struct Arguments {
  #[arg(help = "Execution mode: pull | push", hide_possible_values(true))]
  pub mode: Mode,

  #[arg(long = "address", help = "Server address")]
  pub address: String,
  #[arg(long = "port", help = "Server port")]
  pub port: u16,
  #[arg(long = "tls", help = "Enable TLS", default_value_t = true)]
  pub tls: bool,
  #[arg(long = "timeout", help = "TCP timeout (in seconds)", value_parser = parse_duration)]
  pub timeout: Option<time::Duration>,
  #[arg(
    long = "threads",
    help = "Number of worker threads to spawn",
    default_value_t = num::NonZeroUsize::new(8).unwrap()
  )]
  pub threads: num::NonZeroUsize,

  #[arg(long = "user", help = "IMAP user")]
  pub user: String,
  #[arg(last = true, required = true)]
  pub password_command: Vec<String>,

  #[arg(long = "notmuch", help = "Notmuch directory")]
  pub notmuch: Option<String>,
  #[arg(
    long = "maildir",
    help = "Maildir++ directory, relative to the Notmuch directory"
  )]
  pub maildir: String,
  #[arg(
    long = "create",
    help = "Create the Notmuch database if it doesn't exist",
    default_value_t = false
  )]
  pub create: bool,
  #[arg(long = "purgeable", help = "Local mailboxes that can be purged")]
  pub purgeable: Vec<String>,
  #[arg(
    long = "namespace",
    help = "Notmuch property namespace",
    default_value_t = String::from("sin")
  )]
  pub namespace: String,

  #[arg(long = "interruption", help = "Internal testing facility", hide = true)]
  pub interruption: Option<Interruption>,
}

#[derive(Copy, Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum Interruption {
  AppendIsNotTransactional,
  MoveOutOfTmpPostRename,
  StoredFlags,
  SuccessfulMovePreCommit,
}

impl fmt::Display for Interruption {
  fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
    write!(formatter, "{self:?}")
  }
}

impl error::Error for Interruption {}

static INTERRUPTIONS: once_cell::sync::Lazy<
  std::sync::Mutex<collections::HashMap<thread::ThreadId, Interruption>>,
> = once_cell::sync::Lazy::new(|| std::sync::Mutex::new(collections::HashMap::new()));

pub fn interruption(name: &Option<Interruption>) {
  match (
    name,
    INTERRUPTIONS.lock().unwrap().entry(thread::current().id()),
  ) {
    (Some(interruption), collections::hash_map::Entry::Occupied(mut occupied)) => {
      occupied.insert(*interruption);
    }
    (Some(interruption), collections::hash_map::Entry::Vacant(vacant)) => {
      vacant.insert(*interruption);
    }
    (None, collections::hash_map::Entry::Occupied(occupied)) => {
      occupied.remove();
    }
    (None, collections::hash_map::Entry::Vacant(_)) => (),
  }
}

fn interrupt(interruption: Interruption) -> result::Result<(), Interruption> {
  match INTERRUPTIONS.lock().unwrap().get(&thread::current().id()) {
    Some(interruption_) if *interruption_ == interruption => Err(interruption),
    _ => Ok(()),
  }
}

fn inner_run<O>(
  arguments: &Arguments,
  open: &O,
  credentials: &sync::Credentials,
  stream: &mut imap::Stream<O::RW>,
) -> anyhow::Result<()>
where
  O: sync::Open,
{
  // Exchange pleasantries with the server.
  sync::greetings(stream)?;
  if arguments.mode == Mode::ConnectOnly {
    return Ok(());
  }
  sync::authenticate(stream, credentials)?;
  sync::enable(stream)?;

  // Open (or create) the database.
  let notmuch = arguments.notmuch.as_ref().map(path::Path::new);
  let database = match notmuch::Database::<notmuch::Detached>::open(notmuch, &arguments.namespace) {
    Ok(database) => database,
    Err(error) => match error.downcast_ref::<notmuch::Error>() {
      Some(error)
        if arguments.create
          && notmuch.is_some()
          && (error.no_database() /* when notmuch is Some */
              || error.file_error()/* when notmuch is None, weirdly */) =>
      {
        notmuch::Database::<notmuch::Detached>::create(notmuch.unwrap(), &arguments.namespace)?
      }
      Some(_) | None => Err(error)?,
    },
  };

  // Open the maildir and tie the database to it.
  let relative_maildir = path::Path::new(&arguments.maildir);
  anyhow::ensure!(
    relative_maildir.is_relative(),
    "{} must be relative to {:?}",
    arguments.maildir,
    database.path(),
  );
  let maildir_builder = maildir::Builder::new(&database.path().join(relative_maildir))?;
  let mut database = database.attach(maildir_builder.path())?;

  let lastmod = database.lastmod() + 1;

  // Reach consensus with the server.
  database.transaction(|database| sync::move_out_of_tmp(database, relative_maildir))?;
  database.transaction(|database| match arguments.mode {
    Mode::ConnectOnly => unreachable!(),
    Mode::Pull => sync::pull::run(
      open,
      credentials,
      stream,
      database,
      &maildir_builder,
      &arguments.purgeable,
      arguments.threads,
    ),
    Mode::Push => sync::push::run(stream, database, relative_maildir, &maildir_builder),
  })?;
  database.transaction(|database| sync::move_out_of_tmp(database, relative_maildir))?;

  // And show some statistics.
  let mut messages = database.query(&format!(
    "property:\"{}.marker={}\" and lastmod:{lastmod}..{}",
    notmuch::quote(database.namespace()),
    notmuch::MESSAGE_MARKER,
    database.lastmod() + 1
  ))?;
  let mut count = 0;
  while messages.next().is_some() {
    count += 1
  }
  log::info!("{count} message(s) affected");

  Ok(())
}

struct TCP<'a> {
  address: &'a str,
  port: u16,
  timeout: Option<time::Duration>,
}

impl<'a> sync::Open for TCP<'a> {
  type RW = net::TcpStream;

  fn open(&self) -> anyhow::Result<Self::RW> {
    let &Self {
      address,
      port,
      timeout,
      ..
    } = self;
    let address = (address, port)
      .to_socket_addrs()?
      .next()
      .with_context(|| format!("couldn't resolve {address}:{port}"))?;
    log::debug!("connecting to {:?} with timeout {:?}", address, timeout);
    Ok(match timeout {
      Some(duration) => {
        let stream = net::TcpStream::connect_timeout(&address, duration)?;
        stream.set_read_timeout(Some(duration))?;
        stream
      }
      None => net::TcpStream::connect(address)?,
    })
  }
}

struct TLS<'a>(TCP<'a>);

#[ouroboros::self_referencing]
struct TLSStream {
  tcp_stream: net::TcpStream,
  tls_connection: rustls::ClientConnection,
  #[borrows(mut tcp_stream, mut tls_connection)]
  #[covariant]
  tls_stream: rustls::Stream<'this, rustls::ClientConnection, net::TcpStream>,
}

impl imap::ReadWrite for TLSStream {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    self.with_mut(|fields| fields.tls_stream.read(buf))
  }

  fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
    self.with_mut(|fields| fields.tls_stream.write_all(buf))
  }
}

impl<'a> sync::Open for TLS<'a> {
  type RW = TLSStream;

  fn open(&self) -> anyhow::Result<Self::RW> {
    let mut root_store = rustls::RootCertStore::empty();
    for certificate in rustls_native_certs::load_native_certs()? {
      root_store.add(&rustls::Certificate(certificate.0))?
    }
    Ok(
      TLSStreamBuilder {
        tcp_stream: self.0.open()?,
        tls_connection: rustls::ClientConnection::new(
          std::sync::Arc::new(
            rustls::ClientConfig::builder()
              .with_safe_defaults()
              .with_root_certificates(root_store)
              .with_no_client_auth(),
          ),
          self
            .0
            .address
            .try_into()
            .with_context(|| format!("couldn't convert {} to server name", self.0.address))?,
        )?,
        tls_stream_builder: |tcp_stream, tls_connection| {
          rustls::Stream::new(tls_connection, tcp_stream)
        },
      }
      .build(),
    )
  }
}

fn credentials(user: &str, password_command: &[String]) -> anyhow::Result<sync::Credentials> {
  let mut program = process::Command::new(&password_command[0]);
  let command = program.args(&password_command[1..]);
  log::info!("getting password from {command:?}");
  let output = command.output()?;
  let mut stdout = output.stdout;
  anyhow::ensure!(
    output.status.success(),
    "couldn't get password: {command:?} failed"
  );
  let password = str::from_utf8(
    stdout
      .split(|byte| *byte == b'\n')
      .next()
      .with_context(|| format!("{command:?} didn't output anything"))?,
  )
  .with_context(|| format!("{command:?} didn't output UTF-8"))?;
  let credentials = imap::plain(user, password);
  stdout.zeroize();
  Ok(sync::Credentials(credentials))
}

pub fn run(arguments: &Arguments) -> anyhow::Result<()> {
  interruption(&arguments.interruption);
  let credentials = credentials(&arguments.user, &arguments.password_command)?;
  let tcp = TCP {
    address: &arguments.address,
    port: arguments.port,
    timeout: arguments.timeout,
  };
  if !arguments.tls {
    log::warn!("TLS not enabled, credentials will be sent in clear over the wire");
    return inner_run(
      arguments,
      &tcp,
      &credentials,
      &mut imap::Stream::new(tcp.open()?),
    );
  }
  let tls = TLS(tcp);
  inner_run(
    arguments,
    &tls,
    &credentials,
    &mut imap::Stream::new(tls.open()?),
  )
}
