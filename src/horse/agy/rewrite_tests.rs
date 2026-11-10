//! Ingest across rewrites of an agy conversation database: a step row
//! updated at the same index, a truncation, and a WAL file that is not yet
//! checkpointed. Synthetic databases only.

use super::fixture::Fixture;
use super::*;
use crate::horse::rewrite_check::{assert_no_loss, nodes};
use rusqlite::Connection;

fn store(t: &Path) -> Store {
    Store::open(&t.join("store")).unwrap()
}

fn count(chain: &[Node], key: &str) -> usize {
    chain
        .iter()
        .filter(|n| n.meta.key.as_deref() == Some(key))
        .count()
}

#[test]
fn a_step_row_update_at_the_same_index_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = store(t.path());
    let mut f = Fixture::two_turns();
    f.running();
    let src = f.write(t.path());
    let first = ingest(&st, &src, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // agy finishes the running step in place.
    Fixture::two_turns().write(t.path());
    let r = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((r.updated, r.dropped), (1, 0));
    assert_no_loss(&st, &first.session, &before);
    let chain = nodes(&st, &first.session);
    assert_eq!(count(&chain, "steps:5"), 2);
    assert_eq!(replay(&chain).unwrap(), db::read(&src).unwrap().rows);
    assert_eq!(ingest(&st, &src, VERSION, None).unwrap().added, 0);
}

#[test]
fn a_truncation_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = store(t.path());
    let mut f = Fixture::two_turns();
    let src = f.write(t.path());
    let first = ingest(&st, &src, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A rewind truncates the steps from index 4.
    f.truncate(4);
    f.write(t.path());
    let r = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((r.added, r.dropped), (0, 3));
    assert_no_loss(&st, &first.session, &before);
    let chain = nodes(&st, &first.session);
    for n in &before {
        assert!(chain.iter().any(|c| c.hash == n.hash), "left the session");
    }
    assert_eq!(replay(&chain).unwrap(), db::read(&src).unwrap().rows);

    // A new turn at the same indexes: new nodes, the old ones stay.
    f.user("Try another way").reply("Done another way.");
    f.write(t.path());
    let n = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((n.added, n.dropped), (3, 0));
    assert_no_loss(&st, &first.session, &chain);
    assert_eq!(count(&nodes(&st, &first.session), "steps:5"), 2);
}

#[test]
fn a_wal_file_that_is_not_yet_checkpointed_is_read() {
    let t = tempfile::tempdir().unwrap();
    let st = store(t.path());
    let mut f = Fixture::two_turns();
    f.running();
    let src = f.write(t.path());
    let first = ingest(&st, &src, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // agy holds the database open and writes the finished step into the
    // WAL file without a checkpoint.
    let done = Fixture::two_turns().write(&t.path().join("other"));
    let agy = Connection::open(&src).unwrap();
    agy.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
    agy.execute("ATTACH ?1 AS o", [done.to_str().unwrap()])
        .unwrap();
    agy.execute_batch(
        "BEGIN; DELETE FROM steps; INSERT INTO steps SELECT * FROM o.steps;
         DELETE FROM gen_metadata; INSERT INTO gen_metadata SELECT * FROM o.gen_metadata;
         COMMIT; DETACH o;",
    )
    .unwrap();
    let wal = src.with_extension("db-wal");
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    let r = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((r.updated, r.dropped), (1, 0));
    assert_no_loss(&st, &first.session, &before);
    let chain = nodes(&st, &first.session);
    assert_eq!(replay(&chain).unwrap(), db::read(&done).unwrap().rows);

    // agy dies without a checkpoint: the WAL file stays, and a read again
    // finds the same rows.
    std::mem::forget(agy);
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    let again = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((again.added, again.updated), (0, 0));
    assert_no_loss(&st, &first.session, &chain);
}
