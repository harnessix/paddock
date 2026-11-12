//! FLAKE3 for ctl grants, memory names and views: a writer replaces the
//! file by a rename, and a listing runs next to it. On btrfs a listing
//! without the store lock does not see a file that is renamed at that
//! moment. tmpfs does not do this, so these tests can pass there anyway.

use super::{grant, view};
use crate::horse::device::{self, Grants, Tree};
use crate::horse::Store;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const N: usize = 6;
const ROUNDS: usize = 200;

fn store() -> (tempfile::TempDir, Store, String) {
    let dir = crate::horse::store::race_dir();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let horse = store.new_default_horse().unwrap().id;
    (dir, store, horse)
}

/// Run `write` ROUNDS times on another thread, and `count` here until it
/// ends. The number of counts that are not N.
fn misses(write: impl FnOnce() + Send + 'static, mut count: impl FnMut() -> usize) -> usize {
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let done = done.clone();
        std::thread::spawn(move || {
            write();
            done.store(true, Ordering::Release);
        })
    };
    let mut misses = 0;
    while !done.load(Ordering::Acquire) {
        misses += usize::from(count() != N);
    }
    writer.join().unwrap();
    misses
}

#[test]
fn the_grant_list_sees_every_grant_while_commands_spend() {
    let (_dir, st, _) = store();
    let sandboxes: Vec<String> = (0..N).map(|i| format!("sb-{i}")).collect();
    for s in &sandboxes {
        let g = grant::Grant {
            sandbox: s.clone(),
            commands: vec!["fork".into()],
            horses: Vec::new(),
            budget: u64::MAX,
            tokens: 0,
            used: 0,
            spent_tokens: 0,
            granted_ms: 0,
        };
        grant::set(&st, &g).unwrap();
    }
    let w = st.clone();
    let n = misses(
        move || {
            for _ in 0..ROUNDS {
                for s in &sandboxes {
                    assert!(grant::spend(&w, s, 0).unwrap());
                }
            }
        },
        || grant::list(&st).unwrap().len(),
    );
    assert_eq!(n, 0, "grant lists without every grant");
}

#[test]
fn the_tree_sees_every_memory_while_it_is_rewritten() {
    let (_dir, st, horse) = store();
    let names: Vec<String> = (0..N).map(|i| format!("m{i}")).collect();
    for m in &names {
        device::write_memory(&st, &horse, m, b"first").unwrap();
    }
    let (w, h) = (st.clone(), horse.clone());
    let n = misses(
        move || {
            for r in 0..ROUNDS {
                for m in &names {
                    device::write_memory(&w, &h, m, format!("round {r}").as_bytes()).unwrap();
                }
            }
        },
        || {
            let t = device::build(&st, &horse, &Grants::default()).unwrap();
            t.paths
                .keys()
                .filter(|p| p.starts_with("self/memory/"))
                .count()
        },
    );
    assert_eq!(n, 0, "trees without every memory");
}

#[test]
fn the_tree_sees_every_view_while_it_is_saved_again() {
    let (_dir, st, horse) = store();
    let b = crate::horse::claude::binding(None);
    let session = st.new_session(&horse, b).unwrap().id;
    let names: Vec<String> = (0..N).map(|i| format!("v{i}")).collect();
    let v = view::View {
        session,
        filter: view::Filter::All,
    };
    for name in &names {
        view::save(&st, &horse, name, &v).unwrap();
    }
    let h = st.horse(&horse).unwrap();
    let (w, id) = (st.clone(), horse.clone());
    let n = misses(
        move || {
            for _ in 0..ROUNDS {
                for name in &names {
                    view::save(&w, &id, name, &v).unwrap();
                }
            }
        },
        || {
            let mut t = Tree::default();
            view::add(&st, &h, &mut t).unwrap();
            t.paths
                .keys()
                .filter(|p| p.starts_with("self/views/"))
                .count()
        },
    );
    assert_eq!(n, 0, "trees without every view");
}
