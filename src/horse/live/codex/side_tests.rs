//! The warm side branch of a Codex custody, with the fake app-server:
//! an ephemeral `thread/fork` under the cache key of the parent, its turns
//! as live nodes that no codec projects.

use super::*;
use crate::horse::guard::{self, Op};
use crate::horse::live::codex::side::request::{self, Act, Request, State};
use crate::horse::store::Node;
use fake::SIDE_THREAD;

fn nodes(w: &World, sid: &str) -> Vec<Node> {
    w.st.session_nodes(&w.st.session(sid).unwrap()).unwrap()
}

fn holds_text(n: &[Node], text: &str) -> bool {
    n.iter()
        .any(|n| String::from_utf8_lossy(&n.raw).contains(text))
}

/// Ask the custody of `sid` and wait for its answer.
fn ask(w: &World, sid: &str, act: Act, side: Option<&str>) -> Request {
    let r = request::ask(&w.st, sid, act, side).unwrap();
    until("the answer of the custody", || {
        request::answer(&w.st, sid, &r.id).unwrap().is_some()
    });
    let a = request::answer(&w.st, sid, &r.id).unwrap().unwrap();
    request::remove(&w.st, sid, &r.id).unwrap();
    a
}

/// A projected session in custody after one turn of its own.
fn warm_world(f: impl FnOnce(&mut Fake)) -> (World, String, Live) {
    let w = world();
    let sid = w.projected();
    let l = start_with(&w, &sid, f, bypass());
    w.st.queue_append(&sid, "user", "parent question").unwrap();
    until("the parent turn in the store", || {
        holds_text(&nodes(&w, &sid), "echo: parent question")
    });
    (w, sid, l)
}

#[test]
fn a_warm_side_branch_shares_the_key_and_lands_as_live_nodes_that_no_codec_projects() {
    let (w, sid, l) = warm_world(|_| {});
    let parent = w.st.session(&sid).unwrap();
    let head = parent.head.clone().unwrap();
    let file = find_rollout(&w.root, OLD_THREAD).unwrap();
    let bytes = std::fs::read(&file).unwrap();

    let a = ask(&w, &sid, Act::Open, None);
    assert_eq!(a.state, State::Done, "{:?}", a.error);
    assert_eq!(a.thread.as_deref(), Some(SIDE_THREAD));
    assert!(a.verdict.as_ref().unwrap().keeps);
    let side = a.side.unwrap();
    let s = w.st.session(&side).unwrap();
    assert_eq!(s.head.as_ref(), Some(&head));
    assert_eq!(s.fork_of.as_ref().unwrap().session, sid);
    assert_eq!(s.fork_of.as_ref().unwrap().node, head);
    assert_eq!(s.binding.native_id.as_deref(), Some(SIDE_THREAD));
    let fork = l
        .seen
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|r| r.0 == wire::THREAD_FORK)
        .map(|r| r.1.clone())
        .unwrap();
    assert_eq!(fork, json!({ "threadId": OLD_THREAD, "ephemeral": true }));
    // The warm verdict is saved; a persisted fork at the same node is cold.
    let last = w.st.guard_state(&sid).unwrap().last.unwrap();
    assert!(
        last.keeps && last.op.contains("warm side branch"),
        "{last:?}"
    );
    assert!(
        !guard::check(&w.st, &sid, &Op::Fork { at: head.clone() }, 1)
            .unwrap()
            .keeps
    );

    // An entry of the parent waits; the side branch takes its own.
    w.st.queue_append(&sid, "user", "parent waits").unwrap();
    let e = w.st.queue_append(&side, "user", "side question").unwrap();
    w.st.queue_append(&side, "note", "side note").unwrap();
    until("the side turn and the note in the store", || {
        let n = nodes(&w, &side);
        holds_text(&n, "echo: side question") && holds_text(&n, "side note")
    });
    assert_eq!(w.st.inbox(&sid).unwrap().len(), 1);
    let d = w.st.deliveries(&side).unwrap();
    assert!(d
        .iter()
        .any(|d| d.entry.id == e.id && d.landed_seq.is_some()));

    // Both threads went out under one cache key.
    {
        let seen = l.seen.lock().unwrap();
        assert_eq!(seen.steers, 0);
        let threads: Vec<&str> = seen.keys.iter().map(|k| k.0.as_str()).collect();
        assert_eq!(threads, [OLD_THREAD, SIDE_THREAD]);
        assert!(
            seen.keys.iter().all(|k| k.1 == OLD_THREAD),
            "{:?}",
            seen.keys
        );
    }

    // Every node after the fork point is live, the user input once.
    let n = nodes(&w, &side);
    let k = n.iter().position(|x| x.hash == head).unwrap();
    assert!(n[..=k].iter().all(|x| !x.is_live()));
    assert!(n[k + 1..].iter().all(|x| x.is_live()));
    // The fake sends the user input back after the answer to
    // `turn/start`, the note before the answer to `thread/inject_items`.
    for text in ["\"side question\"", "side note"] {
        let once = n[k + 1..]
            .iter()
            .filter(|x| String::from_utf8_lossy(&x.raw).contains(text))
            .count();
        assert_eq!(once, 1, "{text}");
    }
    for x in &n[k + 1..] {
        let v: Value = serde_json::from_slice(&x.raw).unwrap();
        assert_eq!(v["type"], "response_item");
        assert!(v["timestamp"].as_str().unwrap().ends_with('Z'));
    }

    // No codec projects the side session.
    let home = w._t.path().join("other/.codex");
    let err = codec::project(&w.st, &side, &home, false).unwrap_err();
    assert!(
        format!("{err:#}").contains("is not a native original; persist it with the harness first"),
        "{err:#}"
    );
    assert!(!home.join("sessions").exists());
    assert!(find_rollout(&w.root, SIDE_THREAD).is_none());

    // The parent did not move.
    assert_eq!(w.st.session(&sid).unwrap().head, Some(head.clone()));
    assert_eq!(std::fs::read(&file).unwrap(), bytes);

    // Closing ends the side custody; the parent takes its entry.
    let c = ask(&w, &sid, Act::Close, Some(&side));
    assert_eq!(c.state, State::Done, "{:?}", c.error);
    assert_eq!(c.turns, 1);
    until("the waiting parent entry in the store", || {
        holds_text(&nodes(&w, &sid), "echo: parent waits")
    });
    assert_eq!(nodes(&w, &side).len(), n.len());
    let out = l.out.clone();
    let r = l.close();
    assert_eq!(r.side_turns, 1);
    assert!(
        out.text().contains("closed after 1 turns"),
        "{}",
        out.text()
    );
}

