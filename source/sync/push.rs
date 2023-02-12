use crate::{imap, maildir, notmuch, sync};
use anyhow::Context as _;
use std::{collections, fs, io, path};

struct Append {
  uidvalidity: u64,
  uid: u64,
  highestmodseq: u64,
}

fn append<RW>(
  stream: &mut imap::Stream<RW>,
  mailbox: &[u8],
  flags: &collections::HashSet<&str>,
  buffer: &[u8],
) -> anyhow::Result<Append>
where
  RW: io::Read + io::Write,
{
  // .intersperse() is nightly...
  let mut flags_ = "".to_string();
  for (i, flag) in flags.iter().enumerate() {
    flags_ += flag;
    if i + 1 < flags.len() {
      flags_ += " ";
    }
  }
  let command: &[&[u8]] = &[
    b"append APPEND {",
    &mailbox.len().to_string().into_bytes(),
    b"+}\r\n",
    mailbox,
    b" (",
    flags_.as_bytes(),
    b") {",
    &buffer.len().to_string().into_bytes(),
    b"+}\r\n",
  ];
  stream.input(&[command, &[buffer, b"\r\n"]].concat(), command.len())?;
  let mut highestmodseq = None;
  let imap::Append { uidvalidity, uid } = loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(imap::parser::append_data)? {
        highestmodseq_ @ Some(_) => highestmodseq = highestmodseq_,
        None => stream.expect(imap::parser::skip)?,
      },
      b"append" => break stream.expect(imap::parser::append)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  };
  anyhow::ensure!(
    highestmodseq.is_some(),
    "HIGHESTMODSEQ is missing from APPEND"
  );
  // https://www.rfc-editor.org/rfc/rfc4551#section-3.6
  // If the server doesn't support the persistent storage of mod-sequences for the mailbox [...],
  // the server MUST return 0 as the value of HIGHESTMODSEQ status data item.
  let highestmodseq = highestmodseq.unwrap();
  anyhow::ensure!(highestmodseq > 0, "HIGHESTMODSEQ is not properly supported");
  Ok(Append {
    uidvalidity,
    uid,
    highestmodseq,
  })
}

enum Diff {
  Add,
  Delete,
}

fn store<RW>(
  stream: &mut imap::Stream<RW>,
  uid: u64,
  modseq: u64,
  flags: &collections::HashSet<String>,
  diff: Diff,
) -> anyhow::Result<Option<imap::Store>>
where
  RW: io::Read + io::Write,
{
  // While it's not part of the RFC, specifying both +FLAGS.SILENT and -FLAGS.SILENT will result in
  // Dovecot silently ignoring the last occurence.
  let operator = match diff {
    Diff::Add => b"+",
    Diff::Delete => b"-",
  };
  // .intersperse() is nightly...
  let mut flags_ = "".to_string();
  for (i, flag) in flags.iter().enumerate() {
    flags_ += flag;
    if i + 1 < flags.len() {
      flags_ += " ";
    }
  }
  let command: &[&[u8]] = &[
    b"store UID STORE ",
    &uid.to_string().into_bytes(),
    b" (UNCHANGEDSINCE ",
    &modseq.to_string().into_bytes(),
    b") ",
    operator,
    b"FLAGS.SILENT (",
    flags_.as_bytes(),
    b")\r\n",
  ];
  stream.input(command, command.len())?;
  let mut store = None;
  match loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(imap::parser::store_data)? {
        store_ @ Some(_) => store = store_,
        None => stream.expect(imap::parser::skip)?,
      },
      b"store" => break stream.expect(imap::parser::store)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  } {
    Some(uids) => {
      anyhow::ensure!(
        uids.len() == 1 && uids[0].0 == uids[0].1 && uids[0].0 == uid,
        "invalid UID from STORE"
      );
      Ok(None)
    }
    None => {
      anyhow::ensure!(store.is_some(), "FETCH is missing from STORE");
      let store = store.unwrap();
      Ok(Some(store))
    }
  }
}

