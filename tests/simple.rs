use std::{fs, path, thread, time};
use test_log::test;

mod common;

#[test]
fn invalid_password() {
  // A simple test to show the error message provides barely enough information for debugging.
  common::setup(common::dovecot::server, |runner| -> _ {
    let runner = runner.with_password("invalid password");
    let error = runner.run(sin::Mode::Pull).unwrap_err();
    assert!(error
      .chain()
      .next()
      .unwrap()
      .to_string()
      .starts_with("NO [AUTHENTICATIONFAILED] Authentication failed.\\r\\n"));
    assert_eq!(
      "error at 0: expected \"OK\"",
      error.root_cause().to_string()
    );
    Ok(())
  })
}

#[test]
fn remote_new() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    server_inbox.cur(common::email("test").as_bytes())?;

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

#[test]
fn remote_subfolder() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_subfolder = runner.server_maildir("folder/sub", &Some('/'))?;
    server_subfolder.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    let client_subfolder = runner.client_maildir("folder/sub", &Some('/'))?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_subfolder)?);
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.folder%2fsub.highestmodseq=2 sin.folder%2fsub.separator=%2f sin.folder%2fsub.uidvalidity=<omitted> sin.mailbox=INBOX sin.mailbox=folder%2fsub sin.marker=root
+unread -- id:test
#= test sin.0.folder%2fsub.modseq=2 sin.0.folder%2fsub.tag=unread sin.0.folder%2fsub.uid=1 sin.0.folder%2fsub.uidvalidity=<omitted> sin.0.mailbox=folder%2fsub sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn remote_subfolder_separator() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let runner = runner.with_user("separator");
    let server_subfolder = runner.server_maildir("folder.sub", &Some('.'))?;
    server_subfolder.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    let client_subfolder = runner.client_maildir("folder.sub", &Some('.'))?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_subfolder)?);
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=. sin.INBOX.uidvalidity=<omitted> sin.folder.sub.highestmodseq=2 sin.folder.sub.separator=. sin.folder.sub.uidvalidity=<omitted> sin.mailbox=INBOX sin.mailbox=folder.sub sin.marker=root
+unread -- id:test
#= test sin.0.folder.sub.modseq=2 sin.0.folder.sub.tag=unread sin.0.folder.sub.uid=1 sin.0.folder.sub.uidvalidity=<omitted> sin.0.mailbox=folder.sub sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn remote_change() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    let path = server_inbox.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!(
      "#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    fs::rename(
      &path,
      path::Path::new(&format!("{}:2,a", path.to_str().unwrap())),
    )?;
    runner.run(sin::Mode::Pull)?;

    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!(
      "#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=3 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unknown-0 +unread -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.tag=unknown-0 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn remote_removal() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    let path = server_inbox.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);

    fs::remove_file(&path)?;
    runner.run(sin::Mode::Pull)?;

    assert_eq!((0, 0, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!(
      "#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=3 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
",
      runner.notmuch_dump()?
    );

    Ok(())
  })
}

#[test]
fn remote_mailbox_removal() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_folder = runner.server_maildir("folder", &Some('/'))?;
    server_folder.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.folder.highestmodseq=2 sin.folder.separator=%2f sin.folder.uidvalidity=<omitted> sin.mailbox=INBOX sin.mailbox=folder sin.marker=root
+unread -- id:test
#= test sin.0.folder.modseq=2 sin.0.folder.tag=unread sin.0.folder.uid=1 sin.0.folder.uidvalidity=<omitted> sin.0.mailbox=folder sin.0.marker=message
", runner.notmuch_dump()?);

    server_folder.remove()?;

    assert_eq!(
      runner
        .run(sin::Mode::Pull)
        .unwrap_err()
        .chain()
        .next()
        .unwrap()
        .to_string(),
      "folder has been removed on the server, allow to purge it locally (all messages will be \
       removed) by passing --purgeable folder"
    );
    runner.with_purgeable("folder").run(sin::Mode::Pull)?;

    assert_eq!(
      "#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
",
      runner.notmuch_dump()?
    );

    Ok(())
  })
}

