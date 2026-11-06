use super::*;

fn facts() -> Facts {
    Facts {
        at: "2026-11-06T10:00:00Z".into(),
        uptime: 3600,
        idle_secs: Some(42),
    }
}

fn types(actions: &[Action]) -> Vec<u32> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Type(n) => Some(*n),
            _ => None,
        })
        .collect()
}

fn lines(actions: &[Action]) -> Vec<String> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Line(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_lock_is_recorded_with_its_facts_and_typed_at_once() {
    let mut u = Unlocker::new(true);
    let t0 = Instant::now();
    let a = u.report(true, &facts(), t0);
    let Action::Event(first) = &a[0] else {
        panic!("{a:?}")
    };
    assert!(first.contains("locked at 2026-11-06T10:00:00Z"), "{first}");
    assert!(
        first.contains("42 s before") && first.contains("up 3600 s"),
        "{first}"
    );
    assert_eq!(types(&a), vec![1]);
    assert!(u.busy());
    // A second report of the same lock types nothing more.
    assert!(u.report(true, &facts(), t0).is_empty());
}

#[test]
fn an_unlock_within_the_wait_ends_the_try() {
    let mut u = Unlocker::new(true);
    let t0 = Instant::now();
    u.report(true, &facts(), t0);
    assert!(u.typed(Played::All, t0).is_empty());
    let a = u.report(false, &facts(), t0 + Duration::from_secs(2));
    assert_eq!(a, vec![Action::Event(text::screen_unlocked(1))]);
    assert!(!u.busy());
    // The wait ends without a second try.
    assert!(u.tick(t0 + CONFIRM * 2).is_empty());
}

#[test]
fn three_tries_then_one_line_and_no_more_keys() {
    let mut u = Unlocker::new(true);
    let mut t = Instant::now();
    let mut typed = types(&u.report(true, &facts(), t));
    let mut all_lines = Vec::new();
    for _ in 0..10 {
        u.typed(Played::All, t);
        // Still locked: the health keeps saying so.
        t += Duration::from_secs(2);
        u.report(true, &facts(), t);
        t += CONFIRM;
        u.report(true, &facts(), t);
        let a = u.tick(t);
        typed.extend(types(&a));
        all_lines.extend(lines(&a));
    }
    assert_eq!(typed, vec![1, 2, 3]);
    assert_eq!(all_lines, vec![text::screen_stays_locked(3)]);
    assert!(!u.busy());
    // The same lock gets no fourth try, however long it lasts.
    t += CONFIRM * 10;
    assert!(u.report(true, &facts(), t).is_empty());
    assert!(u.tick(t).is_empty());
}

#[test]
fn the_next_lock_gets_its_own_three_tries() {
    let mut u = Unlocker::new(true);
    let t = Instant::now();
    u.report(true, &facts(), t);
    u.give_up(text::screen_no_keyboard());
    // The user unlocked it by hand.
    let a = u.report(false, &facts(), t);
    assert_eq!(a, vec![Action::Event(text::screen_unlocked(0))]);
    assert_eq!(types(&u.report(true, &facts(), t)), vec![1]);
}

#[test]
fn nothing_is_typed_when_the_screen_is_not_locked() {
    let mut u = Unlocker::new(true);
    let t = Instant::now();
    assert!(u.report(false, &facts(), t).is_empty());
    assert!(u.tick(t + CONFIRM * 3).is_empty());
    assert!(!u.may_type(t));
    assert!(u.typed(Played::All, t).is_empty());
}

#[test]
fn a_stale_report_starts_no_try() {
    let mut u = Unlocker::new(true);
    let t0 = Instant::now();
    u.report(true, &facts(), t0);
    u.typed(Played::All, t0);
    // No report since the lock: the agent is silent.
    let late = t0 + CONFIRM + FRESH + Duration::from_secs(1);
    assert!(!u.may_type(late));
    assert!(u.tick(late).is_empty());
    // A fresh report of the lock lets the next tick try again.
    u.report(true, &facts(), late);
    assert_eq!(types(&u.tick(late)), vec![2]);
}

#[test]
fn may_type_follows_the_newest_report() {
    let mut u = Unlocker::new(true);
    let t = Instant::now();
    u.report(true, &facts(), t);
    assert!(u.may_type(t));
    u.report(false, &facts(), t);
    assert!(!u.may_type(t));
    assert!(!u.locked());
}

#[test]
fn no_password_gives_one_line_and_no_keys() {
    let mut u = Unlocker::new(false);
    let t = Instant::now();
    let a = u.report(true, &facts(), t);
    assert!(types(&a).is_empty());
    assert_eq!(lines(&a), vec![text::screen_no_password()]);
    assert!(u.tick(t + CONFIRM * 5).is_empty());
}

#[test]
fn a_missing_keyboard_ends_the_tries() {
    let mut u = Unlocker::new(true);
    let t = Instant::now();
    u.report(true, &facts(), t);
    let a = u.typed(Played::Failed, t);
    assert_eq!(lines(&a), vec![text::screen_no_keyboard()]);
    assert!(u.tick(t + CONFIRM * 2).is_empty());
}

#[test]
fn a_stopped_typing_waits_for_the_report() {
    let mut u = Unlocker::new(true);
    let t = Instant::now();
    u.report(true, &facts(), t);
    let a = u.typed(Played::Stopped, t);
    assert_eq!(a, vec![Action::Event(text::screen_typing_stopped())]);
    let a = u.report(false, &facts(), t + Duration::from_secs(1));
    assert_eq!(a, vec![Action::Event(text::screen_unlocked(1))]);
}
