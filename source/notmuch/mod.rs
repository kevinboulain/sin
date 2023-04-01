// TODO: property keys containing '=' will be refused by Notmuch.

use std::{cmp, collections, fs, io::Write as _, path};

mod bindings;
pub use bindings::Error;

// Ideally, something that doesn't need quoting.
pub const ROOT_MARKER: &str = "root";
pub const MESSAGE_MARKER: &str = "message";

pub fn quote(str: &str) -> String {
  // Properties are just regular terms and should be quoted when they have spaces:
  //  notmuch --config '' search 'property:"sin.folder with spaces.highestmodseq=2"'
  // When they have quotes, escape them:
  //  notmuch --config '' search 'property:"sin.folder with spaces and ""quotes"".highestmodseq=2"'
  let mut quoted = String::with_capacity(str.len());
  for char in str.chars() {
    if char == '"' {
      quoted.push('"');
    }
    quoted.push(char);
  }
  quoted
}

fn replace_property(
  message: &mut bindings::Message<'_>,
  namespace: &str,
  property: &str,
  old_value: Option<&str>,
  new_value: Option<&str>,
) -> anyhow::Result<()> {
  let property = format!("{namespace}.{property}");
  match old_value {
    Some(value) => message.remove_property(&property, value)?,
    None => message.remove_all_properties(&property)?,
  }
  if let Some(value) = new_value {
    message.add_property(&property, value)?;
  }
  Ok(())
}

fn property<'a>(
  message: &'a bindings::Message<'_>,
  namespace: &'_ str,
  property: &'_ str,
) -> anyhow::Result<Option<&'a str>> {
  let mut value = None;
  let mut properties = message.properties(&format!("{namespace}.{property}"), true)?;
  while let Some((_, value_)) = properties.next()? {
    value = Some(value_)
  }
  Ok(value)
}

fn properties<'a>(
  message: &'a bindings::Message<'_>,
  namespace: &'_ str,
  property: &'_ str,
) -> anyhow::Result<collections::HashSet<&'a str>> {
  let mut values = collections::HashSet::new();
  let mut properties = message.properties(&format!("{namespace}.{property}"), true)?;
  while let Some((_, mailbox)) = properties.next()? {
    values.insert(mailbox);
  }
  Ok(values)
}

#[derive(Debug)]
pub struct RootMessage<'a> {
  inner: bindings::Message<'a>,
  namespace: &'a str,
}

impl<'a> RootMessage<'a> {
  fn setup(&mut self) -> anyhow::Result<()> {
    let namespace = self.namespace;
    // For search.exclude_tags.
    self.inner.add_tag(&format!("{namespace}.internal"))?;
    // The marker
    replace_property(
      &mut self.inner,
      namespace,
      "marker",
      None,
      Some(ROOT_MARKER),
    )
  }

  fn inner_id(message: &bindings::Message<'_>) -> anyhow::Result<u64> {
    // Guaranteed by Database<Detached>::add.
    Ok(message.id()?.split_once('@').unwrap().0.parse().unwrap())
  }

  fn id(&self) -> anyhow::Result<u64> {
    Self::inner_id(&self.inner)
  }

  pub fn validity(&self, mailbox: &str) -> anyhow::Result<(u64, u64)> {
    // An uidvalidity of 0 is actually not supported by the BNF (nz-number) and a highestmodseq of 0
    // is only ever returned when the server doesn't support persistent storage. If this ever ends
    // up refused by a server, we could instead go for (2^32-1, 2^63−1) because a server that
    // reaches these values would have painted itself in the corner anyway and would need to wrap
    // over:
    //
    // https://www.rfc-editor.org/rfc/rfc3501#section-2.3.1.1
    // A [UID] 32-bit value assigned to each message, which when used with the unique identifier
    // validity value (see below) forms a 64-bit value
    //
    // https://www.rfc-editor.org/rfc/rfc7162.html#section-3.1
    // RFC 4551 defined mod-sequences as unsigned 64-bit values. In order to make implementations on
    // various platforms (such as Java) easier, this version of the document redefines them as
    // unsigned 63-bit values.
    let uidvalidity = property(
      &self.inner,
      self.namespace,
      &format!("{mailbox}.uidvalidity"),
    )?
    .unwrap_or("0")
    .parse()
    .unwrap(); // Guaranteed by update_validity.
    let highestmodseq = property(
      &self.inner,
      self.namespace,
      &format!("{mailbox}.highestmodseq"),
    )?
    .unwrap_or("0")
    .parse()
    .unwrap(); // Guaranteed by update_validity.
    Ok((uidvalidity, highestmodseq))
  }

