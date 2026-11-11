//! A hitched child shares the horse of its parent, but its ctl commands
//! reach only its own session: never the parent's, never a sibling's.

use super::tests::{give_to, stub, world_parts};
use super::*;
use crate::horse::run::RunHorse;
use crate::horse::{dsh, hitch};

/// Every command but move, which needs a target horse.
const NO_MOVE: &[&str] = &["fork", "append", "view", "disconnect"];

#[test]
fn a_hitched_child_reaches_only_its_own_session() {
    let (tmp, store, horse, parent) = world_parts();
    let p = RunHorse {
        horse: horse.clone(),
        session: parent.clone(),
        harness: dsh::HARNESS.into(),
    };
    let a = hitch::spawn_child(&store, &p, dsh::HARNESS, dsh::VERSION).unwrap();
    let b = hitch::spawn_child(&store, &p, dsh::HARNESS, dsh::VERSION).unwrap();
    let child = Access {
        store: store.clone(),
        horse: horse.clone(),
        session: a.session.clone(),
    };
    give_to(&store, "sbx-child", NO_MOVE, 100, u64::MAX);
    let host = stub(Answer::Approved);
    for other in [&parent, &b.session] {
        let before = store.inbox(other).unwrap().len();
        for line in [
            format!("fork {other}"),
            format!("append {other} system hi"),
            format!("view {other} v all"),
            format!("disconnect {other}"),
        ] {
            let f = run(&child, "sbx-child", &line, &host).unwrap_err();
            assert_eq!(f.kind, Kind::Refused, "{line}");
            assert!(f.message.contains("not a session of this horse"), "{line}");
        }
        assert_eq!(store.inbox(other).unwrap().len(), before);
    }
    assert!(host.cards().is_empty());
    // Its own session works.
    let own = store.inbox(&a.session).unwrap().len();
    run(
        &child,
        "sbx-child",
        &format!("append {} note hi", a.session),
        &host,
    )
    .unwrap();
    assert_eq!(store.inbox(&a.session).unwrap().len(), own + 1);
    // The parent cannot reach into a child either: only results cross.
    let parent_access = Access {
        store: store.clone(),
        horse,
        session: parent,
    };
    give_to(&store, "sbx-parent", NO_MOVE, 100, u64::MAX);
    let f = run(
        &parent_access,
        "sbx-parent",
        &format!("view {} v all", b.session),
        &host,
    )
    .unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
    drop(tmp);
}
