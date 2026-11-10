//! Ingest across rewrites of a dsh session file: a tail shrink after a
//! crash, a new file after a format bump, and a zstd frame cut in the
//! middle. Synthetic files only.

use super::*;
use crate::horse::rewrite_check::{assert_no_loss, nodes};
use crate::horse::store::Store;

const ID: &str = "session-7c1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f";

fn header() -> Header {
    Header {
        id: ID.into(),
        created_at: 1_793_000_000_000,
        cwd: Some("/home/demo/project".into()),
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: 0,
        agent_preset: None,
    }
}

fn row(seq: usize, ty: &str, text: &str) -> Vec<u8> {
    format!(
        r#"{{"type":"{ty}","seq":{seq},"time":{},"data":{{"text":"{text}"}}}}"#,
        1_793_000_000_000u64 + seq as u64
    )
    .into_bytes()
}

fn rows(n: usize, text: &str) -> Vec<Vec<u8>> {
    (0..n).map(|i| row(i, "turn/note", text)).collect()
}

/// A file with the header frame, then one frame per batch of rows.
fn file(batches: &[&[Vec<u8>]]) -> Vec<u8> {
    let mut out = zframe::encode(&[header().encode(), b"\n".to_vec()].concat()).unwrap();
    for b in batches {
        let body: Vec<u8> = b
            .iter()
            .flat_map(|r| [r.as_slice(), b"\n"].concat())
            .collect();
        out.extend(zframe::encode(&body).unwrap());
    }
    out
}

fn setup(t: &Path, content: &[u8]) -> (Store, PathBuf) {
    let dir = session_dir(&t.join("sessions"), header().cwd.as_deref(), ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE_ZSTD), content).unwrap();
    (Store::open(&t.join("horse")).unwrap(), dir)
}

#[test]
fn a_tail_shrink_after_a_crash_loses_no_node() {
    let t = tempfile::tempdir().unwrap();
    let all = rows(8, "first");
    let (st, dir) = setup(t.path(), &file(&[&all[..5], &all[5..]]));
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);
    assert_eq!(before.len(), 8);

    // The last batch did not survive the crash: the file shrinks, and dsh
    // writes other rows under the same seq numbers.
    let mut next = all[..5].to_vec();
    next.extend((5..7).map(|i| row(i, "turn/note", "after the crash")));
    std::fs::write(dir.join(FILE_ZSTD), file(&[&next[..5], &next[5..]])).unwrap();
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!((r.kept, r.added, r.dropped), (5, 2, 3));
    assert_no_loss(&st, &first.session, &before);
    let chain = nodes(&st, &first.session);
    assert_eq!(chain.len(), 7);
    assert_eq!(chain[..5], before[..5]);

    // The same shrunk file read again changes nothing.
    let again = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!((again.kept, again.added, again.dropped), (7, 0, 0));
    assert_no_loss(&st, &first.session, &chain);
}

#[test]
fn a_new_file_after_a_format_bump_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let all = rows(4, "v4");
    let (st, dir) = setup(t.path(), &file(&[&all]));
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A migration publishes session.v5 next to the old file, which stays.
    let v5 = dir.join("session.v5.jsonl.zstd");
    std::fs::write(&v5, file(&[&rows(5, "v4")])).unwrap();
    let err = ingest(&st, &dir, VERSION, None).unwrap_err().to_string();
    assert!(err.contains("format 5"), "{err}");
    let err = ingest(&st, &v5, VERSION, None).unwrap_err().to_string();
    assert!(err.contains("format 4"), "{err}");
    assert_eq!(nodes(&st, &first.session), before);
    assert_no_loss(&st, &first.session, &before);

    // The old generation itself still reads, with no new node.
    let old = ingest(&st, &dir.join(FILE_ZSTD), VERSION, None).unwrap();
    assert_eq!((old.kept, old.added, old.dropped), (4, 0, 0));
}

#[test]
fn a_zstd_frame_cut_in_the_middle_waits_for_the_repair() {
    let t = tempfile::tempdir().unwrap();
    let all = rows(9, "framed");
    let (st, dir) = setup(t.path(), &file(&[&all[..3], &all[3..6]]));
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A batch frame cut in the middle of its block.
    let whole = file(&[&all[..3], &all[3..6], &all[6..]]);
    let base = file(&[&all[..3], &all[3..6]]).len();
    let cut = base + (whole.len() - base) / 2;
    std::fs::write(dir.join(FILE_ZSTD), &whole[..cut]).unwrap();
    let torn = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(torn.torn);
    assert_eq!((torn.kept, torn.added, torn.dropped), (6, 0, 0));
    assert_eq!(nodes(&st, &first.session), before);

    // dsh repairs the tail on its next open: the recovered rows come back
    // in a new frame, and later rows follow.
    std::fs::write(
        dir.join(FILE_ZSTD),
        file(&[&all[..3], &all[3..6], &all[6..8], &all[8..]]),
    )
    .unwrap();
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!((r.torn, r.kept, r.added, r.dropped), (false, 6, 3, 0));
    assert_no_loss(&st, &first.session, &before);
    assert_eq!(nodes(&st, &first.session).len(), 9);
}
