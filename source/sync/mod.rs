use crate::{imap, maildir, notmuch};
use anyhow::Context as _;
use std::{borrow, collections, fs, io, path, process, str};
use zeroize::Zeroize as _;

pub mod pull;
pub mod push;

pub fn greetings<RW>(stream: &mut imap::Stream<RW>) -> anyhow::Result<()>
where
  RW: io::Read + io::Write,
{
  // Fetch some data first (the Stream doesn't pull, it bufferizes each response to completion).
  // Assumme we won't end up with a partial read of the greetings.
  stream.read(&mut [0; 32 * 1024])?;
  let capabilities = loop {
    match stream.expect(imap::parser::start)? {
      b"*" => {
        // Some servers send notices.
        if let Ok(Some(capabilities)) = stream.parse(imap::parser::available_capabilities) {
          break capabilities;
        }
      }
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  };
  for capability in [
    // https://www.rfc-editor.org/rfc/rfc3501
    "IMAP4rev1",
    "AUTH=PLAIN",
    // https://www.rfc-editor.org/rfc/rfc5161
    "ENABLE",
    // https://www.rfc-editor.org/rfc/rfc7888
    "LITERAL+",
  ] {
    anyhow::ensure!(
      capabilities.contains(&capability.as_bytes()),
      format!("{capability} is missing from CAPABILITY list")
    );
  }
  Ok(())
}

pub fn authenticate<RW>(
  stream: &mut imap::Stream<RW>,
  user: &str,
  password_command: &[String],
) -> anyhow::Result<()>
where
  RW: io::Read + io::Write,
{
  let mut program = process::Command::new(&password_command[0]);
  let command = program.args(&password_command[1..]);
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
  let mut credentials = imap::plain(user, password);
  stdout.zeroize();
  let command: &[&[u8]] = &[b"authenticate AUTHENTICATE PLAIN "];
  let result = stream.input(
    &[command, &[credentials.as_bytes(), b"\r\n"]].concat(),
    command.len(),
  );
  credentials.zeroize();
  result?;
  let capabilities = loop {
    match stream.expect(imap::parser::start)? {
      b"*" => stream.expect(imap::parser::skip)?,
      b"authenticate" => break stream.expect(imap::parser::available_capabilities)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  };
  for capability in [
    // https://www.rfc-editor.org/rfc/rfc2342
    "NAMESPACE",
    // https://www.rfc-editor.org/rfc/rfc4315 (for APPENDUID, COPYUID)
    "UIDPLUS",
    // https://www.rfc-editor.org/rfc/rfc6851
    "MOVE",
    // https://www.rfc-editor.org/rfc/rfc7162 (for UNCHANGEDSINCE)
    "CONDSTORE",
    "QRESYNC",
  ] {
    anyhow::ensure!(
      capabilities.contains(&capability.as_bytes()),
      format!("{capability} is missing from CAPABILITY list")
    );
  }
  Ok(())
}

pub fn enable<RW>(stream: &mut imap::Stream<RW>) -> anyhow::Result<()>
where
  RW: io::Read + io::Write,
{
  // https://www.rfc-editor.org/rfc/rfc7162
  // The Quick Mailbox Resynchronization (QRESYNC) IMAP extension is an extension [...] that allows
  // a reconnecting client to perform full resynchronization, including discovery of expunged
  // messages, in a single round trip.
  //
  // https://www.rfc-editor.org/rfc/rfc7162#section-3.2
  // Each mailbox that supports persistent storage of mod-sequences, i.e., for which the server
  // would send a HIGHESTMODSEQ untagged OK response code on a successful SELECT/EXAMINE, MUST
  // increment the per-mailbox mod-sequence when one or more messages are expunged due to EXPUNGE,
  // UID EXPUNGE, CLOSE, or MOVE [RFC6851]; the server MUST associate the incremented mod-sequence
  // with the UIDs of the expunged messages.
  //
  // https://www.rfc-editor.org/rfc/rfc7162#section-3.2.3
  // A server compliant with this specification is REQUIRED to support "ENABLE QRESYNC" [...] A
  // client making use of QRESYNC MUST issue "ENABLE QRESYNC" once it is authenticated.
  //
  // https://www.rfc-editor.org/rfc/rfc7162#section-3.2.5
  // A server MUST respond with a tagged BAD response if the Quick Resynchronization parameter to
  // the SELECT/EXAMINE command is specified and the client hasn't issued "ENABLE QRESYNC" in the
  // current connection, or the server has not positively responded to that command with the
  // untagged ENABLED response containing QRESYNC.
  let command: &[&[u8]] = &[b"enable ENABLE QRESYNC\r\n"];
  stream.input(command, command.len())?;
  let mut qresync = false;
  loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(imap::parser::enabled_capabilities)? {
        Some(capabilities) => qresync = capabilities.contains(&&b"QRESYNC"[..]),
        None => stream.expect(imap::parser::skip)?,
      },
      b"enable" => break stream.expect(imap::parser::ok)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  }
  anyhow::ensure!(qresync, "QRESYNC is not ENABLEd");
  Ok(())
}

#[derive(Debug)]
struct Mailbox {
  bytes: Vec<u8>,
  string: String,
  separator: Option<char>,
}

fn list<RW>(stream: &mut imap::Stream<RW>) -> anyhow::Result<Vec<Mailbox>>
where
  RW: io::Read + io::Write,
{
  let command: &[&[u8]] = &[b"list LIST \"\" \"*\"\r\n"];
  stream.input(command, command.len())?;
  let mut mailboxes = Vec::new();
  loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(imap::parser::list_mailbox)? {
        Some((flags, separator, mailbox)) => {
          if flags.contains(&&b"\\Noselect"[..]) {
            // https://www.rfc-editor.org/rfc/rfc3501#section-7.2.2
            // \Noselect It is not possible to use this name as a selectable mailbox.
            continue;
          }
          let bytes = match mailbox {
            imap::Mailbox::Inbox => b"INBOX".to_vec(),
            imap::Mailbox::Other(borrow::Cow::Owned(mailbox)) => mailbox,
            imap::Mailbox::Other(borrow::Cow::Borrowed(mailbox)) => mailbox.to_vec(),
          };
          mailboxes.push(Mailbox {
            string: imap::utf7_to_utf8(&bytes)
              .with_context(|| format!("mailbox {bytes:?} isn't proper modified UTF-7"))?,
            bytes,
            separator: separator.map(|s| s as char /* guaranteed by TEXT-CHAR */),
          });
        }
        None => stream.expect(imap::parser::skip)?,
      },
      b"list" => break stream.expect(imap::parser::ok)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  }
  Ok(mailboxes)
}

