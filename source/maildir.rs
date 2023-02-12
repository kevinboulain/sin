// https://cr.yp.to/proto/maildir.html - Maildir
// https://www.courier-mta.org/imap/README.maildirquota.html - Maildir++
// https://doc.dovecot.org/admin_manual/mailbox_formats/maildir/ - Maildir Mailbox Format
//
// The maildir crate isn't suitable because emails need to stay in the 'tmp' directory until they're
// commited into the Notmuch database (otherwise a 'notmuch new' could pick them up after an
// interruption).

use anyhow::Context as _;
use std::{
  fs,
  io::{self, Write as _},
  path,
};

#[derive(Debug)]
pub struct Builder {
  path: path::PathBuf,
}

#[derive(Debug)]
pub struct Maildir {
  path: path::PathBuf,
  root: bool,
}

impl Builder {
  pub fn new(path: &path::Path) -> io::Result<Self> {
    fs::create_dir_all(path)?;
    Ok(Self {
      path: path.to_path_buf(),
    })
  }

  pub fn path(&self) -> &path::Path {
    self.path.as_path()
  }

  pub fn maildir(&self, mailbox: &str, separator: &Option<char>) -> io::Result<Maildir> {
    // TODO: escape the mailbox (e.g.: is / authorized)?
    let (path, root) = if mailbox == "INBOX" {
      // https://doc.dovecot.org/admin_manual/mailbox_formats/maildir/#directory-structure
      // ~/Maildir/new, ~/Maildir/cur and ~/Maildir/tmp directories contain the messages for INBOX.
      (self.path.clone(), true)
    } else if let Some(separator) = separator {
      // https://www.courier-mta.org/imap/README.maildirquota.html
      // Can folders have subfolders, defined in a recursive fashion? The answer is no. If you want
      // to have a client with a hierarchy of folders, emulate it. Pick a hierarchy separator
      // character, say ":". Then, folder foo/bar is subdirectory .foo:bar.
      //
      // https://doc.dovecot.org/admin_manual/mailbox_formats/maildir/#directory-structure
      // ~/Maildir/.folder.subfolder/ is a subfolder of a folder (i.e. folder/subfolder).
      let mut directory = ".".to_string();
      // .intersperse() is nightly...
      let n = mailbox.matches(*separator).count();
      for (i, subdirectory) in mailbox.split(*separator).enumerate() {
        directory += subdirectory;
        if i < n {
          directory.push('.');
        }
      }
      (self.path.join(directory), false)
    } else {
      // https://doc.dovecot.org/admin_manual/mailbox_formats/maildir/#directory-structure
      // ~/Maildir/.folder/ is a mailbox folder.
      (self.path.join(format!(".{mailbox}")), false)
    };
    Maildir::new(path, root)
  }
}

impl Maildir {
  // Making this function pure (by deferring the setup) is more trouble than it's worth.
  fn new(path: path::PathBuf, root: bool) -> io::Result<Self> {
    fs::create_dir_all(&path)?;
    let path = path.canonicalize()?;
    for directory in &["cur", "new", "tmp"] {
      fs::create_dir_all(path.join(directory))?;
    }
    if !root {
      // https://www.courier-mta.org/imap/README.maildirquota.html
      // Within each subdirectory there's an empty file, maildirfolder. Its existence tells the mail
      // delivery agent that this Maildir is a really a folder underneath a parent Maildir++.
      fs::File::create(path.join("maildirfolder"))?;
    }
    Ok(Self { path, root })
  }

  pub fn remove(self) -> io::Result<()> {
    fs::remove_dir_all(self.path)
  }

  pub fn root(&self) -> bool {
    self.root
  }

  pub fn path(&self) -> &path::Path {
    self.path.as_path()
  }

  pub fn has(&self, path: &path::Path) -> bool {
    let parent = path.parent().expect("invalid email");
    self.path.join("cur") == parent
      || self.path.join("new") == parent
      || self.path.join("tmp") == parent
  }

  pub fn tmp_named_with_size(&self, name: &str, size: u64) -> io::Result<Option<path::PathBuf>> {
    let path = self.path.join("tmp").join(name);
    match fs::metadata(&path) {
      Ok(metadata) if metadata.len() == size => Ok(Some(path)),
      Ok(_) => Ok(None),
      Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
      Err(error) => Err(error),
    }
  }

