use super::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn a_lone_move_polls_at_once() {
    let t0 = Instant::now();
    let mut p = MovePace::default();
    assert_eq!(p.moved(t0, t0 + ms(100)), t0);
    // A second move long after the first is lone again.
    let t1 = t0 + MOVE_STREAK + ms(1);
    assert_eq!(p.moved(t1, t1 + ms(100)), t1);
}

#[test]
fn moves_in_a_streak_wait_for_one_poll() {
    let t0 = Instant::now();
    let mut p = MovePace::default();
    p.moved(t0, t0 + ms(100));
    let t1 = t0 + ms(15);
    // The poll that the first move made due later: no sooner than
    // needed, no later than the streak gap.
    assert_eq!(p.moved(t1, t1 + ms(100)), t1 + STREAK_POLL);
    // A poll due sooner stays.
    let t2 = t1 + ms(15);
    assert_eq!(p.moved(t2, t2 + ms(10)), t2 + ms(10));
    assert!(p.streak(t2 + ms(100)));
    assert!(!p.streak(t2 + MOVE_STREAK));
}

#[test]
fn a_notification_in_a_streak_waits_and_polls_at_once_after_it() {
    let t0 = Instant::now();
    let mut p = MovePace::default();
    assert_eq!(p.dirty(t0, t0 + ms(100)), t0);
    p.moved(t0, t0 + ms(100));
    let t1 = t0 + ms(5);
    assert_eq!(p.dirty(t1, t1 + ms(100)), t1 + STREAK_POLL);
    let t2 = t0 + MOVE_STREAK;
    assert_eq!(p.dirty(t2, t2 + ms(100)), t2);
}