  pub fn update_mailbox_properties(
    &mut self,
    mailbox: &str,
    separator: Option<char>,
    uidvalidity: u64,
    highestmodseq: u64,
  ) -> anyhow::Result<()> {
    for (property, old_value, new_value) in [
      ("mailbox", Some(mailbox), Some(mailbox)),
      (
        &format!("{mailbox}.separator"),
        None,
        separator.map(|s| format!("{s}")).as_deref(),
      ),
      (
        &format!("{mailbox}.uidvalidity"),
        None,
        Some(uidvalidity.to_string().as_str()),
      ),
      (
        &format!("{mailbox}.highestmodseq"),
        None,
        Some(highestmodseq.to_string().as_str()),
      ),
    ] {
      replace_property(
        &mut self.inner,
        self.namespace,
        property,
        old_value,
        new_value,
      )?;
    }
    Ok(())
  }

  pub fn lastmod(&self) -> anyhow::Result<u64> {
    Ok(
      property(&self.inner, self.namespace, "lastmod")?
        .unwrap_or("0")
        .parse()
        .unwrap(), // Guaranteed by update_lastmod.
    )
  }

  pub fn update_lastmod(&mut self, lastmod: u64) -> anyhow::Result<()> {
    replace_property(
      &mut self.inner,
      self.namespace,
      "lastmod",
      None,
      Some(&lastmod.to_string()),
    )
  }

  pub fn remove_mailbox_properties(&mut self, mailbox: &str) -> anyhow::Result<()> {
    for (property, old_value) in [
      ("mailbox", Some(mailbox)),
      // The mailbox properties.
      (&format!("{mailbox}.uidvalidity"), None),
      (&format!("{mailbox}.highestmodseq"), None),
      (&format!("{mailbox}.separator"), None),
    ] {
      replace_property(&mut self.inner, self.namespace, property, old_value, None)?;
    }
    Ok(())
  }

  pub fn mailboxes(&self) -> anyhow::Result<collections::HashSet<&str>> {
    properties(&self.inner, self.namespace, "mailbox")
  }

  pub fn separator(&self, mailbox: &str) -> anyhow::Result<Option<char>> {
    Ok(
      property(&self.inner, self.namespace, &format!("{mailbox}.separator"))?
        // Guaranteed by update_mailbox_properties.
        .map(|s| s.chars().next().unwrap()),
    )
  }
}

#[derive(Debug)]
pub struct Message<'a> {
  inner: bindings::Message<'a>,
  namespace: &'a str,
}

