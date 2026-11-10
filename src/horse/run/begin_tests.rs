//! The DNA entry of a new session goes first, also when another process
//! appends to the session the moment that it appears in the store (`paddock
//! horse append` does not take the writer lock).
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn the_dna_entry_is_first_also_when_an_append_comes_at_once() {
    for round in 0..40 {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        // Another process: it waits for the session and appends at once.
        let watcher = {
            let store = store.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Some(s) = store.sessions().ok().and_then(|v| v.into_iter().next()) {
                        return Some(store.queue_append(&s.id, "user", "from outside").unwrap());
                    }
                    std::hint::spin_loop();
                }
                None
            })
        };
        let rh = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
        stop.store(true, Ordering::Relaxed);
        let outside = watcher.join().unwrap();
        let inbox = store.inbox(&rh.session).unwrap();
        assert_eq!(inbox[0].role, "system", "round {round}: {inbox:?}");
        assert!(
            inbox[0].text.starts_with("DNA of this horse"),
            "round {round}: {inbox:?}"
        );
        if let Some(e) = outside {
            assert_eq!(inbox.len(), 2, "round {round}: {inbox:?}");
            assert_eq!(inbox[1].id, e.id, "round {round}");
        }
    }
}

/// The task of a spawned child goes out right after the DNA entry, also
/// when another process appended to the session before the run queued
/// the task (a hitched child's session is visible from its spawn on).
#[test]
fn the_task_goes_right_after_the_dna_entry_also_after_an_outside_append() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let parent = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    for rh in [
        begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap(),
        crate::horse::hitch::spawn_child(&store, &parent, dsh::HARNESS, dsh::VERSION).unwrap(),
    ] {
        store
            .queue_append(&rh.session, "user", "from outside")
            .unwrap();
        store.queue_append(&rh.session, "note", "second").unwrap();
        store.queue_task(&rh.session, "the task").unwrap();
        let inbox = store.inbox(&rh.session).unwrap();
        let texts: Vec<&str> = inbox.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(inbox[0].role, "system", "{texts:?}");
        assert_eq!(&texts[1..], ["the task", "from outside", "second"]);
        assert_eq!(inbox[1].role, "user");
    }
}

/// Without a pending start entry, the task goes first.
#[test]
fn the_task_goes_first_when_no_start_entry_waits() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let rh = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    for e in store.inbox(&rh.session).unwrap() {
        store
            .remove_ref(&format!("inbox/{}/{}.json", rh.session, e.id))
            .ok();
    }
    store
        .queue_append(&rh.session, "user", "from outside")
        .unwrap();
    store.queue_task(&rh.session, "the task").unwrap();
    let texts: Vec<String> = store
        .inbox(&rh.session)
        .unwrap()
        .into_iter()
        .map(|e| e.text)
        .collect();
    assert_eq!(texts, ["the task", "from outside"]);
}
