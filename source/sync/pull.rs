use crate::{imap, maildir, notmuch, sync};
use anyhow::Context as _;
use std::{collections, fs, io, path, str};

fn reselect<RW>(
  stream: &mut imap::Stream<RW>,
  mailbox: &[u8],
  mut uidvalidity: u64,
  mut highestmodseq: u64,
) -> anyhow::Result<sync::Select>
where
  RW: io::Read + io::Write,
{
  loop {
    // https://www.rfc-editor.org/rfc/rfc3501#section-2.3.1.1
    // If unique identifiers from an earlier session fail to persist in this session, the unique
    // identifier validity value MUST be greater than the one used in the earlier session.
    //
    // The unique identifier of a message MUST NOT change during the session, and SHOULD NOT change
    // between sessions. Any change of unique identifiers between sessions MUST be detectable using
    // the UIDVALIDITY mechanism [...]
    let select = sync::select(stream, mailbox, uidvalidity, highestmodseq)?;
    if select.uidvalidity != uidvalidity {
      (uidvalidity, highestmodseq) = (select.uidvalidity, 0);
    } else {
      return Ok(select);
    }
  }
}

fn fetch<'a, P, R, RW>(
  stream: &'a mut imap::Stream<RW>,
  uid: u64,
  property: &str,
  parser: P,
) -> anyhow::Result<R>
where
  P: Fn(
    &'a [u8],
  )
    -> Result<(usize, (u64, R)), peg::error::ParseError<<[u8] as ::peg::Parse>::PositionRepr>>,
  RW: io::Read + io::Write,
{
  let command: &[&[u8]] = &[
    b"fetch UID FETCH ",
    &uid.to_string().into_bytes(),
    b" (",
    property.as_bytes(),
    b" )\r\n",
  ];
  stream.input(command, command.len())?;
  let mut result = None;
  loop {
    match stream.expect(imap::parser::start)? {
      b"*" => match stream.parse(&parser)? {
        Some((uid_, result_)) => {
          anyhow::ensure!(uid == uid_, "invalid UID returned from FETCH");
          result = Some(result_);
        }
        None => stream.expect(imap::parser::skip)?,
      },
      b"fetch" => break stream.expect(imap::parser::ok)?,
      tag => anyhow::bail!("unexpected tag {tag:?}"),
    }
  }
  anyhow::ensure!(result.is_some(), "{property} is missing from FETCH");
  Ok(result.unwrap())
}

fn search_not_uidvalidity<'a>(
  database: &'a mut notmuch::Database<notmuch::Attached>,
  mailbox: &str,
  uidvalidity: u64,
) -> anyhow::Result<notmuch::Messages<'a>> {
  let namespace = notmuch::quote(database.namespace());
  let mailbox = notmuch::quote(mailbox);
  database.query(&format!(
    "    property:\"{namespace}.marker={}\" \
     and property:\"{namespace}.mailbox={mailbox}\" \
     and not property:\"{namespace}.{mailbox}.uidvalidity={uidvalidity}\"",
    notmuch::MESSAGE_MARKER,
  ))
}

fn search_uids<'a>(
  database: &'a notmuch::Database<notmuch::Attached>,
  mailbox: &str,
  uidvalidity: u64,
  uids: &Vec<u64>,
) -> anyhow::Result<notmuch::Messages<'a>> {
  if uids.is_empty() {
    // Otherwise the query would match all messages.
    return Ok(notmuch::Messages::none());
  }
  let namespace = notmuch::quote(database.namespace());
  let mailbox = notmuch::quote(mailbox);
  let uids = uids
    .iter()
    .map(|uid| format!("property:\"{namespace}.{mailbox}.uid={uid}\""))
    .collect::<Vec<String>>()
    .join(" ");
  database.query(&format!(
    "    property:\"{namespace}.marker={}\" \
     and property:\"{namespace}.mailbox={mailbox}\" \
     and property:\"{namespace}.{mailbox}.uidvalidity={uidvalidity}\" \
     and ({uids})",
    notmuch::MESSAGE_MARKER,
  ))
}

