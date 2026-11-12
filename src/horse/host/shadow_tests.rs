//! The final ingest of a shadow run when something of the lineage fails,
//! a base from a real overlay plan, and a fork of a host session.

use super::fixtures::*;
use super::shadow::{self, Base};
use super::*;
use crate::horse::run::{self, final_only, snapshot::Snapshot};

struct Imported {
    _t: tempfile::TempDir,
    store: Store,
    host: PathBuf,
    lower: PathBuf,
    upper: PathBuf,
    horse: String,
    session: String,
}

/// A host session of 3 rows, imported into a horse of its own, and the
/// snapshot of a shadow run that has it.
fn imported() -> Imported {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let host = t.path().join("host-home");
    let lower = t.path().join("run/lower");
    let upper = t.path().join("run/upper");
    for d in [&host, &lower, &upper] {
        std::fs::create_dir_all(d).unwrap();
    }
    let host = canon_home(&host);
    let rows = claude_rows(CLAUDE_A, 3);
    write_claude(&host, "p", CLAUDE_A, &rows);
    write_claude(&lower, "p", CLAUDE_A, &rows);
    let found = list(&host, claude::HARNESS, None).unwrap();
    let r = import::import(
        &store,
        &t.path().join("staging"),
        claude::HARNESS,
        &host,
        &found,
        None,
        "import",
    )
    .unwrap();
    let (horse, session) = (r.imported[0].horse.clone(), r.imported[0].session.clone());
    Imported {
        _t: t,
        store,
        host,
        lower,
        upper,
        horse,
        session,
    }
}

impl Imported {
    fn base(&self) -> Base {
        Base {
            dir: self.lower.clone(),
            host_home: self.host.clone(),
        }
    }

    /// A run that continues the host session with 2 rows, to its end.
    fn run(&self) -> (run::RunHorse, Result<Vec<shadow::Linked>>) {
        self.run_and(|_| {})
    }

    /// `run`, with `during` called on the run before its end.
    fn run_and(
        &self,
        during: impl FnOnce(&run::RunHorse),
    ) -> (run::RunHorse, Result<Vec<shadow::Linked>>) {
        let before = Snapshot::capture(&self.upper, claude::HARNESS).unwrap();
        let rh = run::begin(&self.store, claude::HARNESS, claude::VERSION, None, None).unwrap();
        during(&rh);
        write_claude(&self.upper, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 5));
        let r = final_only::end_with(
            &self.store,
            &rh,
            "sb",
            &self.upper,
            &before,
            true,
            None,
            Some(&self.base()),
        );
        (rh, r)
    }
}

#[test]
fn a_lineage_that_cannot_be_recorded_does_not_stop_the_final_ingest() {
    let i = imported();
    // The host session points at a node that the store does not have.
    i.store
        .lock()
        .unwrap()
        .update_session(&i.session, |s| {
            s.head = Some(crate::horse::Hash::of(b"gone"))
        })
        .unwrap();
    let (rh, r) = i.run();
    let linked = r.expect("the final ingest goes on");
    assert!(linked.is_empty());
    let s = i.store.session(&rh.session).unwrap();
    assert_eq!(i.store.session_nodes(&s).unwrap().len(), 5);
    assert!(s.fork_of.is_none());
    assert_eq!(s.horse, rh.horse);
    let ev = run::events(&i.store, &rh.horse).unwrap();
    assert!(ev.iter().any(|e| e["type"] == "run.end"));
}

#[test]
fn a_host_session_that_fails_half_way_is_taken_out_again() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let base_dir = t.path().join("lower");
    write_claude(&base_dir, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let base = Base {
        dir: base_dir.clone(),
        host_home: t.path().join("host"),
    };
    let horse = store.new_default_horse().unwrap();
    // The rows cannot go into the store: the session is made, its rows fail.
    let objects = store.root().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o500)).unwrap();
    // Root writes into a folder of mode 0500 all the same, so the rows do
    // not fail and the test would prove nothing. Only then it stops.
    let probe = objects.join(".probe");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: geteuid has no preconditions.
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "a folder of mode 0500 took a write, and the test does not run as root"
        );
        eprintln!(
            "SKIPPED a_host_session_that_fails_half_way_is_taken_out_again: it runs as root, \
             and root writes into the read-only objects folder, so no row fails. Run it as a normal user."
        );
        return;
    }
    let rel = claude_rel("p", CLAUDE_A);
    let r = shadow::host_parent(
        &store.lock().unwrap(),
        claude::HARNESS,
        claude::VERSION,
        &horse.id,
        &base,
        &rel,
        &base_dir.join(&rel),
    );
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(r.is_err());
    assert!(store
        .host_session_at(claude::HARNESS, CLAUDE_A, &base.host_home)
        .unwrap()
        .is_none());
    assert!(store.horse(&horse.id).unwrap().sessions.is_empty());
    assert!(store.sessions().unwrap().is_empty());
    // The failure came after the host session was made, not before: its
    // index entry was written (it comes first), and the discard took it
    // out again.
    let ix: serde_json::Value =
        serde_json::from_slice(&store.read_ref("host/index.json").unwrap().expect(
            "the host session was made before its rows failed: its index entry was written",
        ))
        .unwrap();
    assert_eq!(ix["sessions"], serde_json::json!({}), "{ix}");
}

