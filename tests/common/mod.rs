use anyhow::Context as _;
use std::{io, ops, panic, path, process, thread, time};

#[derive(Debug)]
pub struct Child(process::Child);

impl ops::Drop for Child {
  fn drop(&mut self) {
    if let Err(error) = self.0.kill() {
      log::warn!("couldn't kill {self:?} {error}")
    }
  }
}

pub mod dovecot;
mod maildir;
mod notmuch;

#[derive(Clone)]
pub struct Runner {
  directory: path::PathBuf,
  output: path::PathBuf,
  port: u16,
  user: String,
  password: String,
  purgeable: Vec<String>,
  interruption: Option<sin::Interruption>,
}

impl Runner {
  fn new(directory: &path::Path, port: u16) -> Self {
    Self {
      directory: directory.to_path_buf(),
      output: directory.join("output"),
      port,
      user: "user".to_string(),
      password: "password".to_string(),
      purgeable: Vec::new(),
      interruption: None,
    }
  }

  pub fn with_user(&self, user: &str) -> Self {
    Self {
      user: user.to_string(),
      ..self.clone()
    }
  }

  pub fn with_password(&self, password: &str) -> Self {
    Self {
      password: password.to_string(),
      ..self.clone()
    }
  }

  pub fn with_purgeable(&self, mailbox: &str) -> Self {
    Self {
      purgeable: vec![mailbox.to_string()],
      ..self.clone()
    }
  }

  pub fn with_interruption(&self, interruption: sin::Interruption) -> Self {
    Self {
      interruption: Some(interruption),
      ..self.clone()
    }
  }

  fn server_maildir_builder(&self) -> io::Result<sin::maildir::Builder> {
    sin::maildir::Builder::new(&self.directory.join(&self.user).join("maildir"))
  }

  pub fn server_maildir(
    &self,
    mailbox: &str,
    separator: &Option<char>,
  ) -> io::Result<sin::maildir::Maildir> {
    self.server_maildir_builder()?.maildir(mailbox, separator)
  }

  pub fn run(&self, mode: sin::Mode) -> anyhow::Result<()> {
    let arguments = sin::Arguments {
      mode,
      address: "localhost".to_string(),
      port: self.port,
      tls: false,
      timeout: Some(time::Duration::new(10, 0)),
      user: self.user.clone(),
      password_command: vec!["echo".to_string(), self.password.clone()],
      notmuch: Some(
        self
          .output
          .to_str()
          .with_context(|| "invalid directory")?
          .to_string(),
      ),
      maildir: self.user.to_string(),
      create: true,
      purgeable: self.purgeable.clone(),
      namespace: "sin".to_string(),
      interruption: self.interruption,
    };
    match &self.interruption {
      Some(interruption) => {
        let error = sin::run(&arguments).unwrap_err();
        match error.downcast_ref::<sin::Interruption>() {
          Some(interruption_) => {
            assert_eq!(interruption, interruption_);
            Ok(())
          }
          None => Err(error)?,
        }
      }
      None => sin::run(&arguments),
    }
  }

  pub fn client_maildir_builder(&self) -> io::Result<sin::maildir::Builder> {
    sin::maildir::Builder::new(&self.output.join(&self.user))
  }

  pub fn client_maildir(
    &self,
    mailbox: &str,
    separator: &Option<char>,
  ) -> io::Result<sin::maildir::Maildir> {
    self.client_maildir_builder()?.maildir(mailbox, separator)
  }

  pub fn maildir_count(
    &self,
    maildir: &sin::maildir::Maildir,
  ) -> anyhow::Result<(usize, usize, usize)> {
    maildir::count(maildir)
  }

  pub fn notmuch_dump(&self) -> anyhow::Result<String> {
    notmuch::dump(&self.output)
  }

  pub fn notmuch_new(&self) -> anyhow::Result<()> {
    notmuch::run(&self.output, &["new", "--no-hooks"])
  }

  pub fn notmuch_tag(&self, tag: &str, query: &str) -> anyhow::Result<()> {
    notmuch::run(&self.output, &["tag", tag, "--", query])
  }
}

pub fn setup<B, S>(server: S, body: B)
where
  B: Fn(&Runner) -> anyhow::Result<()> + panic::RefUnwindSafe,
  S: Fn() -> anyhow::Result<(tempfile::TempDir, Child, u16)>,
{
  let (directory, _child /* killed at the end of the function */, port) = server().unwrap();
  let runner = Runner::new(directory.path(), port);
  log::debug!("waiting for the server to be ready...");
  while let Err(error) = runner.run(sin::Mode::ConnectOnly) {
    log::trace!("error while waiting for the server to be ready: {error:?}");
    thread::sleep(time::Duration::from_millis(100));
  }
  log::debug!("server ready");
  match panic::catch_unwind(|| body(&runner).unwrap()) {
    Ok(()) => (),
    Err(error) => {
      let path = directory.into_path(); // This prevents the removal of the directory.
      log::error!("keeping {}", path.display());
      panic::resume_unwind(error)
    }
  }
}

pub fn email(id: &str) -> String {
  format!(
    "From: {id}
To: {id}
Subject: {id}
Message-ID: {id}

{id}"
  )
}
