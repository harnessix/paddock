//! Ingest across rewrites of a Grok Build session folder: a rewrite of
//! `chat_history.jsonl` by a rename, a rewind marker, a compaction
//! checkpoint, and a torn line that Grok heals. Synthetic files only.

use super::fixture::Fixture;
use super::*;
use crate::horse::rewrite_check::{assert_no_loss, nodes};
use serde_json::json;
use std::os::unix::fs::MetadataExt;

/// Write the folder; `chat_history.jsonl` goes in by a rename, as Grok
/// rewrites it.
fn write_by_rename(f: &Fixture, home: &Path) -> PathBuf {
    let dir = session_dir(home, &f.cwd, &f.id);
    let chat = dir.join(CHAT_FILE);
    let ino = std::fs::metadata(&chat).map(|m| m.ino()).ok();
    let content: String = f.chat.iter().map(|l| format!("{l}\n")).collect();
    let tmp = dir.join("chat_history.jsonl.sync.tmp");
    f.write(home);
    std::fs::write(&tmp, content).unwrap();
    std::fs::rename(&tmp, &chat).unwrap();
    assert_ne!(std::fs::metadata(&chat).map(|m| m.ino()).ok(), ino);
    dir
}

/// The files of the session as the store replays them equal the folder.
fn assert_replays(st: &Store, session: &str, dir: &Path) {
    let files = replay::replay(&nodes(st, session)).unwrap();
    assert_eq!(files.chat, read_lines(&dir.join(CHAT_FILE)).unwrap().0);
}

#[test]
fn a_chat_history_rewrite_by_rename_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    let dir = f.write(t.path());
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A spawn or resume writes the same items again by a rename.
    write_by_rename(&f, t.path());
    let same = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!((same.added, same.rewritten), (0, false));

    // An integrity repair drops one item in the middle.
    f.chat.remove(2);
    write_by_rename(&f, t.path());
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(r.rewritten);
    assert_eq!(r.added, 1, "only the index node");
    assert_no_loss(&st, &first.session, &before);
    assert_replays(&st, &first.session, &dir);
}

#[test]
fn a_rewind_marker_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    let dir = f.write(t.path());
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A rewind appends a marker and cuts the chat history by a rename.
    f.rewind(1);
    write_by_rename(&f, t.path());
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(r.rewritten);
    assert_no_loss(&st, &first.session, &before);
    assert_replays(&st, &first.session, &dir);

    // The next turn after the rewind is an append again.
    f.prompt(1, "Fix the warning another way").reply("Done.");
    f.write(t.path());
    let next = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(!next.rewritten);
    assert_no_loss(&st, &first.session, &before);
    assert_replays(&st, &first.session, &dir);
}

#[test]
fn a_compaction_checkpoint_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    let dir = f.write(t.path());
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A compaction appends a checkpoint row and rewrites the chat history.
    f.update(
        true,
        json!({"sessionUpdate": "compaction_checkpoint",
               "checkpoint_file": "compaction_checkpoints/0001.json"}),
    );
    f.compact("Summary: the tests pass and the warning is fixed.");
    write_by_rename(&f, t.path());
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(r.rewritten);
    assert_eq!(r.added, 3, "the checkpoint, the index node and the summary");
    assert_no_loss(&st, &first.session, &before);
    assert_replays(&st, &first.session, &dir);
}

#[test]
fn a_torn_line_that_grok_heals_is_skipped() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    let dir = f.write(t.path());
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A crash cut a line. The next append writes a newline first, and Grok
    // skips the broken line when it reads.
    let full = std::fs::read_to_string(dir.join(UPDATES_FILE)).unwrap();
    f.prompt(2, "Lost in the crash");
    let lost = f.updates.pop().unwrap();
    f.prompt(2, "One more");
    let next = f.updates.last().unwrap().clone();
    std::fs::write(
        dir.join(UPDATES_FILE),
        format!("{full}{}\n{next}\n", &lost[..lost.len() / 2]),
    )
    .unwrap();
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!(r.added, 1);
    assert_no_loss(&st, &first.session, &before);
    assert_eq!(ingest(&st, &dir, VERSION, None).unwrap().added, 0);
    let files = replay::replay(&nodes(&st, &first.session)).unwrap();
    assert_eq!(files.updates.last().unwrap(), next.as_bytes());
}
