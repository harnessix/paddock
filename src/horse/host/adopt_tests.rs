use super::adopt::{self, Locked};
use super::fixtures::*;
use super::*;

struct Host {
    _t: tempfile::TempDir,
    store: Store,
    home: PathBuf,
    state: PathBuf,
}

fn host() -> Host {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let state = t.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    Host {
        _t: t,
        store,
        home,
        state,
    }
}

#[test]
fn adoption_converts_then_reads_new_and_changed_sessions() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let before = tree(&h.home);
    let s = adopt::adopt(&h.store, &h.state, claude::HARNESS, &h.home, &[], None).unwrap();
    assert_eq!(s.imported.len(), 1);
    assert!(s.imported[0].new);
    assert_eq!(tree(&h.home), before);
    let a = adopt::load(&h.state).unwrap().harnesses[claude::HARNESS].clone();
    assert_eq!(a.locations, vec![PathBuf::from(".claude/projects")]);
    assert_eq!(a.home, canon_home(&h.home));
    assert_eq!(a.seen.len(), 1);
    assert_eq!(
        h.store.horse(&a.horse).unwrap().sessions,
        vec![s.imported[0].session.clone()]
    );
    let file = adopt::dir(&h.state).join("adoption.json");
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions())
            & 0o777,
        0o600
    );

    // Nothing changed: the sweep reads nothing.
    let w = adopt::sweep(&h.store, &h.state, None).unwrap();
    assert_eq!((w[0].imported.len(), w[0].unchanged), (0, 1));

    // A new session and new rows of the old one.
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 1));
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 4));
    let before = tree(&h.home);
    let w = adopt::sweep(&h.store, &h.state, Some(claude::HARNESS)).unwrap();
    assert_eq!(w[0].imported.len(), 2);
    assert_eq!(tree(&h.home), before);
    let b = w[0]
        .imported
        .iter()
        .find(|i| i.native_id == CLAUDE_B)
        .unwrap();
    assert!(b.new);
    assert_eq!(b.horse, a.horse);
    let old = w[0]
        .imported
        .iter()
        .find(|i| i.native_id == CLAUDE_A)
        .unwrap();
    assert_eq!((old.new, old.rows), (false, 4));
    assert_eq!(h.store.horse(&a.horse).unwrap().sessions.len(), 2);

    // Off: the state goes, the sessions stay, sweeps read nothing.
    let (gone, removed) = adopt::off(&h.state, claude::HARNESS).unwrap().unwrap();
    assert_eq!(removed, None);
    assert_eq!(gone.horse, a.horse);
    assert!(adopt::load(&h.state).unwrap().harnesses.is_empty());
    assert_eq!(h.store.horse(&a.horse).unwrap().sessions.len(), 2);
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 6));
    assert!(adopt::sweep(&h.store, &h.state, None).unwrap().is_empty());
    assert_eq!(
        h.store
            .session_nodes(&h.store.session(&old.session).unwrap())
            .unwrap()
            .len(),
        4
    );
    assert!(adopt::off(&h.state, claude::HARNESS).unwrap().is_none());
}

#[test]
fn adoption_watches_only_the_chosen_locations() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 1));
    let s = adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[PathBuf::from(".claude/projects/p")],
        None,
    )
    .unwrap();
    assert_eq!(s.imported.len(), 1);
    assert_eq!(s.imported[0].native_id, CLAUDE_A);
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 3));
    let w = adopt::sweep(&h.store, &h.state, None).unwrap();
    assert!(w[0].imported.is_empty());
    assert!(h
        .store
        .host_session(claude::HARNESS, CLAUDE_B)
        .unwrap()
        .is_none());
    // A second adopt adds a location to the same horse.
    let s2 = adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[h.home.join(".claude/projects/q")],
        None,
    )
    .unwrap();
    assert_eq!(s2.imported.len(), 1);
    assert_eq!(s2.imported[0].horse, s.imported[0].horse);
}

#[test]
fn adoption_refuses_other_places() {
    let h = host();
    for bad in [".codex/sessions", "../x", "/etc", ".claude"] {
        assert!(
            adopt::adopt(
                &h.store,
                &h.state,
                claude::HARNESS,
                &h.home,
                &[bad.into()],
                None
            )
            .is_err(),
            "{bad}"
        );
    }
    assert!(adopt::adopt(&h.store, &h.state, "opencode", &h.home, &[], None).is_err());
    assert!(adopt::load(&h.state).unwrap().harnesses.is_empty());
    adopt::adopt(&h.store, &h.state, claude::HARNESS, &h.home, &[], None).unwrap();
    let other = h.home.join("../other");
    std::fs::create_dir_all(&other).unwrap();
    let e = adopt::adopt(&h.store, &h.state, claude::HARNESS, &other, &[], None).unwrap_err();
    assert!(format!("{e:#}").contains("--off"), "{e:#}");
}

#[test]
fn a_session_that_cannot_be_read_is_read_again_only_when_it_changes() {
    let h = host();
    let old: Vec<String> = claude_rows(CLAUDE_B, 1)
        .iter()
        .map(|r| r.replace(claude::VERSION, "1.0.0"))
        .collect();
    write_claude(&h.home, "p", CLAUDE_B, &old);
    let s = adopt::adopt(&h.store, &h.state, claude::HARNESS, &h.home, &[], None).unwrap();
    assert_eq!(s.failed.len(), 1);
    let a = adopt::load(&h.state).unwrap().harnesses[claude::HARNESS].clone();
    assert_eq!(a.errors.len(), 1);
    let w = adopt::sweep(&h.store, &h.state, None).unwrap();
    assert_eq!((w[0].failed.len(), w[0].unchanged), (0, 1));
    write_claude(&h.home, "p", CLAUDE_B, &claude_rows(CLAUDE_B, 2));
    let w = adopt::sweep(&h.store, &h.state, None).unwrap();
    assert_eq!(w[0].imported.len(), 1);
    assert!(adopt::load(&h.state).unwrap().harnesses[claude::HARNESS]
        .errors
        .is_empty());
    // The lock is free again.
    drop(Locked::open(&h.state).unwrap());
}
