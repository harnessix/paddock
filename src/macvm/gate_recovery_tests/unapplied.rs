//! A human allow without a wake fence: denied once, no rule, a reason.
use super::*;

#[test]
fn an_unverified_allow_is_denied_once_without_a_rule() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Ask);
    let asker = fw.asker().unwrap();
    asker.set_wake_fence_for_test(false); // As in production.
    let key = triple(1);
    let Join::Wait(q, false) = asker.join(&key, Who::default(), None, 10) else {
        panic!("the question must open");
    };
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowAlways).unwrap();
    fw.settle(11);
    let s = q.result().expect("the question is closed");
    assert!(!s.allowed);
    assert_eq!(s.word, broker::UNVERIFIED_RULE);
    assert!(fw.rules().file.is_empty());
    assert!(fw.rules().session.is_empty());
    assert_eq!(asker.waiting(), 0);
    let status = ask_status::read(&fw.status_path());
    assert_eq!(status.unapplied.len(), 1);
    assert_eq!(status.unapplied[0].target, "host-1.example:443");
}

/// A denial grants no access, so it applies in any wake order.
#[test]
fn an_unfenced_deny_always_writes_its_rule() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Ask);
    let asker = fw.asker().unwrap();
    asker.set_wake_fence_for_test(false); // As in production.
    let key = triple(2);
    let who = Who {
        app: key.app.clone(),
        ..Default::default()
    };
    let Join::Wait(q, false) = asker.join(&key, who, None, 10) else {
        panic!("the question must open");
    };
    broker::answer_connect(&env, &q.id, ConnectAnswer::DenyAlways).unwrap();
    fw.settle(11);
    let s = q.result().expect("the question is closed");
    assert!(!s.allowed);
    assert_eq!(s.answer, ConnectAnswer::DenyAlways);
    assert!(s.word.starts_with("rule-"), "{}", s.word);
    let file = fw.rules().file.clone();
    assert_eq!(file.len(), 1);
    assert_eq!(file[0].host, key.host);
    assert_eq!(file[0].verdict, firewall::Verdict::Deny);
    assert!(fw.rules().session.is_empty());
    assert_eq!(asker.waiting(), 0);
    assert!(ask_status::read(&fw.status_path()).unapplied.is_empty());
}