  pub fn tmp_named(&self, name: &str, buffer: &[u8]) -> io::Result<path::PathBuf> {
    // Do not append ':2,' otherwise Notmuch will consider this mail as processed and always move it
    // from new to cur.
    let path = self.path.join("tmp").join(name);
    let mut file = fs::File::create(&path)?;
    file.write_all(buffer)?;
    file.sync_all()?;
    Ok(path)
  }

  pub fn tmp(&self, buffer: &[u8]) -> io::Result<path::PathBuf> {
    // https://cr.yp.to/proto/maildir.html
    // Unless you're writing messages to a maildir, the format of a unique name is none of your
    // business. A unique name can be anything that doesn't contain a colon (or slash) and doesn't
    // start with a dot. Do not try to extract information from unique names.
    //
    // 'Break' the 'standard' and just use an UUID (IDs should never be parsed) whenever the name
    // wasn't explicitly given.
    self.tmp_named(
      // Ideally we'd use UUIDv7 (for the timestamp) but the uuid crate consider them unstable.
      &uuid::Uuid::new_v4().hyphenated().to_string(),
      buffer,
    )
  }

  // Should only be used in integration tests (hence, no #[cfg(test)]).
  pub fn cur(&self, buffer: &[u8]) -> io::Result<path::PathBuf> {
    let tmp = self.tmp(buffer)?;
    let cur = self.path.join("cur").join(tmp.file_name().unwrap());
    fs::rename(&tmp, &cur)?;
    Ok(cur)
  }
}

pub fn components(path: &path::Path) -> anyhow::Result<[&path::Path; 3]> {
  let parent = path
    .parent()
    .with_context(|| format!("{path:?} is without a parent"))?;
  let grandparent = parent
    .parent()
    .with_context(|| format!("{path:?} is without a grandparent"))?;
  Ok([grandparent, parent, path])
}

pub fn components_to_str<'a>(directories: &[&'a path::Path; 3]) -> anyhow::Result<[&'a str; 3]> {
  let [grandparent, parent, file] = directories;
  let file_name = file
    .file_name()
    .with_context(|| format!("{parent:?} is without a file name"))?;
  let parent_name = parent
    .file_name()
    .with_context(|| format!("{parent:?} is without a file name"))?;
  let grandparent_name = grandparent
    .file_name()
    .with_context(|| format!("{grandparent:?} is without a a file name"))?;
  Ok([
    grandparent_name
      .to_str()
      .with_context(|| format!("couldn't convert {grandparent_name:?} to string"))?,
    parent_name
      .to_str()
      .with_context(|| format!("couldn't convert {parent_name:?} to string"))?,
    file_name
      .to_str()
      .with_context(|| format!("couldn't convert {file_name:?} to string"))?,
  ])
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn inbox() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let directory = directory.path();
    let maildir = Builder::new(&directory)?.maildir("INBOX", &None)?;
    assert_eq!(directory, maildir.path);
    assert_eq!(true, maildir.root);
    Ok(())
  }

  #[test]
  fn no_separator() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let directory = directory.path();
    let maildir = Builder::new(&directory)?.maildir("folder", &None)?;
    assert_eq!(directory.join(".folder"), maildir.path);
    assert_eq!(false, maildir.root);
    Ok(())
  }

  #[test]
  fn separator() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let directory = directory.path();
    let builder = Builder::new(&directory)?;

    for separator in &['.', '/'] {
      let maildir = builder.maildir("folder", &Some(*separator))?;
      assert_eq!(directory.join(".folder"), maildir.path);
      assert_eq!(false, maildir.root);

      let maildir = builder.maildir(&format!("folder{separator}subfolder"), &Some(*separator))?;
      assert_eq!(directory.join(".folder.subfolder"), maildir.path);
      assert_eq!(false, maildir.root);
    }

    Ok(())
  }

  #[test]
  fn components() -> anyhow::Result<()> {
    let components = super::components(&path::Path::new("/maildir/cur/test"))?;
    assert_eq!(
      [
        path::Path::new("/maildir"),
        path::Path::new("/maildir/cur"),
        path::Path::new("/maildir/cur/test")
      ],
      components
    );
    assert_eq!(["maildir", "cur", "test"], components_to_str(&components)?);

    let components = super::components(&path::Path::new("/maildir/.folder/new/test"))?;
    assert_eq!(
      [
        path::Path::new("/maildir/.folder"),
        path::Path::new("/maildir/.folder/new"),
        path::Path::new("/maildir/.folder/new/test"),
      ],
      components
    );
    assert_eq!([".folder", "new", "test"], components_to_str(&components)?);

    Ok(())
  }
}