struct Move {
  uidvalidity: u64,
  uid: u64,
}

fn r#move<RW>(
  stream: &mut imap::Stream<RW>,
  uid: u64,
  mailbox: &[u8],
) -> anyhow::Result<Option<Move>>
where
  RW: io::Read + io::Write,
{
  let command: &[&[u8]] = &[
    b"move UID MOVE ",
    &uid.to_string().into_bytes(),
    b" {",
    &mailbox.len().to_string().into_bytes(),
    b"+}\r\n",
    mailbox,
    b"\r\n",
  ];
  stream.input(command, command.len())?;
  let mut r#move = None;
  // Highestmodseq (if any) is ignored for the same reasons as described in run.
  let _ = loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(imap::parser::move_data)? {
        r#move_ @ Some(_) => r#move = r#move_,
        None => stream.expect(imap::parser::skip)?,
      },
      b"move" => match stream.parse(imap::parser::move_)? {
        Some(result) => break result,
        None => {
          stream.expect(imap::parser::bad)?;
          return Ok(None);
        }
      },
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  };
  match r#move {
    Some(imap::Move {
      uidvalidity,
      from,
      to,
    }) => {
      anyhow::ensure!(
        from.len() == 1
          && to.len() == 1
          && from[0].0 == from[0].1
          && from[0].0 == uid
          && to[0].0 == to[0].1,
        "invalid UID from MOVE"
      );
      Ok(Some(Move {
        uidvalidity,
        uid: to[0].0,
      }))
    }
    // COPYUID is missing but MOVE is allowed to fail partway.
    // For some reason MOVE will report the error but not UID MOVE (which simply reports
    // "OK No messages found")...
    None => Ok(None),
  }
}

fn search_new<'a>(
  database: &'a notmuch::Database<notmuch::Attached>,
  relative_maildir: &path::Path,
  maildir: &maildir::Maildir,
) -> anyhow::Result<notmuch::Messages<'a>> {
  // https://notmuch.readthedocs.io/en/latest/man7/notmuch-search-terms.html
  // folder:<maildir-folder> or folder:/<regex>/ For maildir, this includes messages in the “new”
  // and “cur” subdirectories. The exact syntax for maildir folders depends on your mail
  // configuration. For maildir++, folder:"" matches the inbox folder (which is the root in
  // maildir++), other folder names always start with ".", and nested folders are separated by "."s,
  // such as folder:.classes.topology.
  let folder = relative_maildir.join(if maildir.root() {
    ""
  } else {
    let path = maildir.path();
    let folder = path
      .file_name()
      .with_context(|| format!("couldn't get file name for {path:?}"))?;
    folder
      .to_str()
      .with_context(|| format!("couldn't convert {folder:?} to string"))?
  });
  database.query(&format!(
    "    not property:\"{}.marker={}\" \
     and not property:\"{}.marker={}\" \
     and folder:\"{}\"",
    notmuch::quote(database.root_namespace()),
    notmuch::ROOT_MARKER,
    notmuch::quote(database.namespace()),
    notmuch::MESSAGE_MARKER,
    notmuch::quote(
      folder
        .to_str()
        .with_context(|| format!("couldn't convert {folder:?} to string"))?
    )
  ))
}

fn search_modified<'a>(
  database: &'a notmuch::Database<notmuch::Attached>,
  mailbox: &str,
  lastmod: u64,
) -> anyhow::Result<notmuch::Messages<'a>> {
  let namespace = notmuch::quote(database.namespace());
  let mailbox = notmuch::quote(mailbox);
  database.query(&format!(
    "    property:\"{namespace}.marker={}\" \
     and property:\"{namespace}.mailbox={mailbox}\" \
     and lastmod:{lastmod}..", // The range is inclusive.
    notmuch::MESSAGE_MARKER,
  ))
}