#[test]
fn uidvalidity() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    server_inbox.cur(common::email("test1").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test1
#= test1 sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    // Dovecot would repopulate the maildir with the same uidvalidity (seconds since epoch).
    thread::sleep(time::Duration::from_secs(1));

    fs::remove_dir_all(server_inbox.path())?;
    let server_inbox = runner.server_maildir("INBOX", &None)?; // Recreate it.
    server_inbox.cur(common::email("test2").as_bytes())?;

    assert_eq!(
      runner
        .run(sin::Mode::Pull)
        .unwrap_err()
        .chain()
        .next()
        .unwrap()
        .to_string(),
      "INBOX's validity has changed on the server, allow to purge it locally (all messages will be \
       removed) by passing --purgeable INBOX"
    );

    runner.with_purgeable("INBOX").run(sin::Mode::Pull)?;

    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test2
#= test2 sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn multi_user() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let runner1 = runner.with_user("user1");
    let runner2 = runner.with_user("user2");

    for runner in [&runner1, &runner2] {
      let server_inbox = runner.server_maildir("INBOX", &None)?;
      // The same ID on purpose: to showcase emails from different accounts are properly merged.
      server_inbox.cur(common::email("test").as_bytes())?;
    }

    for runner in [&runner1, &runner2] {
      runner.run(sin::Mode::Pull)?;
      // Dovecot would populate the maildir with the same uidvalidity (seconds since epoch).
      thread::sleep(time::Duration::from_secs(1));
    }

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message sin.1.INBOX.modseq=2 sin.1.INBOX.tag=unread sin.1.INBOX.uid=1 sin.1.INBOX.uidvalidity=<omitted> sin.1.mailbox=INBOX sin.1.marker=message
+sin.internal -- id:1@sin
#= 1@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
", runner.notmuch_dump()?);

    fs::remove_dir_all(runner2.client_maildir_builder()?.path())?;
    runner.notmuch_new()?;

    let runner3 = runner.with_user("user3");
    let server_inbox = runner3.server_maildir("INBOX", &None)?;
    server_inbox.cur(common::email("test3").as_bytes())?;
    runner3.run(sin::Mode::Pull)?;

    // 1@sin has been repurposed and so sin.1.* has been removed from test.
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
+sin.internal -- id:1@sin
#= 1@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test3
#= test3 sin.1.INBOX.modseq=2 sin.1.INBOX.tag=unread sin.1.INBOX.uid=1 sin.1.INBOX.uidvalidity=<omitted> sin.1.mailbox=INBOX sin.1.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn local_new() {
  common::setup(common::dovecot::server, |runner| -> _ {
    // To update the local cache.
    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    client_inbox.cur(common::email("test").as_bytes())?;

    // To add the new email to the database.
    runner.notmuch_new()?;

    runner.run(sin::Mode::Push)?;

    let server_inbox = runner.server_maildir("INBOX", &None)?;
    assert_eq!((1, 0, 0), runner.maildir_count(&server_inbox)?);

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin\n#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.lastmod=4 sin.mailbox=INBOX sin.marker=root
+inbox +unread -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.tag=inbox sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn local_change() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    let path = server_inbox.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
+unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    runner.notmuch_tag("-unread", "mid:test")?;

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.mailbox=INBOX sin.marker=root
 -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    runner.run(sin::Mode::Push)?;

    assert!(path::Path::new(&format!("{}:2,S", path.to_str().unwrap())).exists());
    assert_eq!((1, 0, 0), runner.maildir_count(&client_inbox)?);

    // Notice how highestmodseq < modseq.
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.lastmod=6 sin.mailbox=INBOX sin.marker=root
 -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    runner.run(sin::Mode::Pull)?;

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=3 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.lastmod=6 sin.mailbox=INBOX sin.marker=root
 -- id:test
#= test sin.0.INBOX.modseq=3 sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn local_move() {
  common::setup(common::dovecot::server, |runner| -> _ {
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

    runner.run(sin::Mode::Push)?;

    assert_eq!((0, 0, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!((1, 0, 0), runner.maildir_count(&client_folder)?);
    assert_eq!((0, 0, 0), runner.maildir_count(&server_inbox)?);
    assert_eq!((1, 0, 0), runner.maildir_count(&server_folder)?);

    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=1 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.folder.highestmodseq=1 sin.folder.separator=%2f sin.folder.uidvalidity=<omitted> sin.lastmod=7 sin.mailbox=INBOX sin.mailbox=folder sin.marker=root
+inbox +unread -- id:test
#= test sin.0.folder.modseq=3 sin.0.folder.tag=inbox sin.0.folder.tag=unread sin.0.folder.uid=1 sin.0.folder.uidvalidity=<omitted> sin.0.mailbox=folder sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}

#[test]
fn remote_move_with_local_change() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    let server_folder = runner.server_maildir("folder", &None)?;
    let path = server_folder.cur(common::email("test").as_bytes())?;

    runner.run(sin::Mode::Pull)?;

    // Edit the copy locally.
    // If the pull didn't batch the removals the change would have been lost.
    runner.notmuch_tag("+tag", "mid:test")?;

    // Move the mail on the server.
    let moved_path = server_inbox
      .path()
      .join("cur")
      .join(path.file_name().unwrap());
    fs::rename(path, moved_path)?;

    runner.run(sin::Mode::Pull)?;

    let client_inbox = runner.client_maildir("INBOX", &None)?;
    assert_eq!((0, 1, 0), runner.maildir_count(&client_inbox)?);
    assert_eq!("#notmuch-dump batch-tag:3 config,properties,tags
+sin.internal -- id:0@sin
#= 0@sin sin.INBOX.highestmodseq=2 sin.INBOX.separator=%2f sin.INBOX.uidvalidity=<omitted> sin.folder.highestmodseq=3 sin.folder.separator=%2f sin.folder.uidvalidity=<omitted> sin.mailbox=INBOX sin.mailbox=folder sin.marker=root
+tag +unread -- id:test
#= test sin.0.INBOX.modseq=2 sin.0.INBOX.tag=unread sin.0.INBOX.uid=1 sin.0.INBOX.uidvalidity=<omitted> sin.0.mailbox=INBOX sin.0.marker=message
", runner.notmuch_dump()?);

    Ok(())
  })
}