#[test]
fn a_second_run_of_the_same_host_session_joins_with_a_session_of_its_own() {
    let i = imported();
    let (a, r) = i.run();
    r.unwrap();
    // The next run continues the host session again, from the same base.
    let (b, r) = i.run();
    let linked = r.unwrap();
    assert_eq!(linked.len(), 1);
    assert_ne!(a.session, b.session);
    for rh in [&a, &b] {
        let s = i.store.session(&rh.session).unwrap();
        assert_eq!(s.horse, i.horse);
        assert_eq!(s.fork_of.unwrap().session, i.session);
    }
    assert_eq!(i.store.horse(&i.horse).unwrap().sessions.len(), 3);
}

#[test]
fn a_base_that_paddock_cannot_make_does_not_stop_the_run() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    std::fs::create_dir_all(&env.home).unwrap();
    let plan = crate::shadow::Plan::create_kind(
        &env,
        "sb1",
        vec![".claude".into()],
        vec![],
        crate::shadow::SnapKind::Clone,
    )
    .unwrap();
    // The transcript folder of the clone is a symlink.
    let elsewhere = t.path().join("elsewhere");
    write_claude(&elsewhere, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    std::fs::create_dir_all(plan.home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(
        elsewhere.join(".claude/projects"),
        plan.home.join(".claude/projects"),
    )
    .unwrap();
    assert!(Base::prepare(&plan, claude::HARNESS, &env.home).is_none());
}

#[test]
fn a_real_overlay_plan_gives_the_lineage() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    std::fs::create_dir_all(&env.home).unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let plan = crate::shadow::Plan::create_kind(
        &env,
        "sb3",
        vec![".claude".into()],
        vec![],
        crate::shadow::SnapKind::Overlay,
    )
    .unwrap();
    let lower = plan.lower.clone().unwrap();
    // The snapshot has the host session; the run writes into the upper
    // folder, which is the sandbox home.
    write_claude(&lower, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    // A copy of the home was imported before (`paddock horse import
    // --home`): the same native id, another host. The run continues the
    // session of the real home, not the one of the copy.
    let copy = t.path().join("home-copy");
    write_claude(&copy, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let found = list(&copy, claude::HARNESS, None).unwrap();
    let of_copy = import::import(
        &store,
        &t.path().join("staging"),
        claude::HARNESS,
        &copy,
        &found,
        None,
        "import",
    )
    .unwrap()
    .imported[0]
        .session
        .clone();
    // The home reaches the run by another path (a symlink): the base has
    // the real one, so it is the same host.
    let link = t.path().join("home-link");
    std::os::unix::fs::symlink(&env.home, &link).unwrap();
    let base = Base::prepare(&plan, claude::HARNESS, &link).unwrap();
    assert_eq!(base.dir, lower);
    assert_eq!(base.host_home, canon_home(&env.home));
    let before = Snapshot::capture(&plan.home, claude::HARNESS).unwrap();
    let rh = run::begin(&store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    write_claude(&plan.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 3));
    let lower_before = tree(&lower);
    let linked = final_only::end_with(
        &store,
        &rh,
        "sb3",
        &plan.home,
        &before,
        true,
        None,
        Some(&base),
    )
    .unwrap();
    assert_eq!((linked[0].kept, linked[0].added), (2, 1));
    assert_eq!(tree(&lower), lower_before);
    let host = store
        .host_session_at(claude::HARNESS, CLAUDE_A, &base.host_home)
        .unwrap()
        .unwrap();
    assert_eq!(linked[0].host_session, host.id);
    assert_ne!(linked[0].host_session, of_copy);
    assert_eq!(host.host.as_ref().unwrap().home, canon_home(&env.home));
    assert_eq!(host.host.unwrap().via, "shadow");
    let s = store.session(&rh.session).unwrap();
    assert_eq!(s.fork_of.unwrap().session, host.id);
    // The session of the copy did not change.
    assert_eq!(
        store
            .session_nodes(&store.session(&of_copy).unwrap())
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_host_session_forks_into_a_session_of_its_own() {
    let i = imported();
    let host = i.store.session(&i.session).unwrap();
    let nodes = i.store.session_nodes(&host).unwrap();
    let f = i.store.fork(&i.session, &nodes[1].hash, None).unwrap();
    assert!(f.host.is_none());
    assert_eq!(f.horse, i.horse);
    assert_eq!(f.fork_of.as_ref().unwrap().session, i.session);
    assert_eq!(i.store.session_nodes(&f).unwrap().len(), 2);
    // The host session did not change, and it is still the host session.
    assert_eq!(i.store.session(&i.session).unwrap(), host);
    assert_eq!(
        i.store
            .host_session(claude::HARNESS, CLAUDE_A)
            .unwrap()
            .unwrap()
            .id,
        i.session
    );
    // Run code never takes the fork for the host session, or the reverse.
    assert!(i
        .store
        .session_by_native(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .is_none());

    // A copy of the home has the same native id. Its import is another
    // host session, and a fork of it forks that one, not the first.
    let copy = i._t.path().join("home-copy");
    write_claude(&copy, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 4));
    let found = list(&copy, claude::HARNESS, None).unwrap();
    let r = import::import(
        &i.store,
        &i._t.path().join("staging"),
        claude::HARNESS,
        &copy,
        &found,
        None,
        "import",
    )
    .unwrap();
    assert!(r.imported[0].new);
    let other = r.imported[0].session.clone();
    assert_ne!(other, i.session);
    assert_eq!(i.store.session(&i.session).unwrap(), host);
    let other_s = i.store.session(&other).unwrap();
    let other_nodes = i.store.session_nodes(&other_s).unwrap();
    assert_eq!(other_nodes.len(), 4);
    let g = i.store.fork(&other, &other_nodes[2].hash, None).unwrap();
    assert!(g.host.is_none());
    assert_eq!(g.fork_of.as_ref().unwrap().session, other);
    assert_eq!(i.store.session_nodes(&g).unwrap().len(), 3);
    // Each home finds its own host session; the forks are none of them.
    for (home, id) in [(&i.host, &i.session), (&canon_home(&copy), &other)] {
        assert_eq!(
            &i.store
                .host_session_at(claude::HARNESS, CLAUDE_A, home)
                .unwrap()
                .unwrap()
                .id,
            id
        );
    }
    // Without a home, the native id names two host sessions now.
    assert!(i.store.host_session(claude::HARNESS, CLAUDE_A).is_err());
}

#[test]
fn a_continuation_with_hitch_edges_stays_in_the_horse_of_its_run() {
    let i = imported();
    // The run hitches a child: the edge is in the run's horse.
    let mut child = None;
    let (rh, r) = i.run_and(|rh| {
        child = Some(
            crate::horse::hitch::spawn_child(&i.store, rh, claude::HARNESS, claude::VERSION)
                .unwrap(),
        );
    });
    let child = child.unwrap();
    let linked = r.expect("the final ingest goes on");
    assert_eq!(linked.len(), 1);
    // The checks of a move hold for the join too: the session stays.
    assert_eq!(linked[0].joins, None);
    let s = i.store.session(&rh.session).unwrap();
    assert_eq!(s.horse, rh.horse);
    assert!(i
        .store
        .horse(&rh.horse)
        .unwrap()
        .sessions
        .contains(&rh.session));
    assert!(!i
        .store
        .horse(&i.horse)
        .unwrap()
        .sessions
        .contains(&rh.session));
    // Its lineage to the host session is recorded all the same.
    assert_eq!(s.fork_of.unwrap().session, i.session);
    assert_eq!(s.origin_run.as_deref(), Some("sb"));
    // The edge still joins two sessions of one horse.
    let edges = crate::horse::hitch::edges(&i.store, &rh.horse).unwrap();
    assert!(edges
        .iter()
        .any(|e| e.from == child.session && e.to == rh.session));
    assert_eq!(i.store.session(&child.session).unwrap().horse, rh.horse);
    let ev = run::events(&i.store, &rh.horse).unwrap();
    let stay = ev
        .iter()
        .find(|e| e["type"] == "host.stay")
        .expect("a host.stay event");
    assert!(
        stay["why"].as_str().unwrap().contains("hitch edges"),
        "{stay}"
    );
    assert!(!ev.iter().any(|e| e["type"] == "session.move"));
}
