use super::*;
use crate::horse::run;

fn parent_and_child(store: &Store) -> (RunHorse, Hitched) {
    let parent = run::begin(store, codex::HARNESS, codex::VERSION, None, None).unwrap();
    let child = begin(store, &parent, codex::HARNESS).unwrap();
    (parent, child)
}

#[test]
fn begin_makes_a_session_in_the_parents_horse_with_a_spawned_from_edge() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (parent, child) = parent_and_child(&store);
    assert_eq!(child.horse, parent.horse);
    assert_ne!(child.session, parent.session);
    assert_eq!(child.parent_session, parent.session);
    assert_eq!(child.harness, "codex");
    assert_eq!(store.horse(&parent.horse).unwrap().sessions.len(), 2);
    let all = edges(&store, &parent.horse).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].kind, EdgeKind::SpawnedFrom);
    assert_eq!(
        (&all[0].from, &all[0].to),
        (&child.session, &parent.session)
    );
    assert!(begin(&store, &parent, "fake").is_err());
}

#[test]
fn finish_once_queues_one_append_and_a_second_call_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (parent, child) = parent_and_child(&store);
    let first = finish_once(&store, &child, "hello").unwrap().unwrap();
    assert_eq!(
        first.text,
        format!("hitched {} (codex) finished\nhello", &child.session[..8])
    );
    assert!(finish_once(&store, &child, "later").unwrap().is_none());
    // The first entry is the DNA at the start of the session.
    assert_eq!(store.inbox(&parent.session).unwrap()[1..], [first]);
    let kinds: Vec<_> = edges(&store, &parent.horse)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, vec![EdgeKind::SpawnedFrom, EdgeKind::Append]);
}

#[test]
fn link_of_finds_the_parent_from_the_edge_and_done_reports_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (parent, child) = parent_and_child(&store);
    let access = |session: &str| Access {
        store: store.clone(),
        horse: parent.horse.clone(),
        session: session.into(),
    };
    assert_eq!(link_of(&access(&parent.session)).unwrap(), None);
    assert_eq!(
        link_of(&access(&child.session)).unwrap(),
        Some(child.clone())
    );
    assert_eq!(parent_run(&access(&parent.session)).unwrap(), parent);
    assert!(report_done(&access(&child.session), "failed", "no luck"));
    assert!(report_done(&access(&child.session), "ok", "again"));
    assert!(!report_done(&access(&parent.session), "ok", "not hitched"));
    let inbox = store.inbox(&parent.session).unwrap();
    assert_eq!(inbox.len(), 2);
    assert!(
        inbox[1].text.ends_with("\nfailed: no luck"),
        "{}",
        inbox[0].text
    );
}
