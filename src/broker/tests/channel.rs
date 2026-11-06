//! The live requests of a TUI run through the broker, end to end with the
//! channel server of the sandbox: the hooks' safe points, the pushes and
//! the ledger rows.
use super::*;
use crate::hostcli::horse::channel::{serve, Call, Config, NOTIFICATION};
use std::time::{Duration, Instant};

fn rows(b: &Broker) -> Vec<String> {
    let c = rusqlite::Connection::open(crate::devices::ledger::path(&b.env)).unwrap();
    let mut s = c
        .prepare("SELECT status FROM requests ORDER BY id")
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn give_horse(b: &Broker, tmp: &Path) -> crate::horse::device::Access {
    let store = crate::horse::Store::open(&tmp.join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let binding = crate::horse::Binding {
        harness: "claude".into(),
        native_id: Some("native".into()),
        version: "test".into(),
        container: "test".into(),
    };
    let s = store.new_session(&h.id, binding).unwrap();
    let access = crate::horse::device::Access {
        store,
        horse: h.id,
        session: s.id,
    };
    b.horse.set(access.clone()).ok().unwrap();
    access
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_channel_pushes_this_runs_entries_after_the_stop_only() {
    let (tmp, b, fixed) = setup(None, HostPolicy::default());
    let access = give_horse(&b, tmp.path());
    let st = &access.store;
    let e1 = st.queue_append(&access.session, "user", "first").unwrap();
    let e2 = st.queue_append(&access.session, "note", "second").unwrap();
    let b = Arc::new(b);
    let b2 = b.clone();
    let call: Call = Arc::new(move |r: &Request| Ok(b2.handle(r)));
    let (reader, mut writer) = std::io::pipe().unwrap();
    let out = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink: Arc<Mutex<dyn std::io::Write + Send>> = out.clone();
    let cfg = Config {
        wait: Duration::from_millis(200),
        ..Config::default()
    };
    let server =
        std::thread::spawn(move || serve(std::io::BufReader::new(reader), sink, call, cfg));
    writeln!(
        writer,
        r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#
    )
    .unwrap();
    writeln!(
        writer,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    let pushed = || {
        String::from_utf8(out.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| v["method"] == NOTIFICATION)
            .collect::<Vec<_>>()
    };
    // Busy until the first stop: two full waits pass with nothing pushed.
    let busy = Instant::now();
    while busy.elapsed() < Duration::from_millis(500) {
        assert!(pushed().is_empty());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(st.inbox(&access.session).unwrap().len(), 2);
    let r = b.handle(&req(
        "horse",
        serde_json::json!({"op": "safe_point", "kind": "stop"}),
    ));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    wait_for("two pushes", || pushed().len() == 2);
    let got = pushed();
    assert_eq!(got[0]["params"]["content"], "first");
    assert_eq!(got[0]["params"]["meta"]["entry_id"], e1.id.as_str());
    assert_eq!(got[1]["params"]["meta"]["role"], "note");
    assert_eq!(got[1]["params"]["meta"]["session"], access.session.as_str());
    let d = st.deliveries(&access.session).unwrap();
    assert_eq!(d.len(), 2);
    assert!(d.iter().all(|d| d.landed_seq.is_none()));
    assert_eq!(
        rows(&b),
        vec![
            format!("channel.release {}", e1.id),
            format!("channel.release {}", e2.id)
        ]
    );
    // The release started a turn: a new entry waits for the next stop.
    b.handle(&req(
        "horse",
        serde_json::json!({"op": "safe_point", "kind": "prompt"}),
    ));
    let e3 = st.queue_append(&access.session, "user", "third").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pushed().len(), 2);
    b.handle(&req(
        "horse",
        serde_json::json!({"op": "safe_point", "kind": "stop"}),
    ));
    wait_for("the third push", || pushed().len() == 3);
    assert_eq!(pushed()[2]["params"]["meta"]["entry_id"], e3.id.as_str());
    drop(writer);
    assert_eq!(server.join().unwrap(), 0);
    // Never a card: these requests ask nobody.
    assert_eq!(fixed.asked.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn without_a_horse_the_live_requests_fail_and_record_nothing() {
    let (_tmp, b, _) = setup(None, HostPolicy::default());
    for args in [
        serde_json::json!({"op": "safe_point", "kind": "stop"}),
        serde_json::json!({"op": "channel_wait", "since": 0, "timeout_ms": 10}),
    ] {
        let r = b.handle(&req("horse", args));
        assert_eq!(r.status, Status::Failed);
        assert_eq!(r.message, "this sandbox has no horse");
    }
    assert!(!crate::devices::ledger::path(&b.env).exists() || rows(&b).is_empty());
}