#[derive(Debug)]
struct Changes {
  flags: Vec<String>,
  modseq: u64,
}

#[derive(Debug)]
struct Select {
  uidvalidity: u64,
  highestmodseq: u64,
  vanished: Vec<imap::Range>,
  changes: collections::HashMap<u64 /* uid */, Changes>,
}

fn select<RW>(
  stream: &mut imap::Stream<RW>,
  mailbox: &[u8],
  uidvalidity: u64,
  highestmodseq: u64,
) -> anyhow::Result<Select>
where
  RW: io::Read + io::Write,
{
  let command: &[&[u8]] = &[
    b"select SELECT {",
    &mailbox.len().to_string().into_bytes(),
    b"+}\r\n",
    mailbox,
    b" (QRESYNC (",
    &uidvalidity.to_string().into_bytes(),
    b" ",
    &highestmodseq.to_string().into_bytes(),
    b"))\r\n",
  ];
  stream.input(command, command.len())?;
  let (mut user_keywords, mut uidvalidity, mut highestmodseq, mut vanished, mut changes) =
    (false, None, None, Vec::new(), collections::HashMap::new());
  loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(imap::parser::select_data)? {
        // https://www.rfc-editor.org/rfc/rfc3501#section-7.1
        // The PERMANENTFLAGS list can also include the special flag \*, which indicates that it is
        // possible to create new keywords by attempting to store those flags in the mailbox.
        Some(imap::Select::Flags(flags)) => user_keywords = flags.contains(&&b"\\*"[..]),
        Some(imap::Select::UIDValidity(uidvalidity_)) => uidvalidity = Some(uidvalidity_),
        Some(imap::Select::HighestModSeq(highestmodseq_)) => highestmodseq = Some(highestmodseq_),
        Some(imap::Select::Vanished(mut uids)) => vanished.append(&mut uids),
        Some(imap::Select::Fetch(imap::SelectFetch { uid, flags, modseq })) => {
          let flags = flags
            .iter()
            .map(|flag| {
              str::from_utf8(flag)
                .unwrap() // Guaranteed by the BNF.
                .to_string()
            })
            .collect();
          changes.insert(uid, Changes { flags, modseq });
        }
        None => stream.expect(imap::parser::skip)?,
      },
      b"select" => break stream.expect(imap::parser::ok)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  }
  anyhow::ensure!(user_keywords, "PERMANENTFLAGS \\* is missing from SELECT");
  anyhow::ensure!(uidvalidity.is_some(), "UIDVALIDITY is missing from SELECT");
  anyhow::ensure!(
    highestmodseq.is_some(),
    "HIGHESTMODSEQ is missing from SELECT"
  );
  // https://www.rfc-editor.org/rfc/rfc4551#section-3.6
  // If the server doesn't support the persistent storage of mod-sequences for the mailbox (see
  // Section 3.1.2), the server MUST return 0 as the value of HIGHESTMODSEQ status data item.
  let highestmodseq = highestmodseq.unwrap();
  anyhow::ensure!(highestmodseq > 0, "HIGHESTMODSEQ is not properly supported");
  Ok(Select {
    uidvalidity: uidvalidity.unwrap(),
    highestmodseq,
    vanished,
    changes,
  })
}

pub fn move_out_of_tmp(
  database: &mut notmuch::Database<notmuch::Attached>,
  relative_maildir: &path::Path,
) -> anyhow::Result<()> {
  let folder = relative_maildir
    .file_name()
    .with_context(|| format!("couldn't get file name for {relative_maildir:?}"))?;
  let folder = folder
    .to_str()
    .with_context(|| format!("couldn't convert {folder:?} to string"))?;
  let mut messages = database.query(&format!(
    "    property:\"{}.marker={}\" \
     and path:\"{}/**\" \
     and path:/tmp/[^/]+$/",
    notmuch::quote(database.namespace()),
    notmuch::MESSAGE_MARKER,
    notmuch::quote(folder),
  ))?;
  while let Some(message) = messages.next() {
    for path in message.paths()? {
      let components @ [grandparent, _, _] = maildir::components(&path)?;
      let [_, parent_name, file_name] = maildir::components_to_str(&components)?;
      if parent_name == "tmp" {
        log::debug!("moving message {} out of tmp", message.message_id()?);
        let new = grandparent // Don't end up with '..' in the database...
          .join("new")
          .join(file_name);
        match fs::rename(&path, &new) {
          Ok(_) => (),
          // Might have been previously removed but interrupted.
          Err(error) if error.kind() == io::ErrorKind::NotFound => (),
          Err(error) => Err(error)?,
        }
        crate::interrupt(crate::Interruption::MoveOutOfTmpPostRename)?;
        let mut message = database.add(&new)?;
        message.tags_to_maildir_flags()?; // If necessary, move from new to cur based on flags.
        database.remove(&path)?;
      }
    }
  }
  Ok(())
}
