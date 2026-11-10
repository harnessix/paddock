//! The guard on Codex sessions: a swap under the same thread id keeps the
//! prefix up to the fork point, a fork under a new id is cold.

use super::super::{check, Op};
use crate::horse::codex::{self as codec, CONTAINER, HARNESS, VERSION};
use crate::horse::store::{Hash, Origin, Store};
use serde_json::json;

const ID: &str = "019a5f00-0000-7000-8000-0000000000cc";
const ORIGIN: Origin = Origin {
    harness: HARNESS,
    container: CONTAINER,
    version: VERSION,
};

fn row(ty: &str, payload: serde_json::Value) -> String {
    json!({ "timestamp": "2026-11-06T00:00:00.000Z", "type": ty, "payload": payload }).to_string()
}

fn msg(role: &str, text: &str) -> String {
    row(
        "response_item",
        json!({ "type": "message", "role": role, "content": [{ "type": "input_text", "text": text }] }),
    )
}

/// A session: meta, a turn of 400 bytes of text with a token count of 50
/// input tokens per 100 bytes, and a second turn.
fn session(t: &std::path::Path) -> (Store, String, Vec<Hash>) {
    let st = Store::open(&t.join("store")).unwrap();
    let meta = row(
        "session_meta",
        json!({ "id": ID, "cli_version": VERSION, "cwd": "/w" }),
    );
    let first = msg("user", &"a".repeat(400));
    let first_len = first.len() as u64;
    let rows = [
        meta,
        first,
        row(
            "event_msg",
            json!({ "type": "token_count", "info": { "last_token_usage": { "input_tokens": first_len / 2 } } }),
        ),
        row("event_msg", json!({ "type": "task_complete" })),
        msg("user", &"b".repeat(200)),
        row("event_msg", json!({ "type": "task_complete" })),
    ];
    let dir = t.join("home/.codex/sessions/2026/11/06");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("rollout-2026-11-06T00-00-00-{ID}.jsonl"));
    std::fs::write(&p, rows.join("\n") + "\n").unwrap();
    let sid = codec::ingest(&st, &p, VERSION, None).unwrap().session;
    let nodes = st
        .session_nodes(&st.session(&sid).unwrap())
        .unwrap()
        .into_iter()
        .map(|n| n.hash)
        .collect();
    (st, sid, nodes)
}

#[test]
fn a_rewind_under_the_same_id_keeps_the_prefix() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid, n) = session(t.path());
    let v = check(&st, &sid, &Op::Swap { to: n[3].clone() }, 1).unwrap();
    assert!(v.keeps, "{}", v.message());
    assert_eq!(v.lost_tokens, 0);
    let m = v.message();
    assert!(m.contains(&format!("thread {ID} keeps its id")), "{m}");
    assert!(m.contains("stay in the store"), "{m}");
    let head = check(&st, &sid, &Op::Swap { to: n[5].clone() }, 1).unwrap();
    assert_eq!(head.op, "a swap to the head");
    assert!(head.keeps);
}

#[test]
fn a_branch_after_the_fork_point_is_a_break_of_its_size() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid, n) = session(t.path());
    let other = msg("user", &"c".repeat(800));
    let len = other.len() as u64;
    let to = st
        .put_native(&ORIGIN, Some(&n[3]), "row:4", other.as_bytes())
        .unwrap();
    let v = check(&st, &sid, &Op::Swap { to: to.clone() }, 1).unwrap();
    assert!(!v.keeps);
    assert_eq!(v.lost_bytes, len);
    // The scale of the token count: one token per two bytes.
    assert_eq!(v.lost_tokens, len / 2);
    let m = v.message();
    assert!(m.contains("without the cache"), "{m}");
    assert!(m.contains("is not the end of a turn"), "{m}");
    assert!(m.contains(&format!("fork point {}", n[3].short())), "{m}");
}

#[test]
fn a_fork_under_a_new_id_is_cold_and_points_at_the_swap() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid, n) = session(t.path());
    let v = check(&st, &sid, &Op::Fork { at: n[3].clone() }, 1).unwrap();
    assert!(!v.keeps);
    assert!(v.lost_tokens > 0);
    let m = v.message();
    assert!(m.contains("starts cold"), "{m}");
    assert!(m.contains(&format!("paddock horse swap {sid}")), "{m}");
    assert!(
        m.contains(&format!("paddock horse fork {sid} --warm")),
        "{m}"
    );
}

#[test]
fn a_warm_side_branch_at_the_head_keeps_the_prefix() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid, n) = session(t.path());
    let s = st.session(&sid).unwrap();
    let head = n.last().unwrap();
    let v = super::warm_fork(&st, &s, head, 1).unwrap();
    assert!(v.keeps);
    assert_eq!(v.lost_tokens, 0);
    assert!(v.reasons.is_empty());
    assert!(v.prefix_bytes > 0);
    let m = v.message();
    assert!(m.contains("keeps its cache key"), "{m}");
    assert!(m.contains("no codec projects them"), "{m}");
    // The same fork point as a persisted fork is cold.
    assert!(
        !check(&st, &sid, &Op::Fork { at: head.clone() }, 1)
            .unwrap()
            .keeps
    );
    // Only at the head, and only for Codex.
    let e = super::warm_fork(&st, &s, &n[3], 1).unwrap_err();
    assert!(e.to_string().contains("at the head"), "{e:#}");
    let mut other = s.clone();
    other.binding.harness = crate::horse::dsh::HARNESS.into();
    assert!(super::warm_fork(&st, &other, head, 1).is_err());
}

#[test]
fn a_swap_is_only_for_codex() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let h = st.new_default_horse().unwrap();
    let s = st
        .new_session(
            &h.id,
            crate::horse::Binding {
                harness: crate::horse::dsh::HARNESS.into(),
                native_id: None,
                version: crate::horse::dsh::VERSION.into(),
                container: crate::horse::dsh::CONTAINER.into(),
            },
        )
        .unwrap();
    let to = st.put_blob("text/plain", b"x").unwrap();
    let e = check(&st, &s.id, &Op::Swap { to }, 1).unwrap_err();
    assert!(e.to_string().contains("forks instead"), "{e:#}");
}
