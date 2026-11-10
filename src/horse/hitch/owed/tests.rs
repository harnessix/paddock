use super::*;
use crate::horse::hitch::wire;
use crate::horse::{dsh, run};

#[test]
fn a_hitched_result_is_owed_until_it_leaves_the_inbox() {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let parent = run::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let owed_now = || owed(&store, &parent.horse, &parent.session).unwrap();
    assert!(owed_now().is_empty());
    let a = wire::begin(&store, &parent, "claude").unwrap();
    let b = wire::begin(&store, &parent, "claude").unwrap();
    assert_eq!(owed_now(), [a.session.clone(), b.session.clone()]);
    let entry = wire::finish_once(&store, &a, "done").unwrap().unwrap();
    assert!(entry.text.starts_with(&result_head(&a.session)));
    // Queued is not enough: the result must leave the inbox.
    assert_eq!(owed_now(), [a.session.clone(), b.session.clone()]);
    store.record_delivery(&entry, None, "m-1").unwrap();
    assert_eq!(owed_now(), vec![b.session.clone()]);
    let other = run::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    assert!(owed(&store, &other.horse, &other.session)
        .unwrap()
        .is_empty());
}
