use std::{thread, time};
use test_log::test;

mod common;

// The ingestion of a 1.8GB maildir with 40k emails served locally by Dovecot takes roughly 4m and
// results in a 0.5GB Notmuch database. A flamegraph tells me CPU is dominated by Xapian. Indexing
// the same maildir with Notmuch directly takes roughly 1m and results in a 0.5GB Notmuch database.
//
// Make sure this is run with a sufficiently high optimization level otherwise it will show
// misleading results during profiling: https://github.com/kevinmehall/rust-peg/discussions/326
//  CARGO_PROFILE_TEST_OPT_LEVEL=3 cargo test maildir -- --include-ignored
//
// To generate a maildir from a mailbox:
//  mb2md -s path/to.mbox -d /tmp/maildir
//
// To profile:
//  perf record --call-graph dwarf cargo test --release maildir -- --include-ignored
//  perf script | stackcollapse-perf.pl > out.perf-folded
//  flamegraph.pl out.perf-folded > perf.svg
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
