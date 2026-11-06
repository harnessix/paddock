use super::*;

#[test]
fn borrow_and_return_through_the_broker() {
    let (t, b, fixed) = setup(Some(Answer::Once), HostPolicy::default());
    let o = t.path().join("shared/lib");
    std::fs::create_dir_all(o.join("src")).unwrap();
    std::fs::write(o.join("src/a.rs"), "old\n").unwrap();
    let r = b.handle(&req("borrow", serde_json::json!({ "path": o })));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_eq!(r.result["path"], "/run/paddock/borrow/lib");
    let copy = crate::borrow::dir(&b.env, "t-1").join("lib");
    std::fs::write(copy.join("src/a.rs"), "new\n").unwrap();
    std::fs::write(copy.join("added"), "x").unwrap();
    // The original does not change before the return.
    assert_eq!(
        std::fs::read_to_string(o.join("src/a.rs")).unwrap(),
        "old\n"
    );
    let r = b.handle(&req("return", serde_json::json!({ "name": "lib" })));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_eq!(r.result["added"], 1);
    assert_eq!(r.result["changed"], 1);
    assert_eq!(
        std::fs::read_to_string(o.join("src/a.rs")).unwrap(),
        "new\n"
    );
    assert_eq!(std::fs::read_to_string(o.join("added")).unwrap(), "x");
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 2);

    // A change to the original after the borrow: the return refuses.
    let r = b.handle(&req("borrow", serde_json::json!({ "path": o })));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    std::fs::write(copy.join("src/a.rs"), "agent\n").unwrap();
    std::fs::write(o.join("src/a.rs"), "user\n").unwrap();
    let r = b.handle(&req("return", serde_json::json!({ "name": "lib" })));
    assert_eq!(r.status, Status::Invalid, "{}", r.message);
    assert!(
        r.message.contains("changed after the borrow"),
        "{}",
        r.message
    );
    assert_eq!(
        std::fs::read_to_string(o.join("src/a.rs")).unwrap(),
        "user\n"
    );
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 3);

    // The audit log keeps paths and counts, never content.
    let log = std::fs::read_to_string(audit_file(&b.env)).unwrap();
    assert!(log.contains("1 added, 1 changed, 0 deleted"), "{log}");
    assert!(!log.contains("agent") && !log.contains("new\\n"), "{log}");
}

/// A Seatbelt run sees the folder at its host path: the result, the cards
/// and the copies use it.
#[test]
fn a_seatbelt_run_borrows_into_the_folder_that_it_sees() {
    let (t, mut b, fixed) = setup(Some(Answer::Once), HostPolicy::default());
    let seen = crate::borrow::dir(&b.env, "t-1");
    b.borrow_dir = seen.clone();
    let o = t.path().join("shared/lib");
    std::fs::create_dir_all(&o).unwrap();
    std::fs::write(o.join("a"), "old").unwrap();
    let r = b.handle(&req("borrow", serde_json::json!({ "path": o })));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_eq!(r.result["path"], seen.join("lib").display().to_string());
    std::fs::write(seen.join("lib/a"), "new").unwrap();
    let r = b.handle(&req("return", serde_json::json!({ "name": "lib" })));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_eq!(std::fs::read_to_string(o.join("a")).unwrap(), "new");
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 2);
    // The audit log names the host folder, not /run/paddock/borrow.
    let log = std::fs::read_to_string(audit_file(&b.env)).unwrap();
    assert!(!log.contains(crate::borrow::MOUNT), "{log}");
}

#[test]
fn a_refused_capability_is_off_with_one_line() {
    let (_t, mut b, fixed) = setup(Some(Answer::Once), HostPolicy::default());
    b.refused = vec![
        ("borrow", "borrow is not there on this backend"),
        ("return", "return is not there on this backend"),
    ];
    for cap in ["borrow", "return"] {
        let r = b.handle(&req(cap, serde_json::json!({ "path": "/x", "name": "x" })));
        assert_eq!(r.status, Status::Disabled, "{cap}");
        assert!(
            r.message.contains("not there on this backend"),
            "{}",
            r.message
        );
        assert_eq!(b.mode(cap), crate::broker::Mode::Deny);
    }
    // Nobody was asked.
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 0);
}