#[test]
fn one_side_branch_at_a_time_with_interrupts_and_it_ends_with_the_run() {
    let (w, sid, l) = warm_world(|_| {});
    let c = ask(&w, &sid, Act::Close, None);
    assert_eq!(c.state, State::Failed);
    assert!(c.error.unwrap().contains("no open side branch"));
    let side = ask(&w, &sid, Act::Open, None).side.unwrap();
    let again = ask(&w, &sid, Act::Open, None);
    assert_eq!(again.state, State::Failed);
    assert!(again
        .error
        .unwrap()
        .contains("one branch runs under a cache key"));
    let wrong = ask(&w, &sid, Act::Close, Some(&sid));
    assert_eq!(wrong.state, State::Failed);

    // A long side turn stops on an interrupt of the side thread.
    w.st.queue_append(&side, "user", "[long] side work")
        .unwrap();
    until("the side turn", || count(&l.seen, wire::TURN_START) == 2);
    l.ctl.send(Control::Interrupt).unwrap();
    until("the interrupt", || {
        count(&l.seen, wire::TURN_INTERRUPT) == 1
    });
    let p = l
        .seen
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|r| r.0 == wire::TURN_INTERRUPT)
        .map(|r| r.1.clone())
        .unwrap();
    assert_eq!(p["threadId"], SIDE_THREAD);
    w.st.queue_append(&side, "user", "after the stop").unwrap();
    until("the next side turn", || {
        holds_text(&nodes(&w, &side), "echo: after the stop")
    });
    let out = l.out.clone();
    let r = l.close();
    assert_eq!(r.side_turns, 2);
    assert!(out.text().contains("closed with the run"), "{}", out.text());
    assert!(nodes(&w, &side).iter().any(|n| n.is_live()));
}

#[test]
fn a_persisted_fork_is_not_held_and_the_parent_goes_on() {
    let (w, sid, l) = warm_world(|f| f.persist_forks = true);
    let sessions = w.st.sessions().unwrap().len();
    let a = ask(&w, &sid, Act::Open, None);
    assert_eq!(a.state, State::Failed);
    assert!(a.error.unwrap().contains("persisted fork"));
    assert_eq!(w.st.sessions().unwrap().len(), sessions);
    w.st.queue_append(&sid, "user", "still here").unwrap();
    until("the parent turn", || {
        holds_text(&nodes(&w, &sid), "echo: still here")
    });
    assert_eq!(l.close().side_turns, 0);
}

#[test]
fn a_side_turn_without_items_is_said() {
    let (w, sid, l) = warm_world(|f| f.raw_items = false);
    let side = ask(&w, &sid, Act::Open, None).side.unwrap();
    w.st.queue_append(&side, "user", "quiet").unwrap();
    let out = l.out.clone();
    until("the warning", || {
        out.text().contains("sent no item of side turn")
    });
    // The input of paddock is there, the answer is not.
    let n = nodes(&w, &side);
    assert!(holds_text(&n, "quiet") && !holds_text(&n, "echo: quiet"));
    l.close();
}
