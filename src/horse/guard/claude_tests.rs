//! Tests of the guard for Claude Code sessions, with transcripts in the
//! shapes of claude/shape_tests.rs.

use super::super::{check, tuple, Op, Tuple};
use super::*;
use crate::horse::claude::shape::tests::{assistant, hash, ingested, request, user, CWD, SID};
use crate::horse::claude::{ingest, transcript_path, VERSION};
use crate::horse::store::Store;
use std::path::Path;

/// The request time of the last request of `rows`.
const AT: &str = "2026-11-04T10:05:00Z";

fn at_ms() -> u64 {
    crate::waiting::parse_rfc3339(AT).unwrap() * 1000
}

fn rows(second: &str) -> Vec<String> {
    vec![
        user("u1", None),
        request(&hash('a'), "2026-11-04T10:00:00Z"),
        assistant("a1", "u1", 0, 6000),
        user("u2", Some("a1")),
        request(second, AT),
        assistant("a2", "u2", 6000, 40),
    ]
}

/// The prompt tokens of `a2`: input, cache write and cache read.
const PREFIX: u64 = 3 + 40 + 6000;

/// Append rows to the transcript of `SID` and ingest it again.
fn append(t: &Path, st: &Store, more: &[String]) {
    let path = transcript_path(&t.join("claude"), CWD, SID);
    let mut body = std::fs::read_to_string(&path).unwrap();
    for r in more {
        body += r;
        body.push('\n');
    }
    std::fs::write(&path, body).unwrap();
    ingest(st, &path, VERSION, None).unwrap();
}

fn shape_fields(t: &Tuple) -> (Option<String>, Option<String>, Option<u64>, Option<String>) {
    (
        t.system_hash.as_ref().map(|h| h.to_string()),
        t.tools_hash.as_ref().map(|h| h.to_string()),
        t.last_request_ms,
        t.model.clone(),
    )
}

#[test]
fn the_cost_table_and_the_ttl_steps() {
    assert_eq!(
        cost(PREFIX),
        Cost {
            on_break: 7554,
            on_hit: 605
        }
    );
    assert_eq!(
        cost(0),
        Cost {
            on_break: 0,
            on_hit: 0
        }
    );
    assert_eq!(cost(u64::MAX).on_break, u64::MAX.div_ceil(1000));
    let t = 10 * MAYBE_WARM_MS;
    assert_eq!(ttl_verdict(t, t), Ttl::Warm);
    assert_eq!(ttl_verdict(t + WARM_MS - 1, t), Ttl::Warm);
    assert_eq!(ttl_verdict(t + WARM_MS, t), Ttl::MaybeWarm);
    assert_eq!(ttl_verdict(t + MAYBE_WARM_MS - 1, t), Ttl::MaybeWarm);
    assert_eq!(ttl_verdict(t + MAYBE_WARM_MS, t), Ttl::Cold);
    // A clock behind the request is no idle time.
    assert_eq!(ttl_verdict(t - 1, t), Ttl::Warm);
    assert_eq!(ttl_line(t + 120_000, t), "last request 2 min ago: warm");
    assert_eq!(
        ttl_line(t + 30 * 60_000, t),
        "last request 30 min ago: maybe warm (1 h on a subscription)"
    );
    assert_eq!(
        ttl_line(t + 70 * 60_000, t),
        "last request 70 min ago: cold"
    );
}

#[test]
fn the_tuple_comes_from_the_shape() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid) = ingested(t.path(), &rows(&hash('a')));
    let tu = tuple(&st, &st.session(&sid).unwrap()).unwrap();
    assert_eq!(tu.system_hash.as_ref().unwrap().as_str(), hash('a'));
    assert_eq!(tu.tools_hash, tu.system_hash);
    assert_eq!(tu.last_request_ms, Some(at_ms()));
    assert_eq!(tu.model.as_deref(), Some("claude-opus-5-5"));
    // A shape hash that is no sha256 still gives a stable tuple field.
    let mut x = Tuple::default();
    let s = crate::horse::claude::shape::parse(request("ab12", AT).as_bytes()).unwrap();
    fill(&mut x, &s);
    assert_eq!(x.system_hash, Some(Hash::of(b"ab12")));
    // A chain without a request row keeps the fields as they are.
    let t2 = tempfile::tempdir().unwrap();
    let (st2, sid2) = ingested(t2.path(), &[user("u1", None), assistant("a1", "u1", 0, 9)]);
    let tu2 = tuple(&st2, &st2.session(&sid2).unwrap()).unwrap();
    assert_eq!(shape_fields(&tu2), (None, None, None, None));
    let v = check(&st2, &sid2, &Op::Append { after: None }, at_ms()).unwrap();
    assert!(
        v.keeps && v.reasons.is_empty() && v.notes.is_empty(),
        "{v:?}"
    );
}

