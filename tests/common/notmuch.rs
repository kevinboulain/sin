// --config '' means the default configuration option will still be loaded (e.g.: new.tags =
// unread;inbox).

use std::{path, process, str};

pub fn dump(database: &path::Path) -> anyhow::Result<String> {
  let regex = regex::Regex::new(".uidvalidity=(\\d+)").unwrap();
  let stdout = process::Command::new("notmuch")
    .env("NOTMUCH_DATABASE", database.as_os_str())
    .args(&["--config", "", "dump"])
    .output()?
    .stdout;
  Ok(
    regex
      .replace_all(str::from_utf8(&stdout).unwrap(), ".uidvalidity=<omitted>")
      .to_string(),
  )
}

pub fn run(database: &path::Path, arguments: &[&str]) -> anyhow::Result<()> {
  let mut arguments_ = vec!["--config", ""];
  arguments_.extend(arguments);
  let status = process::Command::new("notmuch")
    .env("NOTMUCH_DATABASE", database.as_os_str())
    .args(&arguments_[..])
    .status()?;
  assert_eq!(Some(0), status.code());
  Ok(())
}
