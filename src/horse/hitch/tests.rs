use super::*;
use crate::horse::{dsh, store::Origin};

#[test]
fn children_share_a_horse_and_only_results_reach_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let parent = run::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let origin = Origin {
        harness: "dsh",
        container: "test",
        version: "1",
    };
    let head = store.put_native(&origin, None, "seq:1", b"parent").unwrap();
    store
        .update_session(&parent.session, |s| s.head = Some(head.clone()))
        .unwrap();
    let a = spawn_child(&store, &parent, dsh::HARNESS, dsh::VERSION).unwrap();
    let b = spawn_child(&store, &parent, dsh::HARNESS, dsh::VERSION).unwrap();
    assert_eq!(store.horse(&parent.horse).unwrap().sessions.len(), 3);
    assert_eq!(a.horse, parent.horse);
    assert_eq!(b.horse, parent.horse);
    let child_head = store.put_native(&origin, None, "seq:2", b"child").unwrap();
    store
        .update_session(&a.session, |s| s.head = Some(child_head.clone()))
        .unwrap();
    let first = child_finished(&store, &a, &parent.session, "hello").unwrap();
    let second = child_finished(&store, &b, &parent.session, "world").unwrap();
    assert_eq!(
        first.text,
        format!("hitched {} (dsh) finished\nhello", &a.session[..8])
    );
    // The first entry is the DNA at the start of the session.
    assert_eq!(store.inbox(&parent.session).unwrap()[1..], [first, second]);
    assert_eq!(
        visible_sessions(&store, &parent.horse, &a.session),
        vec![a.session.clone()]
    );
    assert!(visible_sessions(&store, "other", &a.session).is_empty());
    let all = edges(&store, &parent.horse).unwrap();
    assert_eq!(
        all.iter().map(|e| &e.kind).collect::<Vec<_>>(),
        vec![
            &EdgeKind::SpawnedFrom,
            &EdgeKind::SpawnedFrom,
            &EdgeKind::Append,
            &EdgeKind::Append
        ]
    );
    assert_eq!(all[0].at, Some(head.clone()));
    assert_eq!(all[1].at, Some(head));
    assert_eq!(all[2].at, Some(child_head));
    assert_eq!(all[2].role.as_deref(), Some("note"));
    assert_eq!(all[0].to, parent.session);
    assert!(child_finished(&store, &a, &b.session, "no").is_err());
}
