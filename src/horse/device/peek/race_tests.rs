//! FLAKE3 for peek grants: a read rewrites its grant by a rename, and the
//! listings of `active` and `revoke` run next to it. On btrfs a listing
//! without the store lock does not see a grant that is renamed at that
//! moment. tmpfs does not do this, so these tests can pass there anyway.

use super::grant::{self, active, record, revoke};
use crate::horse::Store;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const SANDBOX: &str = "sb-race";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    horse: String,
    session: String,
}

fn world() -> World {
    let dir = crate::horse::store::race_dir();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let horse = store.new_default_horse().unwrap().id;
    let b = crate::horse::claude::binding(None);
    let session = store.new_session(&horse, b).unwrap().id;
    World {
        _dir: dir,
        store,
        horse,
        session,
    }
}

impl World {
    fn issue(&self, sandbox: &str, id: &str) {
        let expires = crate::horse::now_ms() + DAY_MS;
        grant::issue_trainer(
            &self.store,
            sandbox,
            id,
            &self.horse,
            &self.session,
            expires,
            1 << 20,
        )
        .unwrap();
    }

    /// One read of grant `id`, as a peek read records it. A revoked grant
    /// or one at its read limit records nothing.
    fn read(&self, sandbox: &str, id: &str) {
        let guard = grant::lock(&self.store, sandbox, id).unwrap();
        let _ = record(
            &self.store,
            &guard,
            sandbox,
            id,
            &self.horse,
            &self.session,
            "ls",
            Vec::new(),
            0,
            "done",
        );
    }
}

#[test]
fn active_sees_every_grant_while_reads_rewrite_them() {
    let w = Arc::new(world());
    let ids: Vec<String> = (0..16).map(|i| format!("g{i}")).collect();
    for id in &ids {
        w.issue(SANDBOX, id);
    }
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (w, done, ids) = (w.clone(), done.clone(), ids.clone());
        std::thread::spawn(move || {
            // Below the limit of 120 reads per minute of each grant.
            for _ in 0..100 {
                for id in &ids {
                    w.read(SANDBOX, id);
                }
            }
            done.store(true, Ordering::Release);
        })
    };
    let mut misses = 0;
    while !done.load(Ordering::Acquire) {
        misses += usize::from(active(&w.store, SANDBOX).unwrap().len() != ids.len());
    }
    writer.join().unwrap();
    assert_eq!(misses, 0, "listings of active() without every grant");
}

/// Each round issues 8 grants again and revokes them while reads rewrite
/// them. Files that are not grants make the folder long, so a listing
/// takes long enough to meet a rename. `revoke` passes over them.
#[test]
fn revoke_reaches_every_grant_while_reads_rewrite_them() {
    let w = Arc::new(world());
    let ids: Vec<String> = (0..8).map(|i| format!("g{i}")).collect();
    let folder = w.store.root().join("peek/grants").join(SANDBOX);
    crate::privfs::dir(&folder).unwrap();
    for i in 0..4000 {
        std::fs::write(folder.join(format!("pad-{i}")), b"").unwrap();
    }
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (w, done, ids) = (w.clone(), done.clone(), ids.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                for id in &ids {
                    w.read(SANDBOX, id);
                }
            }
        })
    };
    let mut skipped = Vec::new();
    for round in 0..100 {
        for id in &ids {
            let _ = std::fs::remove_file(folder.join(format!("{id}.json")));
            w.issue(SANDBOX, id);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        revoke(&w.store, SANDBOX).unwrap();
        for id in &ids {
            if !grant::get(&w.store, SANDBOX, id).unwrap().unwrap().revoked {
                skipped.push(format!("round {round}: {id}"));
            }
        }
    }
    done.store(true, Ordering::Release);
    writer.join().unwrap();
    assert!(skipped.is_empty(), "revoke skipped grants: {skipped:?}");
}
