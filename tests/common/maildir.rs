use std::fs;

pub fn count(maildir: &sin::maildir::Maildir) -> anyhow::Result<(usize, usize, usize)> {
  let count = |directory| -> anyhow::Result<_> {
    let mut files = 0;
    for entry in fs::read_dir(directory)? {
      if entry?.path().is_file() {
        files += 1;
      }
    }
    Ok(files)
  };
  Ok((
    count(maildir.path().join("cur"))?,
    count(maildir.path().join("new"))?,
    count(maildir.path().join("tmp"))?,
  ))
}
