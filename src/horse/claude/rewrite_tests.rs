//! Ingest across rewrites of a Claude Code transcript: a compaction rewrite
//! of a big file, a rename, and a torn last line. Synthetic files only.

use super::*;
use crate::horse::rewrite_check::{assert_no_loss, has_key, nodes};
use crate::horse::store::Store;
use std::os::unix::fs::MetadataExt;

const CWD: &str = "/home/demo/project";
const P: &str = "5d2c7e10-3b4a-4c8d-9e6f-1a2b3c4d5e6f";

fn chain(ty: &str, uuid: &str, parent: Option<&str>, text: &str) -> Vec<u8> {
    let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
    let role = if ty == "assistant" {
        "assistant"
    } else {
        "user"
    };
    format!(
        r#"{{"parentUuid":{parent},"isSidechain":false,"type":"{ty}","message":{{"role":"{role}","content":"{text}"}},"uuid":"{uuid}","timestamp":"2026-11-04T10:00:00.000Z","userType":"external","entrypoint":"cli","cwd":"{CWD}","sessionId":"{P}","version":"{VERSION}"}}"#
    )
    .into_bytes()
}

fn last_prompt(leaf: &str) -> Vec<u8> {
    format!(r#"{{"type":"last-prompt","lastPrompt":"x","leafUuid":"{leaf}","sessionId":"{P}"}}"#)
        .into_bytes()
}

/// Two turns and a `last-prompt` row.
fn rows() -> Vec<Vec<u8>> {
    vec![
        chain("user", "u1", None, "Read notes.txt."),
        chain("assistant", "a1", Some("u1"), "It says: the store works."),
        last_prompt("a1"),
        chain("user", "u2", Some("a1"), "Count its words."),
        chain("assistant", "a2", Some("u2"), "Three words."),
        last_prompt("a2"),
    ]
}

fn bytes(rows: &[Vec<u8>]) -> Vec<u8> {
    rows.iter()
        .flat_map(|r| [r.as_slice(), b"\n"].concat())
        .collect()
}

fn write(t: &Path, rows: &[Vec<u8>]) -> PathBuf {
    let path = transcript_path(&t.join("claude"), CWD, P);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes(rows)).unwrap();
    path
}

/// Write `content` to a temporary file and rename it over `path`, as
/// Claude Code does: the inode changes.
fn rename_over(path: &Path, content: &[u8]) {
    let ino = std::fs::metadata(path).unwrap().ino();
    let tmp = path.with_extension("jsonl.compact.tmp.1a2b3c4d");
    std::fs::write(&tmp, content).unwrap();
    std::fs::rename(&tmp, path).unwrap();
    assert_ne!(std::fs::metadata(path).unwrap().ino(), ino);
}

fn store(t: &Path) -> Store {
    Store::open(&t.join("horse")).unwrap()
}

#[test]
fn a_rewrite_of_a_transcript_over_5_mib_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let pad = "y".repeat(128 * 1024);
    let mut rows = rows();
    let mut prev = "a2".to_string();
    for i in 0..44 {
        let uuid = format!("tool{i}");
        rows.push(chain("user", &uuid, Some(&prev), &pad));
        prev = uuid;
    }
    let boundary = format!(
        r#"{{"parentUuid":null,"logicalParentUuid":"{prev}","isSidechain":false,"type":"system","subtype":"compact_boundary","content":"Conversation compacted","uuid":"cb","timestamp":"2026-11-04T11:00:00.000Z","cwd":"{CWD}","sessionId":"{P}","version":"{VERSION}"}}"#
    )
    .into_bytes();
    let summary = chain("user", "sum", Some("cb"), "Summary: the store works.");
    rows.extend([boundary.clone(), summary.clone(), last_prompt("sum")]);
    let file = write(t.path(), &rows);
    assert!(std::fs::metadata(&file).unwrap().len() >= 5 * 1024 * 1024);
    let st = store(t.path());
    let first = ingest(&st, &file, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // The rewrite keeps the rows from the boundary and goes on with a turn.
    let after = vec![
        boundary,
        summary,
        last_prompt("sum"),
        chain("assistant", "a3", Some("sum"), "Done."),
    ];
    rename_over(&file, &bytes(&after));
    let r = ingest(&st, &file, VERSION, None).unwrap();
    assert!(r.rewritten);
    assert_eq!((r.session.as_str(), r.kept), (first.session.as_str(), 2));
    assert_no_loss(&st, &first.session, &before);
    let chain = nodes(&st, &first.session);
    for n in &before {
        assert!(
            chain.iter().any(|c| c.hash == n.hash),
            "dropped from the session"
        );
    }
    assert!(has_key(&chain, "uuid:a3"));

    // The same rewrite read again adds nothing.
    let again = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((again.added, again.rewritten), (0, false));
    assert_no_loss(&st, &first.session, &chain);
}

