//! Two hitched children of a parent whose dsh session is held live: the
//! in-process fake dsh of the live custody tests, the spawner with a fake
//! child program. No sandbox, no harness, no vendor.
use super::*;
use crate::horse::hitch::visible_sessions;
use crate::horse::live::fake::{Fake, Seen};
use crate::horse::live::{custody, harness::Control, rpc};
use crate::horse::{dsh, inbox::Delivery};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::Instant;

const MAIN: &str = "session-11111111-2222-4333-8444-5555555555aa";

fn rows_of(store: &Store, sid: &str) -> Vec<String> {
    let s = store.session(sid).unwrap();
    store
        .session_nodes(&s)
        .unwrap()
        .into_iter()
        .map(|n| String::from_utf8(n.raw).unwrap())
        .collect()
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The deliveries of hitched results to `session`.
fn hitched_deliveries(store: &Store, session: &str) -> Vec<Delivery> {
    store
        .deliveries(session)
        .unwrap()
        .into_iter()
        .filter(|d| d.entry.text.starts_with("hitched "))
        .collect()
}

#[test]
fn two_hitched_results_land_once_each_as_the_newest_entry_at_a_safe_point() {
    let s = setup();
    let store = Store::open_host(&s.env).unwrap();
    let parent = run::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    store
        .lock()
        .unwrap()
        .bind_native(&parent.session, MAIN)
        .unwrap();

    // The parent's custody over the fake dsh on a socket pair.
    let (pad, dsh_end) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fake = Fake {
        root: None,
        existing: Vec::new(),
        step: Duration::from_millis(5),
        seen: seen.clone(),
    };
    let input = dsh_end.try_clone().unwrap();
    std::thread::spawn(move || fake.serve(input, dsh_end));
    let (rt, rx) = std::sync::mpsc::channel();
    rpc::read_runtime(pad.try_clone().unwrap(), rt);
    let (tx, control) = std::sync::mpsc::channel();
    let set = custody::Setup {
        store: store.clone(),
        horse: parent.horse.clone(),
        main: MAIN.into(),
        resume: false,
        sessions_root: None,
        cwd: crate::horse::live::fake::CWD.into(),
        provider: crate::horse::live::PROVIDER.into(),
        model: crate::horse::live::MODEL.into(),
        effort: None,
        run: "sbx-parent".into(),
        tick: Duration::from_millis(20),
    };
    let custody = std::thread::spawn(move || custody::run(set, pad, rx, &control, std::io::sink()));
    tx.send(Control::User("split the work".into())).unwrap();
    wait_for("the first turn of the parent", || {
        rows_of(&store, &parent.session)
            .iter()
            .any(|r| r.contains("\"turn/end\""))
    });

    // Two hitched children through the parent's broker.
    let program = child_that_writes("0");
    let program: Vec<&str> = program.iter().map(|a| a.to_str().unwrap()).collect();
    let b = horse_broker(&s, &parent, &store, &program);
    let sp = b.spawner.clone().unwrap();
    for _ in 0..2 {
        let resp = b.handle(&start_hitched("codex"));
        assert_eq!(resp.status, RStatus::Done, "{}", resp.message);
        let id = resp.result["id"].as_str().unwrap().to_string();
        let done = sp.wait(&id, Duration::from_secs(15)).unwrap();
        assert_eq!(done.state, State::Exited, "{}", done.message);
    }
    wait_for("both results in the parent's session", || {
        let d = hitched_deliveries(&store, &parent.session);
        d.len() == 2 && d.iter().all(|d| d.landed_seq.is_some())
    });
    tx.send(Control::Close).unwrap();
    custody.join().unwrap().unwrap();

    // One horse, three sessions, one SpawnedFrom and one Append per child.
    let sessions = store.horse(&parent.horse).unwrap().sessions;
    assert_eq!(sessions.len(), 3, "{sessions:?}");
    let all = edges(&store, &parent.horse).unwrap();
    let children: Vec<String> = all
        .iter()
        .filter(|e| e.kind == EdgeKind::SpawnedFrom)
        .map(|e| {
            assert_eq!(e.to, parent.session);
            e.from.clone()
        })
        .collect();
    assert_eq!(children.len(), 2);
    for c in &children {
        let appends = all
            .iter()
            .filter(|e| e.kind == EdgeKind::Append && e.from == *c)
            .count();
        assert_eq!(appends, 1, "{all:?}");
    }

    // Each result reached the parent once.
    let rows = rows_of(&store, &parent.session);
    let delivered = hitched_deliveries(&store, &parent.session);
    let prompts = seen.lock().unwrap().prompts.clone();
    for c in &children {
        let head = format!("hitched {} (codex) finished", &c[..8]);
        let mine: Vec<&Delivery> = delivered
            .iter()
            .filter(|d| d.entry.text.starts_with(&head))
            .collect();
        assert_eq!(mine.len(), 1, "{delivered:?}");
        assert_eq!(prompts.iter().filter(|p| p.1.contains(&head)).count(), 1);
        assert_eq!(rows.iter().filter(|r| r.contains(&head)).count(), 2);
    }
    // Nothing came while a turn of the parent ran.
    assert!(
        prompts.iter().all(|(_, _, running)| !running),
        "{prompts:?}"
    );
    for d in &delivered {
        // The safe point: the head at delivery is the end of a turn.
        let at = d.at_head.as_ref().expect("a head at delivery");
        let nodes = store
            .session_nodes(&store.session(&parent.session).unwrap())
            .unwrap();
        let k = nodes.iter().position(|n| n.hash == *at).unwrap();
        assert!(rows[k].contains("\"turn/end\""), "{}", rows[k]);
        // The newest entry: no other message between the safe point and it.
        let landed = d.landed_seq.unwrap() as usize;
        assert!(rows[landed].contains("\"user/message\""));
        assert!(rows[landed].contains(&d.entry.text[..17]));
        assert!(rows[k + 1..landed]
            .iter()
            .all(|r| !r.contains("/message\"")));
    }
    assert!(store.inbox(&parent.session).unwrap().is_empty());

    // A second end of a child changes nothing.
    for c in &children {
        let link = wire::link_of(&Access {
            store: store.clone(),
            horse: parent.horse.clone(),
            session: c.clone(),
        })
        .unwrap()
        .unwrap();
        assert!(wire::finish_once(&store, &link, "again").unwrap().is_none());
    }
    assert_eq!(hitched_deliveries(&store, &parent.session).len(), 2);

    // A sibling sees its own session only.
    for c in &children {
        assert_eq!(
            visible_sessions(&store, &parent.horse, c),
            std::slice::from_ref(c)
        );
    }
}
