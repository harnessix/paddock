//! Two hitched Claude Code children that start at the same moment, as
//! with `[spawn] max_running = 2`: the host-side steps of each start run
//! on their own thread, each with its own open store, while a reader
//! polls the store as a host would. No sandbox, no harness.
use super::*;
use crate::horse::{dsh, run};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};

/// The store steps of `lifecycle::start` for a headless hitched child,
/// then the end of the child as the spawner does it.
fn child_start(root: &std::path::Path, hitched: &Hitched, sandbox: &str) -> String {
    let store = Store::open(root).unwrap();
    let rh = hitched.run();
    let session = store.session(&rh.session).unwrap();
    assert_eq!(session.horse, rh.horse);
    run::by_sandbox::record(&store, sandbox, &rh).unwrap();
    run::record_event(&store, &rh, "run.begin", sandbox, serde_json::json!({})).unwrap();
    let native = crate::horse::uuid_v4().unwrap();
    store
        .lock()
        .unwrap()
        .bind_native(&rh.session, &native)
        .unwrap();
    store
        .queue_append(&rh.session, "user", "write the note")
        .unwrap();
    native
}

#[test]
fn two_hitched_children_that_start_together_both_get_their_bound_session() {
    for round in 0..20 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = Store::open(&root).unwrap();
        let parent = run::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
        store
            .lock()
            .unwrap()
            .bind_native(&parent.session, "session-parent")
            .unwrap();

        // A reader that polls as the host and the test of a host do.
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let reader = {
            let (root, stop, seen) = (root.clone(), stop.clone(), seen.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let all = Store::open(&root).and_then(|s| s.sessions());
                    let all =
                        all.unwrap_or_else(|e| panic!("a read of the sessions failed: {e:#}"));
                    let mut seen = seen.lock().unwrap();
                    for s in all {
                        let bound = s.binding.harness == "claude" && s.binding.native_id.is_some();
                        if bound && !seen.contains(&s.id) {
                            seen.push(s.id);
                        }
                    }
                }
            })
        };

        // Both begins and both starts at the same moment.
        let gate = Arc::new(Barrier::new(2));
        let starts: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|n| {
                let (root, parent, gate) = (root.clone(), parent.clone(), gate.clone());
                std::thread::spawn(move || {
                    let store = Store::open(&root).unwrap();
                    gate.wait();
                    let hitched = begin(&store, &parent, "claude").unwrap();
                    let native = child_start(&root, &hitched, &format!("claude-spawn-{n}"));
                    (hitched, native)
                })
            })
            .collect();
        let children: Vec<(Hitched, String)> =
            starts.into_iter().map(|t| t.join().unwrap()).collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while seen.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        stop.store(true, Ordering::SeqCst);
        reader.join().unwrap();

        let (a, b) = (&children[0].0, &children[1].0);
        assert_ne!(a.session, b.session, "round {round}");
        let mut seen = seen.lock().unwrap().clone();
        seen.sort();
        let mut want = vec![a.session.clone(), b.session.clone()];
        want.sort();
        assert_eq!(seen, want, "round {round}");
        let horse = store.horse(&parent.horse).unwrap();
        assert_eq!(horse.sessions.len(), 3, "round {round}: {horse:?}");
        let all = edges(&store, &parent.horse).unwrap();
        for (child, native) in &children {
            let s = store.session(&child.session).unwrap();
            assert_eq!(s.binding.native_id.as_deref(), Some(native.as_str()));
            assert!(horse.sessions.contains(&child.session));
            let spawned = all
                .iter()
                .filter(|e| e.kind == EdgeKind::SpawnedFrom && e.from == child.session)
                .count();
            assert_eq!(spawned, 1, "round {round}: {all:?}");
            assert_eq!(store.inbox(&child.session).unwrap().len(), 2);
            // The device view of each child shows its own bound session only.
            let access = Access {
                store: store.clone(),
                horse: parent.horse.clone(),
                session: child.session.clone(),
            };
            let tree = access.tree().unwrap();
            let own = format!("self/sessions/{}/binding", child.session);
            assert!(tree.paths.contains_key(&own), "round {round}");
            let listed = tree
                .paths
                .keys()
                .filter(|p| p.starts_with("self/sessions/") && p.ends_with("/binding"))
                .count();
            assert_eq!(listed, 1, "round {round}");
            assert_eq!(link_of(&access).unwrap().as_ref(), Some(child));
        }
        let runs = run::events(&store, &parent.horse)
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "run.begin")
            .count();
        assert_eq!(runs, 2, "round {round}");
    }
}