impl<'a> Message<'a> {
  pub fn message_id(&'_ self) -> anyhow::Result<&'_ str> {
    Ok(self.inner.id()?)
  }

  pub fn mailboxes(&self) -> anyhow::Result<collections::HashSet<&str>> {
    properties(&self.inner, self.namespace, "mailbox")
  }

  pub fn uid(&self, mailbox: &str) -> anyhow::Result<u64> {
    Ok(
      property(&self.inner, self.namespace, &format!("{mailbox}.uid"))?
        // Guaranteed by update_mailbox_properties.
        .unwrap()
        .parse()
        .unwrap(),
    )
  }

  pub fn modseq(&self, mailbox: &str) -> anyhow::Result<u64> {
    Ok(
      property(&self.inner, self.namespace, &format!("{mailbox}.modseq"))?
        // Guaranteed by update_mailbox_properties.
        .unwrap()
        .parse()
        .unwrap(),
    )
  }

  pub fn paths(&self) -> anyhow::Result<Vec<path::PathBuf>> {
    Ok(self.inner.paths()?)
  }

  pub fn cached_tags(&self, mailbox: &str) -> anyhow::Result<collections::HashSet<&str>> {
    properties(&self.inner, self.namespace, &format!("{mailbox}.tag"))
  }

  pub fn tags(&'_ self) -> anyhow::Result<collections::HashSet<&'_ str>> {
    Ok(self.inner.tags()?)
  }

  pub fn remove_mailbox_properties(&mut self, mailbox: &str) -> anyhow::Result<()> {
    let namespace = self.namespace;
    for (property, old_value) in [
      // The affected mailbox.
      ("mailbox", Some(mailbox)),
      // The mailbox properties.
      (&format!("{mailbox}.uidvalidity"), None),
      (&format!("{mailbox}.uid"), None),
      (&format!("{mailbox}.modseq"), None),
      (&format!("{mailbox}.tag"), None),
    ] {
      replace_property(&mut self.inner, namespace, property, old_value, None)?;
    }
    // The marker when there's nothing left.
    let mut count = 0;
    {
      let mut properties = self.inner.properties(&format!("{namespace}."), false)?;
      while properties.next()?.is_some() {
        count += 1;
      }
    }
    if count == 1 {
      replace_property(&mut self.inner, namespace, "marker", None, None)?;
    }
    Ok(())
  }

  pub fn update_mailbox_properties(
    &mut self,
    mailbox: &str,
    uidvalidity: u64,
    uid: u64,
    modseq: u64,
    tags: &collections::HashSet<&str>,
  ) -> anyhow::Result<()> {
    // TODO? should these properties be multi-valued? I'm not sure what it would bring to the
    // table...
    if let Ok(Some(current_uidvalidity)) = property(
      &self.inner,
      self.namespace,
      &format!("{mailbox}.uidvalidity"),
    ) {
      if current_uidvalidity.parse::<u64>().unwrap() == uidvalidity && self.uid(mailbox)? != uid {
        log::warn!(
          "message {} has duplicates in {mailbox} but the property system doesn't handle this \
           edge case currently and if it did, all flags would end up the same given how Notmuch \
           handles them (get rid of this warning by removing the duplicates)",
          self.message_id()?
        );
      }
    }

    for (property, old_value, new_value) in [
      // The marker
      ("marker", None, Some(MESSAGE_MARKER)),
      // The affected mailbox.
      ("mailbox", Some(mailbox), Some(mailbox)),
      // The mailbox properties.
      (
        &format!("{mailbox}.uidvalidity"),
        None,
        Some(&uidvalidity.to_string()),
      ),
      (&format!("{mailbox}.uid"), None, Some(&uid.to_string())),
      (
        &format!("{mailbox}.modseq"),
        None,
        Some(&modseq.to_string()),
      ),
    ] {
      replace_property(
        &mut self.inner,
        self.namespace,
        property,
        old_value,
        new_value,
      )?;
    }
    // Update the current tags and the cached copy (so local changes can be detected).
    let cached_tags: Vec<String> = self
      .cached_tags(mailbox)?
      .into_iter()
      .map(String::from)
      .collect();
    let cached_tags: collections::HashSet<&str> = cached_tags.iter().map(String::as_str).collect();
    let property = format!("{mailbox}.tag");
    for tag in cached_tags.difference(tags) {
      replace_property(&mut self.inner, self.namespace, &property, Some(tag), None)?;
      self.inner.remove_tag(tag)?;
    }
    for tag in tags.difference(&cached_tags) {
      replace_property(
        &mut self.inner,
        self.namespace,
        &property,
        Some(tag),
        Some(tag),
      )?;
      self.inner.add_tag(tag)?;
    }
    Ok(())
  }

  pub fn tags_to_maildir_flags(&mut self) -> anyhow::Result<()> {
    // If this message is in a maildir, rename it to reflect the updated flags.
    self.inner.tags_to_maildir_flags()?;
    Ok(())
  }
}

#[derive(Debug)]
pub struct Messages<'a> {
  inner: Option<bindings::Messages<'a>>,
  namespace: &'a str,
}

impl<'a> Messages<'a> {
  pub fn none() -> Self {
    Self {
      inner: None,
      namespace: "",
    }
  }