#[test]
fn the_same_shape_and_a_fresh_request_is_warm() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid) = ingested(t.path(), &rows(&hash('a')));
    st.refresh_guard(&sid).unwrap();
    assert_eq!(st.guard_state(&sid).unwrap().last, None);
    let v = check(&st, &sid, &Op::Append { after: None }, at_ms() + 60_000).unwrap();
    assert!(v.keeps, "{v:?}");
    assert_eq!(v.lost_tokens, 0);
    assert!(v.reasons.is_empty(), "{v:?}");
    assert_eq!(
        v.notes,
        [
            format!("shape {}", &hash('a')[..12]),
            "last request 1 min ago: warm".to_string()
        ]
    );
    assert!(v.message().contains("last request 1 min ago: warm"));
}

#[test]
fn a_shape_change_is_a_break_from_token_0() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid) = ingested(t.path(), &rows(&hash('a')));
    st.refresh_guard(&sid).unwrap();
    append(
        t.path(),
        &st,
        &[
            user("u3", Some("a2")),
            request(&hash('b'), "2026-11-04T10:06:00Z"),
            assistant("a3", "u3", 0, 6100),
        ],
    );
    let now = at_ms() + 2 * 60_000;
    let v = check(&st, &sid, &Op::Append { after: None }, now).unwrap();
    assert!(!v.keeps, "{v:?}");
    let prefix = 3 + 6100;
    assert_eq!(v.lost_tokens, prefix);
    let m = v.message();
    assert!(
        m.contains(&hash('a')[..12]) && m.contains(&hash('b')[..12]),
        "{m}"
    );
    assert!(m.contains("from token 0"), "{m}");
    let c = cost(prefix);
    assert!(
        m.contains(&format!(
            "a break pays {prefix} tokens at the cache-write price (about {} input tokens) instead of {}",
            c.on_break, c.on_hit
        )),
        "{m}"
    );
    assert!(!m.contains("bytes"), "{m}");
    // The refresh after the turn records the break as the last verdict,
    // and the next check is quiet again.
    st.refresh_guard(&sid).unwrap();
    let last = st.guard_state(&sid).unwrap().last.unwrap();
    assert!(!last.keeps && last.message().contains(&hash('b')[..12]));
    let again = check(&st, &sid, &Op::Append { after: None }, now).unwrap();
    assert!(again.keeps, "{again:?}");
}

#[test]
fn an_old_request_is_cold_and_a_half_hour_is_maybe_warm() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid) = ingested(t.path(), &rows(&hash('a')));
    st.refresh_guard(&sid).unwrap();
    let cold = check(
        &st,
        &sid,
        &Op::Append { after: None },
        at_ms() + 70 * 60_000,
    )
    .unwrap();
    assert!(!cold.keeps);
    assert_eq!(cold.lost_tokens, PREFIX);
    assert!(
        cold.message().contains("last request 70 min ago: cold"),
        "{}",
        cold.message()
    );
    let maybe = check(
        &st,
        &sid,
        &Op::Append { after: None },
        at_ms() + 30 * 60_000,
    )
    .unwrap();
    assert!(maybe.keeps, "{maybe:?}");
    assert_eq!(
        maybe.reasons,
        ["last request 30 min ago: maybe warm (1 h on a subscription)"]
    );
}

#[test]
fn system_tools_and_model_changes_break_from_token_0() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid) = ingested(t.path(), &rows(&hash('a')));
    let now = at_ms() + 1000;
    for op in [
        Op::System,
        Op::Tools { added_only: true },
        Op::Model("claude-sonnet-5-5".into()),
    ] {
        let v = check(&st, &sid, &op, now).unwrap();
        assert!(!v.keeps, "{op:?}: {v:?}");
        assert_eq!(v.lost_tokens, PREFIX, "{op:?}");
    }
    let same = check(&st, &sid, &Op::Model("claude-opus-5-5".into()), now).unwrap();
    assert!(same.keeps, "{same:?}");
}

#[test]
fn a_fork_keeps_the_shape_of_its_parent() {
    let t = tempfile::tempdir().unwrap();
    let (st, sid) = ingested(t.path(), &rows(&hash('a')));
    st.refresh_guard(&sid).unwrap();
    let parent = st.session(&sid).unwrap();
    let head = parent.head.clone().unwrap();
    let now = at_ms() + 60_000;
    let at_head = check(&st, &sid, &Op::Fork { at: head.clone() }, now).unwrap();
    assert!(at_head.keeps, "{at_head:?}");
    let fork = st.fork(&sid, &head, None).unwrap();
    let pt = tuple(&st, &parent).unwrap();
    let ft = tuple(&st, &st.session(&fork.id).unwrap()).unwrap();
    assert_eq!(shape_fields(&ft), shape_fields(&pt));
    assert_eq!(ft.head, pt.head);
    // The guard stays silent for the fork: same shape, warm cache.
    st.refresh_guard(&fork.id).unwrap();
    let v = check(&st, &fork.id, &Op::Append { after: None }, now).unwrap();
    assert!(v.keeps && v.reasons.is_empty(), "{v:?}");
    assert_eq!(st.guard_state(&fork.id).unwrap().last, None);
}
