use std::{thread, time};
use test_log::test;

mod common;

// The ingestion of a 1.8GB maildir with 40k emails served locally by Dovecot takes roughly 4m and
// results in a 0.5GB Notmuch database. A flamegraph tells me CPU is dominated by Xapian. Indexing
// the same maildir with Notmuch directly takes roughly 1m and results in a 0.5GB Notmuch database.
#[test]
#[ignore = "requires a local maildir to benchmark against (/tmp/maildir)"]
fn maildir() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let runner = runner.with_user("maildir");
    runner.run(sin::Mode::Pull)
  })
}

#[test]
#[ignore = "spins up a server"]
fn server() {
  common::setup(common::dovecot::server, |runner| -> _ {
    let server_inbox = runner.server_maildir("INBOX", &None)?;
    server_inbox.cur(common::email("test").as_bytes())?;

    thread::sleep(time::Duration::from_secs(1800));
    panic!()
  })
}