pub fn run<RW>(
  stream: &mut imap::Stream<RW>,
  database: &mut notmuch::Database<notmuch::Attached>,
  relative_maildir: &path::Path,
  maildir_builder: &maildir::Builder,
) -> anyhow::Result<()>
where
  RW: io::Read + io::Write,
{
  // https://www.rfc-editor.org/rfc/rfc7162#section-6
  // After completing a full synchronization, the client MUST also take note of any unsolicited
  // MODSEQ FETCH data items and HIGHESTMODSEQ response codes received from the server. Whenever the
  // client receives a tagged response to a command, it checks the received unsolicited responses to
  // calculate the new HIGHESTMODSEQ value. If the HIGHESTMODSEQ response code is received, the
  // client MUST use it even if it has seen higher mod-sequences. Otherwise, the client calculates
  // the highest value among all MODSEQ FETCH data items received since the last tagged response. If
  // this value is bigger than the client's copy of the HIGHESTMODSEQ value, then the client MUST
  // use this value as its new HIGHESTMODSEQ value.
  //
  // I don't believe we need to handle this in our case: the highestmodseq is completely ignored as
  // part of the push and will be retrieved as part of the pull (at the cost of some wasted effort).

  let lastmod = database.root()?.lastmod()?;

  let mut mailboxes = collections::HashMap::new();
  for mailbox in sync::list(stream)? {
    let maildir = maildir_builder.maildir(&mailbox.string, &mailbox.separator)?;
    mailboxes.insert(maildir.path().to_path_buf(), mailbox);
  }

  for sync::Mailbox {
    bytes: mailbox_bytes,
    string: mailbox_string,
    separator,
  } in mailboxes.values()
  {
    log::info!("pushing to mailbox {mailbox_string}");
    let maildir = maildir_builder.maildir(mailbox_string, separator)?;

    let validity = database.root()?.validity(mailbox_string)?;

    let sync::Select { uidvalidity, .. } =
      sync::select(stream, mailbox_bytes, validity.0, validity.1)?;

    // If the mailbox has changed, the best course of action is to pull (clearing the local cache).
    anyhow::ensure!(
      uidvalidity == validity.0,
      "uidvalidity has changed ({} -> {uidvalidity}), rerun a pull",
      validity.0
    );

    // New messages exist in the database, synchronize them to the server and initialize them.
    let mut messages = search_new(database, relative_maildir, &maildir)?;
    while let Some(mut message) = messages.next() {
      let tags: Vec<String> = message.tags()?.into_iter().map(String::from).collect();
      let tags = tags.iter().map(String::as_str).collect();
      let flags = notmuch::tags_to_flags(&tags);
      log::debug!(
        "uploading message {} (flags:{flags:?})",
        message.message_id()?
      );
      let buffer = fs::read(
        // Taking any path should be okay: Notmuch (well, the Message-ID when present) guarantees
        // they're the same.
        message.paths()?.first().unwrap(), // Guaranteed by Notmuch.
      )?;
      let Append {
        uidvalidity,
        uid,
        // Highestmodseq is only used as modseq for this message.
        // Because push and pull are separate operations, it's likely we could miss some changes
        // that haven't been pulled yet if we were to store that into the root.
        highestmodseq: modseq,
      } = append(stream, mailbox_bytes, &flags, &buffer)?;
      // If interrupted here, we can not know if the append was successful or not. Rerunning the
      // push will result in duplicated emails. The number of duplicated emails can be made smaller
      // by going for smaller transactions. However, the best way to solve this is to always run a
      // pull beforehand, see tests.
      // TODO? when pushing we could generate a lockfile that won't be cleaned up to force users to
      // repull.
      crate::interrupt(crate::Interruption::AppendIsNotTransactional)?;
      message.update_mailbox_properties(mailbox_string, uidvalidity, uid, modseq, &tags)?;
    }

    // Messages were modified locally (the above also counts as a modification so some server
    // operations might be superfluous).
    let mut messages = search_modified(database, mailbox_string, lastmod)?;
    while let Some(mut message) = messages.next() {
      // Message tags might have changed, synchronize them to the server.
      let tags: Vec<String> = message.tags()?.into_iter().map(String::from).collect();
      let tags = tags.iter().map(String::as_str).collect();
      let flags = notmuch::tags_to_flags(&tags);
      let cached_flags: Vec<String> = notmuch::tags_to_flags(&message.cached_tags(mailbox_string)?)
        .into_iter()
        .map(String::from)
        .collect();
      let cached_flags: collections::HashSet<&str> =
        cached_flags.iter().map(String::as_str).collect();
      log::debug!(
        "updating message {} (flags:({cached_flags:?} -> {flags:?}))",
        message.message_id()?
      );
      let uid = message.uid(mailbox_string)?;
      for (mode, flags) in [
        (Diff::Delete, cached_flags.difference(&flags)),
        (Diff::Add, flags.difference(&cached_flags)),
      ] {
        let flags: collections::HashSet<_> = flags.map(|f| f.to_string()).collect();
        if !flags.is_empty() {
          match store(stream, uid, message.modseq(mailbox_string)?, &flags, mode)? {
            Some(imap::Store {
              modseq, ..
            }) => message.update_mailbox_properties(mailbox_string, uidvalidity, uid, modseq, &tags)?,
            None => anyhow::bail!(
              "message {} in {mailbox_string} couldn't be updated with flags {flags:?}, rerun a pull",
              message.message_id()?,
            ),
          }
        }
      }
      crate::interrupt(crate::Interruption::StoredFlags)?;

      // Or a message might have moved, reflect the change on the server.
      let mut found = false;
      let mut maildirs = collections::HashSet::new();
      for path in message.paths()? {
        let [grandparent, _, _] = maildir::components(&path)?;
        if grandparent == maildir.path() {
          found = true;
        } else {
          maildirs.insert(grandparent.to_path_buf());
        }
      }
      let mut cached_mailboxes = message.mailboxes()?;
      if !found && cached_mailboxes.remove(mailbox_string.as_str()) {
        for (path, mailbox) in &mailboxes {
          if !cached_mailboxes.contains(mailbox.string.as_str()) && maildirs.contains(path) {
            // It doesn't matter which destination mailbox is chosen. If duplicates were moved, the
            // end result would be the same.
            log::debug!(
              "moving message {} to {}",
              message.message_id()?,
              mailbox.string
            );
            match r#move(stream, message.uid(mailbox_string)?, &mailbox.bytes)? {
              Some(Move { uidvalidity, uid }) => {
                crate::interrupt(crate::Interruption::SuccessfulMovePreCommit)?;
                // https://www.rfc-editor.org/rfc/rfc6851#section-4.4
                // When one or more messages are moved to a target mailbox, if the server is capable
                // of storing modification sequences for the mailbox, the server MUST generate and
                // assign new modification sequence numbers to the moved messages that are higher
                // than the highest modification sequence of the messages originally in the mailbox.
                //
                // So we can reuse the current one and the pull bump it.
                let modseq = message.modseq(mailbox_string)?;
                let cached_tags: Vec<String> = message
                  .cached_tags(mailbox_string)?
                  .into_iter()
                  .map(String::from)
                  .collect();
                let cached_tags = cached_tags.iter().map(String::as_str).collect();
                message.remove_mailbox_properties(mailbox_string)?;
                message.update_mailbox_properties(
                  &mailbox.string,
                  uidvalidity,
                  uid,
                  modseq,
                  &cached_tags,
                )?;
                break;
              }
              None => anyhow::bail!(
                "message {} couldn't be moved to {}, assuming previously interrupted, rerun a pull",
                message.message_id()?,
                mailbox.string
              ),
            }
          }
        }
      }
    }
  }

  // Avoid spurious lastmod change.
  if lastmod != database.lastmod() {
    database
      .root()?
      .update_lastmod(database.lastmod() + 1 /* for this update */)?;
  }

  Ok(())
}