  pub fn next(&'_ mut self) -> Option<Message<'_>> {
    match &mut self.inner {
      Some(ref mut inner) => inner.next().map(|message| Message {
        inner: message,
        namespace: self.namespace,
      }),
      None => None,
    }
  }
}

pub struct Database<S> {
  inner: bindings::Database,
  transaction: bool,
  state: S,
}

impl<S> Database<S> {
  pub fn transaction<B, R>(&mut self, mut body: B) -> anyhow::Result<R>
  where
    B: FnMut(&mut Self) -> anyhow::Result<R>,
  {
    // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
    // Note that, unlike a typical database transaction, this only ensures atomicity, not
    // durability; neither begin nor end necessarily flush modifications to disk.
    //
    // For writable databases, notmuch_database_close commits all changes to disk before closing the
    // database, unless the caller is currently in an atomic section (there was a
    // notmuch_database_begin_atomic without a matching notmuch_database_end_atomic). In this case
    // changes since the last commit are discarded.
    assert!(!self.transaction, "nested transactions aren't supported");
    self.inner.begin_atomic()?;
    self.transaction = true;
    // https://github.com/vhdirk/notmuch-rs/blob/master/src/database.rs#L498
    // AtomicOperation implements Drop, it's not suitable for our usage: we shouldn't commit if
    // anything failed at all.
    match body(self) {
      Ok(result) => {
        // https://github.com/notmuch/notmuch/blob/master/lib/notmuch.h
        // Indicate the end of an atomic database operation. If repeated (with matching
        // notmuch_database_begin_atomic) "database.autocommit" times, commit the the transaction
        // and all previous (non-cancelled) transactions to the database.
        self.transaction = false;
        self.inner.end_atomic()?;
        // As hinted above: until database.autocommit is reached, all the transactions must be
        // successful for the commit to happen when the database is closed.
        // In the following example (assuming database.autocommit >= 2):
        //  notmuch_database_begin_atomic
        //  notmuch_database_end_atomic (success)
        //  notmuch_database_begin_atomic
        //  notmuch_database_close (failure)
        // The first transaction will be dropped even though it was successful.
        // Hence, nested transactions aren't supported.
        self.inner.reopen()?;
        Ok(result)
      }
      Err(error) => {
        // Because the atomic context hasn't been exited no other action will go through. As such,
        // the only reasonable thing to do is to reopen the database and let the caller do what they
        // think is best.
        self.transaction = false;
        self.inner.reopen()?;
        Err(error)
      }
    }
  }

  pub fn remove(&self, path: &path::Path) -> anyhow::Result<()> {
    self.inner.remove_message(path)?;
    Ok(())
  }

  pub fn path(&self) -> &path::Path {
    self.inner.path()
  }

  pub fn lastmod(&self) -> u64 {
    self.inner.lastmod()
  }
}

pub struct Detached {
  namespace: String,
}

impl Database<Detached> {
  pub fn open(path: Option<&path::Path>, namespace: &str) -> anyhow::Result<Database<Detached>> {
    Ok(Database::<Detached> {
      inner: bindings::Database::open(path)?,
      transaction: false,
      state: Detached {
        namespace: namespace.to_string(),
      },
    })
  }

  pub fn create(path: &path::Path, namespace: &str) -> anyhow::Result<Database<Detached>> {
    fs::create_dir_all(path)?;
    Ok(Database::<Detached> {
      inner: bindings::Database::create(path)?,
      transaction: false,
      state: Detached {
        namespace: namespace.to_string(),
      },
    })
  }

  pub fn attach(mut self, path: &path::Path) -> anyhow::Result<Database<Attached>> {
    let root_path = path.join(&self.state.namespace);
    let id = match self.find(&root_path)? {
      Some(message) => Some(message.id()?),
      None => None, // The borrow checker doesn't like calling self.add(&root_path) here.
    };
    let id = match id {
      Some(id) => id,
      None => self.add(&root_path)?,
    };
    let namespace = format!("{}.{id}", self.state.namespace);
    Ok(Database::<Attached> {
      inner: self.inner,
      transaction: self.transaction,
      state: Attached {
        detached: self.state,
        path: path.to_path_buf(),
        namespace,
      },
    })
  }

