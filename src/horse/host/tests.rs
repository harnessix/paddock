use super::fixtures::*;
use super::shadow::Base;
use super::*;
use crate::horse::run::{self, final_only, snapshot::Snapshot};

struct Shadow {
    _t: tempfile::TempDir,
    store: Store,
    /// The host home that the snapshot came from (a fixture).
    host: PathBuf,
    /// The base: the `lower` folder of an overlay.
    lower: PathBuf,
    /// The sandbox home (`upper`).
    upper: PathBuf,
}

fn shadow() -> Shadow {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let host = t.path().join("host-home");
    let lower = t.path().join("run/lower");
    let upper = t.path().join("run/upper");
    for d in [&host, &lower, &upper] {
        std::fs::create_dir_all(d).unwrap();
    }
    Shadow {
        _t: t,
        store,
        host,
        lower,
        upper,
    }
}

impl Shadow {
    fn base(&self) -> Base {
        Base {
            dir: self.lower.clone(),
            host_home: self.host.clone(),
        }
    }
}

fn rows(st: &Store, id: &str) -> Vec<Vec<u8>> {
    st.session_nodes(&st.session(id).unwrap())
        .unwrap()
        .into_iter()
        .map(|n| n.raw)
        .collect()
}

fn as_bytes(v: &[String]) -> Vec<Vec<u8>> {
    v.iter().map(|r| r.as_bytes().to_vec()).collect()
}

