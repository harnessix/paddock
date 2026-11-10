use super::*;
use crate::horse::{Binding, Store};
use std::sync::Mutex as StdMutex;

fn store() -> (tempfile::TempDir, Store) {
    let t = tempfile::tempdir().unwrap();
    let s = Store::open(t.path()).unwrap();
    (t, s)
}

fn access(store: &Store) -> Access {
    let h = store.new_default_horse().unwrap();
    let b = Binding {
        harness: "claude".into(),
        native_id: Some("native-1".into()),
        version: "test".into(),
        container: "test".into(),
    };
    let s = store.new_session(&h.id, b).unwrap();
    Access {
        store: store.clone(),
        horse: h.id,
        session: s.id,
    }
}

fn req(args: serde_json::Value) -> Request {
    Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: serde_json::from_value(args).unwrap(),
        reason: String::new(),
    }
}

fn wait(since: u64, ms: u64) -> Request {
    req(serde_json::json!({"op": CHANNEL_WAIT, "since": since, "timeout_ms": ms}))
}

fn point(kind: &str) -> Request {
    req(serde_json::json!({"op": SAFE_POINT, "kind": kind}))
}

fn ids(r: &Response) -> Vec<String> {
    assert_eq!(r.status, Status::Done, "{}", r.message);
    r.result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn release_needs_an_idle_tui_and_never_repeats_an_entry() {
    let (_t, st) = store();
    let a = access(&st);
    let e1 = st.queue_append(&a.session, "user", "one").unwrap();
    let e2 = st.queue_append(&a.session, "note", "two").unwrap();
    let mut s = TuiState::default();
    assert!(s.busy, "a new state is busy until the first stop");
    assert!(release_pending(&mut s, vec![e1.clone()]).is_empty());
    mark(&mut s, SafePoint::Stop, 42);
    assert_eq!(s.last_stop_ms, 42);
    let out = release_pending(&mut s, vec![e1.clone(), e2.clone()]);
    assert_eq!(out, vec![e1.clone(), e2.clone()]);
    assert!(s.busy, "a release starts a turn");
    mark(&mut s, SafePoint::Stop, 43);
    assert!(release_pending(&mut s, vec![e1, e2]).is_empty());
    mark(&mut s, SafePoint::Prompt, 44);
    assert!(s.busy);
    assert_eq!(s.last_stop_ms, 43);
    assert!("idle".parse::<SafePoint>().is_err());
}

#[test]
fn a_wait_while_busy_times_out_empty() {
    let (_t, st) = store();
    let a = access(&st);
    st.queue_append(&a.session, "user", "hello").unwrap();
    let runs = Runs::default();
    let lines = StdMutex::new(Vec::<String>::new());
    let ledger = |l: &str| lines.lock().unwrap().push(l.to_string());
    let start = Instant::now();
    let r = handle_in(&runs, Some(&a), &wait(0, 300), &ledger);
    assert!(ids(&r).is_empty());
    assert_eq!(r.result["next"], 0);
    assert!(start.elapsed() >= Duration::from_millis(300));
    // After a prompt it is still busy.
    handle_in(&runs, Some(&a), &point("prompt"), &ledger);
    assert!(ids(&handle_in(&runs, Some(&a), &wait(0, 50), &ledger)).is_empty());
    assert_eq!(st.inbox(&a.session).unwrap().len(), 1);
    assert!(st.deliveries(&a.session).unwrap().is_empty());
    assert!(lines.lock().unwrap().is_empty());
}

#[test]
fn a_stop_releases_the_pending_entries_once_in_order() {
    let (_t, st) = store();
    let a = access(&st);
    let e1 = st.queue_append(&a.session, "user", "first").unwrap();
    let e2 = st.queue_append(&a.session, "system", "second").unwrap();
    let runs = Runs::default();
    let lines = StdMutex::new(Vec::<String>::new());
    let ledger = |l: &str| lines.lock().unwrap().push(l.to_string());
    let r = handle_in(&runs, Some(&a), &point("stop"), &ledger);
    assert_eq!(r.status, Status::Done, "{}", r.message);
    // The stop releases nothing by itself.
    assert_eq!(st.inbox(&a.session).unwrap().len(), 2);
    let r = handle_in(&runs, Some(&a), &wait(0, 1000), &ledger);
    assert_eq!(ids(&r), vec![e1.id.clone(), e2.id.clone()]);
    assert_eq!(r.result["next"], 2);
    assert_eq!(r.result["entries"][1]["text"], "second");
    assert_eq!(r.result["entries"][1]["role"], "system");
    let d = st.deliveries(&a.session).unwrap();
    assert_eq!(
        d.iter().map(|d| d.entry.id.clone()).collect::<Vec<_>>(),
        vec![e1.id.clone(), e2.id.clone()]
    );
    assert!(d
        .iter()
        .all(|d| d.landed_seq.is_none() && d.message_id == d.entry.id));
    assert!(st.inbox(&a.session).unwrap().is_empty());
    assert_eq!(
        *lines.lock().unwrap(),
        vec![
            format!("channel.release {}", e1.id),
            format!("channel.release {}", e2.id)
        ]
    );
    // The same since again, and after another stop: nothing twice.
    assert!(ids(&handle_in(&runs, Some(&a), &wait(0, 50), &ledger)).is_empty());
    handle_in(&runs, Some(&a), &point("stop"), &ledger);
    assert!(ids(&handle_in(&runs, Some(&a), &wait(2, 50), &ledger)).is_empty());
    assert_eq!(st.deliveries(&a.session).unwrap().len(), 2);
    assert_eq!(runs.state(&key(&a)).released.len(), 2);
}

#[test]
fn a_waiting_request_wakes_at_the_stop() {
    for _ in 0..20 {
        let (_t, st) = store();
        let a = access(&st);
        let e = st.queue_append(&a.session, "user", "later").unwrap();
        let runs = Arc::new(Runs::default());
        let (r2, a2) = (runs.clone(), a.clone());
        let waiter =
            std::thread::spawn(move || handle_in(&r2, Some(&a2), &wait(0, 10_000), &|_| {}));
        runs.mark(&key(&a), SafePoint::Stop);
        let start = Instant::now();
        let r = waiter.join().unwrap();
        assert_eq!(ids(&r), vec![e.id]);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

#[test]
fn an_entry_queued_during_the_wait_goes_out() {
    let (_t, st) = store();
    let a = access(&st);
    let runs = Arc::new(Runs::default());
    runs.mark(&key(&a), SafePoint::Stop);
    let (r2, a2) = (runs.clone(), a.clone());
    let waiter = std::thread::spawn(move || handle_in(&r2, Some(&a2), &wait(0, 10_000), &|_| {}));
    let e = st.queue_append(&a.session, "user", "new").unwrap();
    assert_eq!(ids(&waiter.join().unwrap()), vec![e.id]);
}

#[test]
fn another_run_never_gets_the_entries_of_this_one() {
    let (_t, st) = store();
    let mine = access(&st);
    let other = access(&st);
    let e_other = st.queue_append(&other.session, "user", "secret").unwrap();
    let e_mine = st.queue_append(&mine.session, "user", "mine").unwrap();
    let runs = Runs::default();
    handle_in(&runs, Some(&mine), &point("stop"), &|_| {});
    let r = handle_in(&runs, Some(&mine), &wait(0, 200), &|_| {});
    assert_eq!(ids(&r), vec![e_mine.id]);
    // The other run is still busy, and the request has no session field
    // that could point anywhere else.
    let mut forged = wait(0, 50);
    forged
        .args
        .insert("session".into(), serde_json::json!(other.session));
    handle_in(&runs, Some(&mine), &point("stop"), &|_| {});
    assert!(ids(&handle_in(&runs, Some(&mine), &forged, &|_| {})).is_empty());
    assert_eq!(st.inbox(&other.session).unwrap(), vec![e_other]);
    assert!(st.deliveries(&other.session).unwrap().is_empty());
}

#[test]
fn without_a_horse_both_requests_fail_like_the_other_horse_requests() {
    for r in [wait(0, 10), point("stop")] {
        let resp = handle_in(&Runs::default(), None, &r, &|_| {});
        assert_eq!(resp.status, Status::Failed);
        assert_eq!(resp.message, "this sandbox has no horse");
    }
    let (_t, st) = store();
    let a = access(&st);
    let resp = handle_in(&Runs::default(), Some(&a), &point("idle"), &|_| {});
    assert_eq!(resp.status, Status::Failed);
    assert!(resp.message.contains("stop or prompt"), "{}", resp.message);
    let mut old = point("stop");
    old.v = PROTOCOL + 1;
    assert_eq!(
        handle_in(&Runs::default(), Some(&a), &old, &|_| {}).status,
        Status::Failed
    );
}

#[test]
fn a_failed_record_keeps_the_entry_for_the_next_wait() {
    let (_t, st) = store();
    let a = access(&st);
    let e1 = st.queue_append(&a.session, "user", "one").unwrap();
    let e2 = st.queue_append(&a.session, "user", "two").unwrap();
    let runs = Runs::default();
    runs.mark("k", SafePoint::Stop);
    let inbox = || Ok(vec![e1.clone(), e2.clone()]);
    // The second record fails: the first goes out, the second stays.
    let mut n = 0;
    let out = runs
        .wait("k", Duration::from_millis(10), inbox, |_| {
            n += 1;
            if n == 2 {
                bail!("disk full")
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(out, vec![e1.clone()]);
    assert_eq!(runs.state("k").released, vec![e1.id.clone()]);
    // The first record fails: nothing goes out and the TUI stays idle.
    runs.mark("k", SafePoint::Stop);
    let err = runs
        .wait(
            "k",
            Duration::from_millis(10),
            || Ok(vec![e2.clone()]),
            |_| bail!("disk full"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("disk full"));
    let s = runs.state("k");
    assert!(!s.busy);
    assert_eq!(s.released, vec![e1.id.clone()]);
    let out = runs
        .wait(
            "k",
            Duration::from_millis(10),
            || Ok(vec![e2.clone()]),
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(out, vec![e2]);
}

#[test]
fn only_the_two_ops_are_live_ops() {
    assert!(is_live_op(&wait(0, 1)));
    assert!(is_live_op(&point("stop")));
    assert!(!is_live_op(&req(serde_json::json!({"op": "sessions"}))));
    let mut other = point("stop");
    other.cap = "kv".into();
    assert!(!is_live_op(&other));
}

#[test]
fn a_custody_hears_the_hooks_and_can_give_an_entry_back() {
    let runs = Runs::default();
    let rx = runs.subscribe("h/s");
    let other = runs.subscribe("h/t");
    runs.mark("h/s", SafePoint::Prompt);
    runs.mark_thread("h/s", SafePoint::Stop, Some("t-1"));
    let heard = |kind, thread: Option<&str>| Heard {
        kind,
        thread: thread.map(String::from),
        turn: None,
    };
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [
            heard(SafePoint::Prompt, None),
            heard(SafePoint::Stop, Some("t-1"))
        ]
    );
    assert!(other.try_recv().is_err());
    // A released entry that started no turn: idle again, and with
    // `again` it may go out once more.
    {
        let run = runs.run("h/s");
        let mut st = run.state.lock().unwrap();
        st.busy = true;
        st.released = vec!["e1".into(), "e2".into()];
    }
    runs.give_back("h/s", "e1", true);
    let st = runs.state("h/s");
    assert_eq!(
        (st.busy, st.released.clone()),
        (false, vec!["e2".to_string()])
    );
    runs.mark("h/s", SafePoint::Prompt);
    runs.give_back("h/s", "e2", false);
    assert_eq!(runs.state("h/s").released, ["e2"]);
    assert!(!runs.state("h/s").busy);
    // A dropped receiver is dropped from the list; forget ends the run.
    drop(rx);
    runs.mark("h/s", SafePoint::Stop);
    runs.forget("h/s");
    assert_eq!(runs.state("h/s"), TuiState::default());
    assert_eq!(run_key("h", "s"), "h/s");
}

#[test]
fn a_safe_point_names_its_thread_and_a_bad_thread_fails() {
    let runs = Runs::default();
    let dir = tempfile::tempdir().unwrap();
    let store = crate::horse::Store::open(dir.path()).unwrap();
    let h = store.new_default_horse().unwrap();
    let s = store
        .new_session(&h.id, crate::horse::claude::binding(None))
        .unwrap();
    let access = Access {
        store,
        horse: h.id.clone(),
        session: s.id.clone(),
    };
    let rx = runs.subscribe(&run_key(&h.id, &s.id));
    let mut r = point("stop");
    r.args.insert("thread".into(), serde_json::json!("019a-t"));
    let resp = handle_in(&runs, Some(&access), &r, &|_| {});
    assert_eq!(resp.status, Status::Done, "{}", resp.message);
    assert_eq!(rx.try_recv().unwrap().thread.as_deref(), Some("019a-t"));
    r.args.insert("thread".into(), serde_json::json!("../x"));
    let resp = handle_in(&runs, Some(&access), &r, &|_| {});
    assert_eq!(resp.status, Status::Failed);
    assert!(resp.message.contains("thread id"), "{}", resp.message);
    assert!(rx.try_recv().is_err());
    // A run whose entries paddock types releases none to a channel server.
    runs.typed(&run_key(&h.id, &s.id));
    let st = runs.state(&run_key(&h.id, &s.id));
    assert!(st.typed && !st.busy);
}