  fn add(&'_ mut self, path: &path::Path) -> anyhow::Result<u64> {
    self.transaction(|database| {
      let namespace = &database.state.namespace;
      let mut ids = collections::HashSet::new();
      let mut max_id = 0;
      let mut messages = database
        .inner
        .query(&format!("property:{namespace}.marker={ROOT_MARKER}"))?;
      while let Some(message) = messages.next() {
        let root_id = RootMessage::inner_id(&message)?;
        ids.insert(root_id);
        max_id = cmp::max(max_id, root_id);
      }
      if !ids.is_empty() {
        max_id += 1; // Start at 0 otherwise.
      }

      for id in (0..=max_id)
        .collect::<collections::HashSet<_>>()
        .difference(&ids)
      {
        // Cleanup loose ends.
        // TODO? If more than the last ID was removed, we have no way to find out (but the ID will
        // be reused when asked to).
        let property_prefix = format!("{namespace}.{id}.");
        let mut messages = database.inner.query(&format!(
          "property:{property_prefix}marker={MESSAGE_MARKER}"
        ))?;
        while let Some(mut message) = messages.next() {
          message.remove_all_properties_with_prefix(&property_prefix)?;
        }
      }

      let mut file = fs::File::create(path)?; // Truncates the file if it exists.
      file.write_all(
        format!(
          "Subject: DO NOT REMOVE, THIS KEEPS TRACKS OF {namespace}'S INTERNAL STATE
Message-ID: {max_id}@{namespace}
"
        )
        .as_bytes(),
      )?;
      file.sync_all()?;

      let mut message = RootMessage {
        inner: database.inner.index_message(path)?,
        namespace: &database.state.namespace,
      };
      message.setup()?;
      message.id()
    })
  }

  fn find(&'_ self, path: &path::Path) -> anyhow::Result<Option<RootMessage<'_>>> {
    Ok(
      self
        .inner
        .find_message_by_filename(path)?
        .map(|message| RootMessage {
          inner: message,
          namespace: &self.state.namespace,
        }),
    )
  }
}

pub struct Attached {
  detached: Detached,
  path: path::PathBuf,
  namespace: String,
}

impl Database<Attached> {
  pub fn root_namespace(&self) -> &str {
    &self.state.detached.namespace
  }

  pub fn namespace(&self) -> &str {
    &self.state.namespace
  }

  pub fn add(&'_ self, path: &path::Path) -> anyhow::Result<Message<'_>> {
    Ok(Message {
      inner: self.inner.index_message(path)?,
      namespace: &self.state.namespace,
    })
  }

  pub fn query(&'_ self, query: &str) -> anyhow::Result<Messages<'_>> {
    let query = query.trim(); // The query might be indented for readability.
    log::debug!("? {query}");
    Ok(Messages {
      inner: Some(self.inner.query(query)?),
      namespace: self.namespace(),
    })
  }

  pub fn root(&'_ self) -> anyhow::Result<RootMessage<'_>> {
    // Sadly, it doesn't look like we can upcast from Database<Attached> easily so
    // Database::<Detached>::find is reimplemented here.
    let root_namespace = self.root_namespace();
    Ok(
      self
        .inner
        .find_message_by_filename(&self.state.path.join(root_namespace))?
        .map(|message| RootMessage {
          inner: message,
          namespace: root_namespace,
        })
        .unwrap(), // Guaranteed by Database::<Detached>::attach.
    )
  }
}

pub fn flags_to_tags<'a>(
  flags: &'_ collections::HashSet<&'a str>,
) -> collections::HashSet<&'a str> {
  // https://www.rfc-editor.org/rfc/rfc3501#section-2.3.2
  // The currently-defined system flags are:
  //  \Seen [...]
  //  \Answered [...]
  //  \Flagged [...]
  //  \Deleted [...]
  //  \Draft [...]
  //  \Recent [...]
  //
  // https://notmuch.readthedocs.io/en/latest/man1/notmuch-config.html
  // maildir.synchronize_flags
  //  If true, then the following maildir flags (in message filenames) will be synchronized with the
  //  corresponding notmuch tags:
  //   Flag Tag
  //   D    draft
  //   F    flagged
  //   P    passed
  //   R    replied
  //   S    unread (added when 'S' flag is not present)
  //
  // https://www.rfc-editor.org/rfc/rfc3501#section-2.3.2
  // Keywords do not begin with "\".
  let mut tags = collections::HashSet::new();
  if !flags.contains("\\Seen") {
    tags.insert("unread");
  }
  for flag in flags {
    tags.insert(if *flag == "\\Answered" {
      "replied"
    } else if *flag == "\\Flagged" {
      "flagged"
    } else if *flag == "\\Draft" {
      "draft"
    } else if flag.starts_with('\\') {
      continue;
    } else {
      flag
    });
  }
  tags
}

pub fn tags_to_flags<'a>(tags: &'_ collections::HashSet<&'a str>) -> collections::HashSet<&'a str> {
  let mut flags = collections::HashSet::new();
  let mut unread = false;
  for tag in tags {
    flags.insert(if *tag == "unread" {
      unread = true;
      continue;
    } else if *tag == "replied" {
      "\\Answered"
    } else if *tag == "flagged" {
      "\\Flagged"
    } else if *tag == "draft" {
      "\\Draft"
    } else {
      tag
    });
  }
  if !unread {
    flags.insert("\\Seen");
  }
  flags
}

#[cfg(test)]
mod tests {
  use super::*;