#[test]
fn a_shadow_run_that_continues_a_host_session_is_its_fork() {
    let s = shadow();
    let host_rows = claude_rows(CLAUDE_A, 3);
    write_claude(&s.host, "p", CLAUDE_A, &host_rows);
    // The snapshot (lower) is a copy of the host data.
    write_claude(&s.lower, "p", CLAUDE_A, &host_rows);
    let host_before = tree(&s.host);
    let lower_before = tree(&s.lower);
    let before = Snapshot::capture(&s.upper, claude::HARNESS).unwrap();
    let rh = run::begin(&s.store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    // The run resumes the session: the overlay copies the file up and the
    // harness appends two rows.
    let run_rows = claude_rows(CLAUDE_A, 5);
    write_claude(&s.upper, "p", CLAUDE_A, &run_rows);

    let linked = final_only::end_with(
        &s.store,
        &rh,
        "sb",
        &s.upper,
        &before,
        true,
        None,
        Some(&s.base()),
    )
    .unwrap();

    let host = s
        .store
        .host_session(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .expect("a host session");
    let origin = host.host.clone().unwrap();
    assert_eq!(origin.via, "shadow");
    assert_eq!(origin.home, s.host);
    assert_eq!(origin.path, claude_rel("p", CLAUDE_A));
    // The host session has the host rows only, the run's session all rows.
    assert_eq!(rows(&s.store, &host.id), as_bytes(&host_rows));
    assert_eq!(rows(&s.store, &rh.session), as_bytes(&run_rows));
    let run_s = s.store.session(&rh.session).unwrap();
    let fork = run_s.fork_of.clone().expect("a fork of the host session");
    assert_eq!(fork.session, host.id);
    assert_eq!(Some(&fork.node), host.head.as_ref());
    // One horse: the run's.
    assert_eq!(host.horse, rh.horse);
    assert_eq!(
        s.store.horse(&rh.horse).unwrap().sessions,
        vec![rh.session.clone(), host.id.clone()]
    );
    assert_eq!(
        linked,
        vec![shadow::Linked {
            session: rh.session.clone(),
            host_session: host.id.clone(),
            harness: claude::HARNESS.into(),
            native_id: CLAUDE_A.into(),
            kept: 3,
            added: 2,
            joins: None,
        }]
    );
    // The rows after the fork node are the run's.
    assert_eq!(run_s.origin_run.as_deref(), Some("sb"));
    assert!(linked[0]
        .line()
        .contains("the host history has none of them"));
    // Run code finds the run's session by the native id, never the host's.
    assert_eq!(
        s.store
            .session_by_native(claude::HARNESS, CLAUDE_A)
            .unwrap()
            .unwrap()
            .id,
        rh.session
    );
    // The host data and the snapshot did not change.
    assert_eq!(tree(&s.host), host_before);
    assert_eq!(tree(&s.lower), lower_before);
    let ev = run::events(&s.store, &rh.horse).unwrap();
    let cont: Vec<_> = ev.iter().filter(|e| e["type"] == "host.continue").collect();
    assert_eq!(cont.len(), 1);
    assert_eq!(cont[0]["kept"], 3);
    assert_eq!(cont[0]["added"], 2);
}

#[test]
fn a_host_session_in_the_store_keeps_its_rows_and_horse() {
    let s = shadow();
    let host_rows = claude_rows(CLAUDE_A, 4);
    write_claude(&s.host, "p", CLAUDE_A, &host_rows);
    write_claude(&s.lower, "p", CLAUDE_A, &host_rows);
    // The host session came in before, into a horse of its own.
    let adopted = s.store.new_default_horse().unwrap();
    let origin = HostOrigin {
        via: "import".into(),
        home: s.host.clone(),
        path: claude_rel("p", CLAUDE_A),
        read_ms: 1,
    };
    let stage = Staging::new(&s.host.join("../stage")).unwrap();
    let found = &list(&s.host, claude::HARNESS, None).unwrap()[0];
    let copy = stage.copy(&s.host, found).unwrap();
    let host_id = ingest_locked(
        &s.store.lock().unwrap(),
        claude::HARNESS,
        &copy,
        claude::VERSION,
        &adopted.id,
        &origin,
    )
    .unwrap();
    let host_head = s.store.session(&host_id).unwrap().head;

    let before = Snapshot::capture(&s.upper, claude::HARNESS).unwrap();
    let rh = run::begin(&s.store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    let run_rows = claude_rows(CLAUDE_A, 6);
    write_claude(&s.upper, "p", CLAUDE_A, &run_rows);
    let linked = final_only::end_with(
        &s.store,
        &rh,
        "sb",
        &s.upper,
        &before,
        true,
        None,
        Some(&s.base()),
    )
    .unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!((linked[0].kept, linked[0].added), (4, 2));
    let host = s.store.session(&host_id).unwrap();
    assert_eq!(host.head, host_head);
    assert_eq!(host.horse, adopted.id);
    assert_eq!(host.host.unwrap().via, "import");
    assert_eq!(rows(&s.store, &host_id), as_bytes(&host_rows));
    // Decision of the owner (2026-11-12): the run's session joins the
    // horse of the host session, and its rows carry the run as origin.
    assert_eq!(linked[0].joins.as_deref(), Some(adopted.id.as_str()));
    assert!(linked[0]
        .line()
        .contains(&format!("joins horse {}", adopted.id)));
    let run_s = s.store.session(&rh.session).unwrap();
    assert_eq!(run_s.horse, adopted.id);
    assert_eq!(run_s.origin_run.as_deref(), Some("sb"));
    assert_eq!(rows(&s.store, &rh.session), as_bytes(&run_rows));
    assert_eq!(run_s.fork_of.unwrap().session, host_id);
    // The lineage starts with the horse of the host session, closed, then
    // the run's horse, closed, then the host horse again.
    let steps: Vec<(&str, bool)> = run_s
        .lineage
        .iter()
        .map(|l| (l.horse.as_str(), l.until_ms.is_some()))
        .collect();
    assert_eq!(
        steps,
        vec![
            (adopted.id.as_str(), true),
            (rh.horse.as_str(), true),
            (adopted.id.as_str(), false)
        ]
    );
    // Both horses have the events.
    for h in [&adopted.id, &rh.horse] {
        let ev = run::events(&s.store, h).unwrap();
        let cont = ev
            .iter()
            .find(|e| e["type"] == "host.continue")
            .unwrap_or_else(|| panic!("{h}"));
        assert_eq!(cont["run"], "sb");
        assert!(ev.iter().any(|e| e["type"] == "session.move"), "{h}");
    }
    // The run's horse records the end of the run before the move.
    let ev = run::events(&s.store, &rh.horse).unwrap();
    assert!(ev.iter().any(|e| e["type"] == "run.end"));
    assert_eq!(
        s.store.horse(&adopted.id).unwrap().sessions,
        vec![host_id.clone(), rh.session.clone()]
    );
    assert!(!s
        .store
        .horse(&rh.horse)
        .unwrap()
        .sessions
        .contains(&rh.session));
    // Run code still finds the run's session by its native id.
    assert_eq!(
        s.store
            .session_by_native(claude::HARNESS, CLAUDE_A)
            .unwrap()
            .unwrap()
            .id,
        rh.session
    );
}

#[test]
fn a_new_session_of_a_shadow_run_has_no_host_parent() {
    let s = shadow();
    write_claude(&s.lower, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let before = Snapshot::capture(&s.upper, claude::HARNESS).unwrap();
    let rh = run::begin(&s.store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    write_claude(&s.upper, "p", CLAUDE_B, &claude_rows(CLAUDE_B, 2));
    let linked = final_only::end_with(
        &s.store,
        &rh,
        "sb",
        &s.upper,
        &before,
        true,
        None,
        Some(&s.base()),
    )
    .unwrap();
    assert!(linked.is_empty());
    assert!(s
        .store
        .host_session(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .is_none());
    let run_s = s.store.session(&rh.session).unwrap();
    assert!(run_s.fork_of.is_none());
    assert_eq!(run_s.binding.native_id.as_deref(), Some(CLAUDE_B));
}

#[test]
fn a_shadow_codex_run_forks_the_host_rollout() {
    let s = shadow();
    let host_rows = codex_rows(CODEX_A, 2);
    write_codex(&s.lower, CODEX_A, &host_rows);
    let before = Snapshot::capture(&s.upper, codex::HARNESS).unwrap();
    let rh = run::begin(&s.store, codex::HARNESS, codex::VERSION, None, None).unwrap();
    let run_rows = codex_rows(CODEX_A, 5);
    write_codex(&s.upper, CODEX_A, &run_rows);
    let linked = final_only::end_with(
        &s.store,
        &rh,
        "sb",
        &s.upper,
        &before,
        true,
        None,
        Some(&s.base()),
    )
    .unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!((linked[0].kept, linked[0].added), (3, 3));
    let host = s
        .store
        .host_session(codex::HARNESS, CODEX_A)
        .unwrap()
        .unwrap();
    assert_eq!(rows(&s.store, &host.id), as_bytes(&host_rows));
    assert_eq!(rows(&s.store, &rh.session), as_bytes(&run_rows));
}

#[test]
fn a_clone_base_is_a_copy_from_before_the_run() {
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
    let rows3 = claude_rows(CLAUDE_A, 3);
    write_claude(&plan.home, "p", CLAUDE_A, &rows3);
    let base = Base::prepare(&plan, claude::HARNESS, &env.home).unwrap();
    assert!(base.dir.starts_with(&plan.dir));
    // The run changes its clone; the base keeps the rows from before.
    write_claude(&plan.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 5));
    let copy = base.session(&claude_rel("p", CLAUDE_A)).unwrap();
    let mut want = rows3.join("\n");
    want.push('\n');
    assert_eq!(std::fs::read_to_string(copy).unwrap(), want);
    assert!(base.session(&claude_rel("p", CLAUDE_B)).is_none());
    // An overlay keeps its lower folder.
    let o = crate::shadow::Plan::create_kind(
        &env,
        "sb2",
        vec![".claude".into()],
        vec![],
        crate::shadow::SnapKind::Overlay,
    )
    .unwrap();
    assert_eq!(
        Base::prepare(&o, claude::HARNESS, &env.home).unwrap().dir,
        o.lower.clone().unwrap()
    );
}
