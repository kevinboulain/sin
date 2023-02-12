use std::{fs, path};
use test_log::test;

mod common;

#[test]
fn lastmod() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    server_inbox.cur(common::email("test").as_bytes())?;

    // When there's nothing to synchronize, lastmod should stay the same.
    for _ in 0..3 {
      runner.run(sin::Mode::Pull)?;
      runner.run(sin::Mode::Push)?;
    }

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.lastmod=4 sin.mailbox=INBOX sin.marker=root
+unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn quoting() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let folder = "folder with spaces and \"quotes\"";
    let urlencoded_folder = "folder%20with%20spaces%20and%20%22quotes%22";

    let server_folder = runner.server_maildir(folder, &None)?;
    let path = server_folder.cur(common::email("test1").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    assert_eq!(format!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.{urlencoded_folder}.highestmodseq=2 sin.{urlencoded_folder}.separator=%2f sin.{urlencoded_folder}.uidvalidity=<omitted> sin.mailbox=INBOX sin.mailbox={urlencoded_folder} sin.marker=root
+unread -- id:test1
#= test1 sin.0.{urlencoded_folder}.modseq=2 sin.0.{urlencoded_folder}.tag=unread sin.0.{urlencoded_folder}.uid=1 sin.0.{urlencoded_folder}.uidvalidity=<omitted> sin.0.mailbox={urlencoded_folder} sin.0.marker=message
"), runner.notmuch_dump()?);

    fs::rename(
      &path,
      path::Path::new(&format!("{}:2,a", path.to_str().unwrap())),
    )?;
    runner.run(sin::Mode::Pull)?;

    runner.notmuch_tag("+test1", "mid:test1")?;
    let client_folder = runner.client_maildir(folder, &None)?;
    client_folder.cur(common::email("test2").as_bytes())?;

    runner.notmuch_new()?;
    runner.run(sin::Mode::Push)?;

    assert_eq!((2, 0, 0), runner.maildir_count(&server_folder)?);
    fs::metadata(path::Path::new(&format!("{}:2,ac", path.to_str().unwrap())))?; // 'b' for inbox.
    assert_eq!((1, 1, 0), runner.maildir_count(&client_folder)?);

    assert_eq!(format!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.{urlencoded_folder}.highestmodseq=3 sin.{urlencoded_folder}.separator=%2f sin.{urlencoded_folder}.uidvalidity=<omitted> sin.lastmod=7 sin.mailbox=INBOX sin.mailbox={urlencoded_folder} sin.marker=root
+test1 +unknown-0 +unread -- id:test1
#= test1 sin.0.{urlencoded_folder}.modseq=6 sin.0.{urlencoded_folder}.tag=test1 sin.0.{urlencoded_folder}.tag=unknown-0 sin.0.{urlencoded_folder}.tag=unread sin.0.{urlencoded_folder}.uid=1 sin.0.{urlencoded_folder}.uidvalidity=<omitted> sin.0.mailbox={urlencoded_folder} sin.0.marker=message
+inbox +unread -- id:test2
#= test2 sin.0.{urlencoded_folder}.modseq=5 sin.0.{urlencoded_folder}.tag=inbox sin.0.{urlencoded_folder}.tag=unread sin.0.{urlencoded_folder}.uid=2 sin.0.{urlencoded_folder}.uidvalidity=<omitted> sin.0.mailbox={urlencoded_folder} sin.0.marker=message
"), runner.notmuch_dump()?);

    Ok(())
  })
}