fn remove_message(
  mailbox: &str,
  maildir: &maildir::Maildir,
  message: &mut notmuch::Message<'_>,
) -> anyhow::Result<Vec<path::PathBuf>> {
  log::debug!(
    "removing message {} (uid:{})",
    message.message_id()?,
    message.uid(mailbox)?
  );
  let mut removals = Vec::new();
  for path in message.paths()? {
    if maildir.has(&path) {
      // Removing from the file system is always okay:
      //  - If it's a duplicate, the search query will still find a reference to it and clean up the
      //    properties.
      //  - If it's the last message under this message ID and the transaction is interrupted,
      //    another 'notmuch new' will simply remove all leftovers (unless it's in tmp, in this case
      //    it will be ignored and the search query will still find it).
      match fs::remove_file(&path) {
        Ok(_) => (),
        // Might have been previously removed but interrupted.
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => Err(error)?,
      }
      removals.push(path);
    }
  }
  message.remove_mailbox_properties(mailbox)?;
  Ok(removals)
}

pub fn run<RW>(
  stream: &mut imap::Stream<RW>,
  database: &mut notmuch::Database<notmuch::Attached>,
  maildir_builder: &maildir::Builder,
  purgeable: &[String],
) -> anyhow::Result<()>
where
  RW: io::Read + io::Write,
{
  let mut removals = Vec::new();

  let mailboxes: collections::HashMap<String, sync::Mailbox> = sync::list(stream)?
    .into_iter()
    .map(|m| (m.string.clone(), m))
    .collect();

  for sync::Mailbox {
    bytes: mailbox_bytes,
    string: mailbox_string,
    separator,
  } in mailboxes.values()
  {
    log::info!("pulling from mailbox {mailbox_string}");
    let maildir = maildir_builder.maildir(mailbox_string, separator)?;

    let validity = database.root()?.validity(mailbox_string)?;

    // https://www.rfc-editor.org/rfc/rfc7162#section-3.1.2.1
    // A disconnected client can use the value of HIGHESTMODSEQ to check if it has to refetch
    // metadata from the server. If the UIDVALIDITY value has changed for the selected mailbox,
    // the client MUST delete the cached value of HIGHESTMODSEQ. If UIDVALIDITY for the mailbox is
    // the same, and if the HIGHESTMODSEQ value stored in the client's cache is less than the
    // value returned by the server, then some metadata items on the server have changed since the
    // last synchronization, and the client needs to update its cache.
    let sync::Select {
      vanished,
      mut changes,
      uidvalidity,
      highestmodseq,
    } = reselect(stream, mailbox_bytes, validity.0, validity.1)?;

    {
      // Sanity checking, just in case. There's currently no good way for a user to get out of this
      // predicament: there's no way to edit properties via the Notmuch CLI... Best course of action
      // would be for the server to change the uidvalidity.
      let separator_ = database.root()?.separator(mailbox_string)?;
      anyhow::ensure!(
        validity == (0, 0) || *separator == separator_,
        "separator for {mailbox_string} has changed from {separator_:?} to {separator:?}, \
         refusing to continue"
      );
    }

    // https://www.rfc-editor.org/rfc/rfc4549#section-2
    // If the UIDVALIDITY value returned by the server differs, the client MUST empty the local
    // cache of the mailbox and remove any pending "actions" that refer to UIDs in that mailbox
    // (and consider them failed).
    if uidvalidity != validity.0 {
      // TODO? should we also do a threshold check on the number of vanished messages?
      anyhow::ensure!(
        validity == (0, 0) || purgeable.contains(mailbox_string),
        "{mailbox_string}'s validity has changed on the server, allow to purge it locally (all \
         messages will be removed) by passing --purgeable {mailbox_string}"
      );

      log::debug!(
        "purging messages (uidvalidity:({} -> {uidvalidity}))",
        validity.0
      );
      let mut messages = search_not_uidvalidity(database, mailbox_string, uidvalidity)?;
      while let Some(mut message) = messages.next() {
        removals.append(&mut remove_message(mailbox_string, &maildir, &mut message)?);
      }
    }

    // The updated messages already exist in the database, update them.
    let mut messages = search_uids(
      database,
      mailbox_string,
      uidvalidity,
      &changes.keys().copied().collect(),
    )?;
    while let Some(mut message) = messages.next() {
      let uid = message.uid(mailbox_string)?;
      let modseq = message.modseq(mailbox_string)?;
      let sync::Changes {
        flags,
        modseq: modseq_,
      } = changes
        .remove(&uid) // So the messages aren't added back in the next step.
        .unwrap(); // Guaranteed by the query.
      if modseq == modseq_ {
        // The pull updates the modseq but can not update the highestmodseq due to possible race
        // conditions. Skip to avoid changing the lastmod needlessly.
        continue;
      }
      log::debug!(
        "updating message {} (uidvalidity:{uidvalidity} uid:{uid} modseq:({modseq} -> {modseq_}) flags:({:?} -> {flags:?}))",
        message.message_id()?,
        notmuch::tags_to_flags(&message.tags()?),
      );
      message.update_mailbox_properties(
        mailbox_string,
        uidvalidity,
        uid,
        modseq_,
        &notmuch::flags_to_tags(&flags.iter().map(String::as_str).collect()),
      )?;
      // The message already exists, possibly moving to another directory is okay.
      message.tags_to_maildir_flags()?;
    }

    // The updated messages do not already exist in the database, add them.
    for (uid, sync::Changes { flags, modseq }) in changes {
      // https://www.rfc-editor.org/rfc/rfc3501#section-6.4.5
      // RFC822.SIZE The [RFC-2822] size of the message.
      let size = fetch(stream, uid, "RFC822.SIZE", imap::parser::fetch_size_data)?;
      // Something somewhat unique but not as much as recommended by the maildir 'standard' so we
      // can resume after an interruption. It should never be relied on anywhere else (that's what
      // properties are for): that would break FCC that we can not control.
      let name = format!("{}_{uidvalidity}_{uid}", database.root_namespace());
      let path = match maildir.tmp_named_with_size(&name, size)? {
        Some(path) => {
          log::debug!(
            "reusing previously fetched message (uidvalidity:{uidvalidity} uid:{uid} path:{path:?})",
          );
          path
        }
        None => {
          // https://www.rfc-editor.org/rfc/rfc3501#section-6.4.5
          // BODY.PEEK[<section>]<<partial>> An alternate form of BODY[<section>] that does not
          // implicitly set the \Seen flag.
          let body = fetch(stream, uid, "BODY.PEEK[]", imap::parser::fetch_body_data)?;
          maildir.tmp_named(&name, &body.with_context(|| "BODY.PEEK[] returned NIL")?)?
        }
      };
      let mut message = database.add(&path)?;
      log::debug!(
        "adding message {} (uidvalidity:{uidvalidity} uid:{uid} modseq:{modseq} flags:{flags:?})",
        message.message_id()?
      );
      message.update_mailbox_properties(
        mailbox_string,
        uidvalidity,
        uid,
        modseq,
        &notmuch::flags_to_tags(&flags.iter().map(String::as_str).collect()),
      )?;
      // Do not call tags_to_maildir_flags: this would move the message outside of tmp and it
      // would later be picked by 'notmuch new' even if the transaction fails.
    }

    // The removed messages exist in the database, remove them.
    let mut messages = search_uids(
      database,
      mailbox_string,
      uidvalidity,
      &vanished
        .iter()
        .flat_map(|imap::Range(start, end)| (*start..=*end))
        .collect(),
    )?;
    while let Some(mut message) = messages.next() {
      removals.append(&mut remove_message(mailbox_string, &maildir, &mut message)?);
    }

    // Avoid spurious lastmod change.
    if validity != (uidvalidity, highestmodseq) {
      database.root()?.update_mailbox_properties(
        mailbox_string,
        *separator,
        uidvalidity,
        highestmodseq,
      )?;
    }
  }

  let known_mailboxes: Vec<String> = database
    .root()?
    .mailboxes()?
    .into_iter()
    .map(String::from)
    .collect();
  for known_mailbox in known_mailboxes {
    if !mailboxes.contains_key(&known_mailbox) {
      anyhow::ensure!(
        purgeable.contains(&known_mailbox),
        "{known_mailbox} has been removed on the server, allow to purge it locally (all messages \
         will be removed) by passing --purgeable {known_mailbox}"
      );
      let separator = database.root()?.separator(&known_mailbox)?;
      let maildir = maildir_builder.maildir(&known_mailbox, &separator)?;
      log::debug!("purging messages (mailbox:{known_mailbox})");
      {
        let mut messages = search_not_uidvalidity(database, &known_mailbox, 0)?;
        while let Some(mut message) = messages.next() {
          removals.append(&mut remove_message(&known_mailbox, &maildir, &mut message)?);
        }
      }
      maildir.remove()?;
      database.root()?.remove_mailbox_properties(&known_mailbox)?;
    }
  }

  // Perform the removals last so that a move from a mailbox to another (identified via the
  // Message ID) can be noticed by the database, preventing any local state loss.
  for path in removals {
    database.remove(&path)?;
  }

  Ok(())
}
