use std::{fs, path};
use test_log::test;

mod common;

fn successful_move_pre_commit_begin(runner: &common::Runner) -> anyhow::Result<()> {
  let server_inbox = runner.server_maildir("INBOX", &None)?;
  let server_folder = runner.server_maildir("folder", &None)?;

  runner.run(sin::Mode::Pull)?;

  let client_inbox = runner.client_maildir("INBOX", &None)?;
  let path = client_inbox.cur(common::email("test").as_bytes())?;
  let client_folder = runner.client_maildir("folder", &None)?;

  runner.notmuch_new()?;

  runner.run(sin::Mode::Push)?;

  let moved_path = client_folder
    .path()
    .join("cur")
    .join(path.file_name().unwrap());
  fs::rename(path, moved_path)?;

  runner.notmuch_new()?;

  runner
    .with_interruption(sin::Interruption::SuccessfulMovePreCommit)
    .run(sin::Mode::Push)?;

  // Locally and remotely, the message has been moved from the inbox to the folder.
  assert_eq!((0, 0, 0), runner.maildir_count(&client_inbox)?);
  assert_eq!((1, 0, 0), runner.maildir_count(&client_folder)?);
  assert_eq!((0, 0, 0), runner.maildir_count(&server_inbox)?);
  assert_eq!((1, 0, 0), runner.maildir_count(&server_folder)?);

  // But the current state doesn't agree.
  assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.folder.highestmodseq=1 sin.folder.separator=%2f sin.folder.uidvalidity=<omitted> sin.lastmod=4 sin.mailbox=INBOX sin.mailbox=folder sin.marker=root
+inbox +unread -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.tag=inbox sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

  Ok(())
}

fn successful_move_pre_commit_end(runner: &common::Runner) -> anyhow::Result<()> {
  runner.run(sin::Mode::Pull)?;

  // No more inbox.
  assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=4 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.folder.highestmodseq=3 sin.folder.separator=%2f sin.folder.uidvalidity=<omitted> sin.lastmod=4 sin.mailbox=INBOX sin.mailbox=folder sin.marker=root
+inbox +unread -- id:test
#= test sin.0.folder.modseq=3 sin.0.folder.tag=inbox sin.0.folder.tag=unread sin.0.folder.uid=1 sin.0.folder.uidvalidity=<omitted> sin.0.mailbox=folder sin.0.marker=message
", runner.notmuch_dump()?);

  Ok(())
}

#[test]
fn successful_move_pre_commit_push() {
  common::setup(common::dovecot::server, |runner| -> _ {
    successful_move_pre_commit_begin(runner)?;

    // Repushing is invalid since I'm not sure there's a correct way to get out of this situation.
    let error = runner.run(sin::Mode::Push).unwrap_err();
    assert_eq!(
      "message test couldn't be moved to folder, assuming previously interrupted, rerun a pull",
      error.root_cause().to_string()
    );

    successful_move_pre_commit_end(runner)
  })
}

#[test]
fn successful_move_pre_commit_pull() {
  common::setup(common::dovecot::server, |runner| -> _ {
    successful_move_pre_commit_begin(runner)?;
    successful_move_pre_commit_end(runner)
  })
}

#[test]
fn move_out_of_tmp_post_rename() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    server_inbox.cur(common::email("test").as_bytes())?;

    runner
      .with_interruption(sin::Interruption::MoveOutOfTmpPostRename)
      .run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);

    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

fn append_is_not_transactional_begin(runner: &common::Runner) -> anyhow::Result<()> {
  runner.run(sin::Mode::Pull)?;

  let client_inbox = runner.client_maildir("INBOX", &None)?;
  client_inbox.cur(common::email("test").as_bytes())?;

  runner.notmuch_new()?;

  runner
    .with_interruption(sin::Interruption::AppendIsNotTransactional)
    .run(sin::Mode::Push)?;

  Ok(())
}

#[test]
fn append_is_not_transactional_push() {
  common::setup(common::dovecot::server, |runner| -> _ {
    append_is_not_transactional_begin(runner)?;

    runner.run(sin::Mode::Push)?;

    let server_inbox = runner.server_maildir("INBOX", &None)?;
    assert_eq!((2, 0, 0), runner.maildir_count(&server_inbox)?);

    runner.run(sin::Mode::Pull)?;

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin\n#= 0@sin sin.INBOX.highestmodseq=5 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.lastmod=4 sin.mailbox=INBOX sin.marker=root
+inbox +unread -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.tag=inbox sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn append_is_not_transactional_pull() {
  common::setup(common::dovecot::server, |runner| -> _ {
    append_is_not_transactional_begin(runner)?;

    runner.run(sin::Mode::Pull)?;

    let server_inbox = runner.server_maildir("INBOX", &None)?;
    assert_eq!((1, 0, 0), runner.maildir_count(&server_inbox)?);

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin\n#= 0@sin sin.INBOX.highestmodseq=3 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+inbox +unread -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.tag=inbox sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    runner.run(sin::Mode::Push)?;

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin\n#= 0@sin sin.INBOX.highestmodseq=3 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.lastmod=6 sin.mailbox=INBOX sin.marker=root
+inbox +unread -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.tag=inbox sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn stored_flags() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    let path = server_inbox.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    runner.notmuch_tag("-unread", "mid:test")?;

    runner
      .with_interruption(sin::Interruption::StoredFlags)
      .run(sin::Mode::Push)?;

    // The server has been updated.
    assert!(path::Path::new(&format!("{}:2,S", path.to_str().unwrap())).exists());

    // But not the local cache.
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
 -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    // Repushing won't do anything since the modseq is specified.
    let error = runner.run(sin::Mode::Push).unwrap_err();
    assert_eq!(
      "message test in INBOX couldn't be updated with flags {\"\\\\Seen\"}, rerun a pull",
      error.root_cause().to_string()
    );

    runner.run(sin::Mode::Pull)?;

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=3 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
 -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}
