use super::fake_state::{add, card, fixture, notice, notice_card};
use super::*;

#[test]
fn notice_identity_survives_rewrite_and_external_selection() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let before = notice_card(&a, &env);
    add(&a, "second", 103);
    let after = notice_card(&a, &env);
    assert_ne!(before.id, after.id);
    assert_eq!(before.created, after.created);
    assert_eq!(
        before.connect.as_ref().unwrap().asked_at,
        after.connect.as_ref().unwrap().asked_at
    );
    assert_ne!(
        before.connect.as_ref().unwrap().more,
        after.connect.as_ref().unwrap().more
    );
    broker::answer_connect_cli(&env, &after.id, ConnectAnswer::Deny).unwrap();
    assert!(broker::connect_card_retired(&env, &before).unwrap());
}

#[test]
fn pause_deny_is_bound_to_selected_presentation() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    let (first, _) = add(&a, "first", 101);
    let (second, _) = add(&a, "second", 102);
    let selected = notice_card(&a, &env);
    broker::answer_connect_cli(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let (late, _) = add(&a, "late", 103);
    assert_eq!(card(&env, &selected.id), selected);
    let polled = a.poll(104);
    assert_eq!(polled.answers.len(), 2);
    assert!(polled.answers.iter().any(|a| a.triple == first));
    assert!(polled.answers.iter().any(|a| a.triple == second));
    assert!(polled.answers.iter().all(|a| a.triple != late));
    assert!(a.open().iter().any(|(t, _)| t == &late));
    let path = broker::approvals_dir(&env).join(format!("{}.done", selected.id));
    let outcome: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(outcome["request"]["connect"]["more"], 2);
    assert_eq!(outcome["affected"].as_array().unwrap().len(), 2);
}

#[test]
fn failed_pause_write_retries_same_frozen_targets() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let selected = notice_card(&a, &env);
    broker::answer_connect_cli(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let path = broker::approvals_dir(&env).join(format!("{}.done", selected.id));
    std::fs::create_dir(&path).unwrap();
    assert!(a.poll(102).answers.is_empty());
    let (late, _) = add(&a, "late", 103);
    std::fs::remove_dir(&path).unwrap();
    let got = a.poll(104);
    assert_eq!(got.answers.len(), 1);
    assert_ne!(got.answers[0].triple, late);
    assert!(a.open().iter().any(|(t, _)| t == &late));
    let outcome: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(outcome["affected"].as_array().unwrap().len(), 1);
}

#[test]
fn published_pause_outcome_can_be_reconciled_only_with_same_targets() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let selected = notice_card(&a, &env);
    broker::answer_connect_cli(&env, &selected.id, ConnectAnswer::AllowOnce).unwrap();
    let frozen = std::fs::read_to_string(
        broker::approvals_dir(&env).join(format!("{}.answer", selected.id)),
    )
    .unwrap();
    broker::record_pause_outcome(
        &env,
        &selected.id,
        ConnectAnswer::AllowOnce,
        "cli",
        vec![],
        &selected,
        &frozen,
    )
    .unwrap();
    broker::record_pause_outcome(
        &env,
        &selected.id,
        ConnectAnswer::AllowOnce,
        "cli",
        vec![],
        &selected,
        &frozen,
    )
    .unwrap();
    let other = broker::AffectedDecision {
        id: "other".into(),
        app: "app".into(),
        parent_app: None,
        team: None,
        host: "other.example".into(),
        port: 443,
        path: None,
        guest_pid: None,
        waiting: 1,
    };
    assert!(broker::record_pause_outcome(
        &env,
        &selected.id,
        ConnectAnswer::AllowOnce,
        "cli",
        vec![other],
        &selected,
        &frozen
    )
    .is_err());
    assert!(a.poll(102).answers.is_empty());
    assert!(!a.paused());
}

#[test]
fn a_replaced_answered_notice_is_retained_without_applying_its_scope() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let selected = notice_card(&a, &env);
    broker::answer_connect_cli(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let path = broker::approvals_dir(&env).join(format!("{}.json", selected.id));
    let mut changed = selected.clone();
    changed.connect.as_mut().unwrap().more += 1;
    std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(a.poll(102).answers.is_empty());
    assert!(a.paused());
    assert!(!broker::approvals_dir(&env)
        .join(format!("{}.done", selected.id))
        .exists());
    assert!(broker::approvals_dir(&env)
        .join(format!("{}.answer", selected.id))
        .is_file());
}

#[test]
fn a_stale_display_id_cannot_select_a_newer_target_set() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let displayed = notice_card(&a, &env);
    add(&a, "second", 102);
    let current = notice_card(&a, &env);
    assert_ne!(displayed.id, current.id);
    assert!(broker::answer_connect_cli(&env, &displayed.id, ConnectAnswer::Deny).is_err());
    assert!(!broker::approvals_dir(&env)
        .join(format!("{}.answer", current.id))
        .exists());
    assert!(a.paused());
}

#[test]
fn changed_pause_answer_between_check_and_record_is_retained() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let selected = notice_card(&a, &env);
    broker::answer_connect_cli(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let answer_path = broker::approvals_dir(&env).join(format!("{}.answer", selected.id));
    let mut changed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&answer_path).unwrap()).unwrap();
    changed["source"] = "panel".into();
    super::notice::BEFORE_OUTCOME_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(&answer_path, changed.to_string()).unwrap();
        }));
    });
    assert!(a.poll(102).answers.is_empty());
    assert!(a.paused());
    assert!(!broker::approvals_dir(&env)
        .join(format!("{}.done", selected.id))
        .exists());
}