#[test]
fn a_rename_of_the_transcript_is_an_append() {
    let t = tempfile::tempdir().unwrap();
    let rows = rows();
    let file = write(t.path(), &rows);
    let st = store(t.path());
    let first = ingest(&st, &file, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // The same rows and a new one, in a new file renamed over the old one.
    let mut more = rows.clone();
    more.push(chain("user", "u3", Some("a2"), "Thanks."));
    rename_over(&file, &bytes(&more));
    let r = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((r.rewritten, r.kept, r.added), (false, rows.len(), 1));
    assert_no_loss(&st, &first.session, &before);

    // The transcript moved to another project folder keeps its session.
    let moved = transcript_path(&t.path().join("claude"), "/home/demo/moved", P);
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&file, &moved).unwrap();
    let m = ingest(&st, &moved, VERSION, None).unwrap();
    assert_eq!(
        (m.session, m.added, m.rewritten),
        (first.session.clone(), 0, false)
    );
    assert_no_loss(&st, &first.session, &nodes(&st, &first.session));
}

#[test]
fn a_torn_last_line_is_read_once_when_it_completes() {
    let t = tempfile::tempdir().unwrap();
    let rows = rows();
    let file = write(t.path(), &rows);
    let st = store(t.path());
    let first = ingest(&st, &file, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    let next = chain("user", "u3", Some("a2"), "Thanks.");
    let mut b = bytes(&rows);
    b.extend_from_slice(&next[..next.len() / 2]);
    std::fs::write(&file, &b).unwrap();
    let torn = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((torn.torn, torn.added), (true, 0));

    b.extend_from_slice(&next[next.len() / 2..]);
    b.push(b'\n');
    std::fs::write(&file, &b).unwrap();
    let done = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((done.torn, done.added), (false, 1));
    assert_eq!(ingest(&st, &file, VERSION, None).unwrap().added, 0);
    assert_no_loss(&st, &first.session, &before);
}

#[test]
fn a_torn_line_that_a_crash_left_before_new_rows_is_skipped() {
    let t = tempfile::tempdir().unwrap();
    let rows = rows();
    let file = write(t.path(), &rows);
    let st = store(t.path());
    let first = ingest(&st, &file, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A crash cut the last line; the resumed process ends it and appends.
    let lost = chain("user", "u3", Some("a2"), "Thanks.");
    let mut b = bytes(&rows);
    b.extend_from_slice(&lost[..lost.len() / 2]);
    b.push(b'\n');
    let next = chain("user", "u4", Some("a2"), "Thanks again.");
    b.extend_from_slice(&next);
    b.push(b'\n');
    std::fs::write(&file, &b).unwrap();
    let r = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((r.rewritten, r.added), (false, 1));
    let chain = nodes(&st, &first.session);
    assert!(has_key(&chain, "uuid:u4") && !has_key(&chain, "uuid:u3"));
    assert_no_loss(&st, &first.session, &before);
    assert_eq!(ingest(&st, &file, VERSION, None).unwrap().added, 0);
}