  fn test<C, O, R>(create: C, open: O) -> anyhow::Result<()>
  where
    C: Fn(&path::Path, &mut Database<Attached>) -> anyhow::Result<R>,
    O: Fn(&path::Path, &mut Database<Attached>) -> anyhow::Result<()>,
  {
    let directory = tempfile::tempdir()?;
    let path = directory.path();
    create(
      path,
      &mut Database::<Detached>::create(&path, "test")?.attach(&path)?,
    )?;
    open(
      path,
      &mut Database::<Detached>::open(Some(&path), "test")?.attach(&path)?,
    )?;
    Ok(())
  }

  fn email(path: &path::Path, name: &str, id: &str) -> anyhow::Result<path::PathBuf> {
    let path = path.join("cur");
    fs::create_dir_all(&path)?;
    let path = path.join(name);
    let mut file = fs::File::create(&path)?;
    file.write_all(
      format!(
        "From: test
To: test
Subject: test
Message-ID: {id}"
      )
      .as_bytes(),
    )?;
    file.sync_all()?;
    Ok(path)
  }

  #[test]
  fn simple() -> anyhow::Result<()> {
    test(
      |path, database| -> _ {
        let tags = collections::HashSet::from(["tag1", "tag2"]);
        let mut message = database.add(&email(path, "test1", "id1")?)?;
        message.update_mailbox_properties("INBOX", 0, 1, 2, &tags)?;
        message.tags_to_maildir_flags()?;
        let mut message = database.add(&email(path, "test2", "id2")?)?;
        message.update_mailbox_properties("INBOX", 0, 2, 3, &tags)?;
        message.tags_to_maildir_flags()?;
        Ok(())
      },
      |path, database| -> _ {
        let mut found = 0;
        let mut messages = database.query(&format!(
          "    tag:tag1 \
           and property:test.0.marker={MESSAGE_MARKER}
           and property:test.0.mailbox=INBOX \
           and property:test.0.INBOX.uidvalidity=0 \
           and property:test.0.INBOX.uid=1 \
           and property:test.0.INBOX.modseq=2 \
           and property:test.0.INBOX.tag=tag1",
        ))?;
        while let Some(message) = messages.next() {
          assert_eq!(
            path.join("cur").join("test1:2,S"),
            message.inner.paths()?.into_iter().next().unwrap()
          );
          found += 1;
        }
        assert_eq!(1, found);
        Ok(())
      },
    )
  }

  #[test]
  #[should_panic(expected = "nested transactions aren't supported")]
  fn nested_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::<Detached>::create(&directory.path(), "test").unwrap();
    database
      .transaction(|database| database.transaction(|_| Ok(())))
      .unwrap();
  }

  #[test]
  fn transaction() -> anyhow::Result<()> {
    test(
      |path, database| -> _ {
        match database.transaction(|database| -> anyhow::Result<(), _> {
          let mut message = database.add(&email(path, "uncommited", "uncommited")?)?;
          message.update_mailbox_properties("INBOX", 0, 1, 2, &collections::HashSet::new())?;
          anyhow::bail!("uncommitted");
        }) {
          Ok(_) => unreachable!(),
          Err(error) => assert_eq!("uncommitted", error.root_cause().to_string()),
        };
        database.transaction(|database| -> _ {
          let mut message = database.add(&email(path, "commited", "commited")?)?;
          message.update_mailbox_properties("INBOX", 0, 2, 3, &collections::HashSet::new())?;
          Ok(())
        })?;
        Ok(())
      },
      |_, database| -> _ {
        let mut found = 0;
        let mut messages = database.query(&format!(
          "property:test.0.marker={MESSAGE_MARKER} and mid:/.*/"
        ))?;
        while let Some(message) = messages.next() {
          assert_eq!("commited", message.inner.id()?);
          found += 1;
        }
        assert_eq!(1, found);
        Ok(())
      },
    )
  }
}
