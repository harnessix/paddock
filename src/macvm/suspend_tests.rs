use super::*;
use std::cell::{Cell, RefCell};

fn s(app: &str, instance: u32, pid: u32, started: u64, last_key: u64) -> Session {
    Session {
        pid,
        app: app.into(),
        instance,
        started,
        last_key,
        slot: Some(0),
        ..Default::default()
    }
}

fn busy_as(mut s: Session, b: Busy) -> Session {
    s.busy = Some(b.word().into());
    s
}

#[test]
fn the_least_recently_key_session_is_chosen() {
    let live = vec![s("a", 1, 10, 100, 500), s("b", 1, 11, 200, 300)];
    assert_eq!(choose(&live, "c").unwrap().app, "b");
    // A session that was never key counts from its start.
    let live = vec![s("a", 1, 10, 100, 500), s("b", 1, 11, 600, 0)];
    assert_eq!(choose(&live, "c").unwrap().app, "a");
}

#[test]
fn busy_sessions_are_passed_over_for_the_next_one() {
    let live = vec![
        busy_as(s("a", 1, 10, 100, 100), Busy::Dialog),
        s("b", 1, 11, 100, 900),
    ];
    assert_eq!(choose(&live, "c").unwrap().app, "b");
    for b in [Busy::Dialog, Busy::Copy, Busy::DataPrepare] {
        let live = vec![
            busy_as(s("a", 1, 10, 100, 100), b),
            busy_as(s("b", 2, 11, 1, 1), b),
        ];
        let refused = choose(&live, "c").unwrap_err();
        assert_eq!(refused.len(), 2);
        assert_eq!(refused[0], ("b 2".to_string(), b));
    }
}

#[test]
fn the_starting_session_and_suspended_ones_are_never_chosen() {
    let mut gone = s("a", 1, 10, 1, 1);
    gone.suspended = true;
    let live = vec![gone, s("b", 1, 11, 2, 2), s("b", 2, 12, 3, 3)];
    assert_eq!(choose(&live, "b").unwrap().key(), "b--2");
    assert!(choose(&live[..2], "b").unwrap_err().is_empty());
}

#[test]
fn busy_words_round_trip() {
    for b in [Busy::Dialog, Busy::Copy, Busy::DataPrepare] {
        assert_eq!(Busy::parse(b.word()), Some(b));
        assert!(!b.text().is_empty());
    }
    assert_eq!(Busy::parse("other"), None);
    assert_eq!(busy(true, true, true), Some(Busy::Dialog));
    assert_eq!(busy(false, true, true), Some(Busy::Copy));
    assert_eq!(busy(false, false, true), Some(Busy::DataPrepare));
    assert_eq!(busy(false, false, false), None);
}

/// A fake host: two slots, the records, and the shims' answers.
struct Fake {
    held: Cell<usize>,
    live: Vec<Session>,
    asked: RefCell<Vec<(String, String)>>,
    refuse: Vec<String>,
    /// The slot frees after this many polls of an accepted ask.
    frees_after: Cell<Option<u32>>,
    clock: Cell<Instant>,
}

impl Fake {
    fn new(live: Vec<Session>) -> Self {
        Self {
            held: Cell::new(2),
            live,
            asked: RefCell::new(vec![]),
            refuse: vec![],
            frees_after: Cell::new(None),
            clock: Cell::new(Instant::now()),
        }
    }

    fn run(&self, frees_after: Option<u32>) -> Result<Freed, NoSlot> {
        let full = || {
            if let Some(n) = self.frees_after.get() {
                if n == 0 {
                    self.held.set(1);
                } else {
                    self.frees_after.set(Some(n - 1));
                }
            }
            self.held.get() >= 2
        };
        let live = || self.live.clone();
        let ask = |s: &Session, why: &str| {
            self.asked.borrow_mut().push((s.key(), why.to_string()));
            if self.refuse.contains(&s.key()) {
                return Err(format!("{} said no", s.key())); // not user text
            }
            self.frees_after.set(frees_after);
            Ok(())
        };
        let sleep = |d: Duration| self.clock.set(self.clock.get() + d);
        let now = || self.clock.get();
        let p = Ports {
            full: &full,
            live: &live,
            ask: &ask,
            sleep: &sleep,
            now: &now,
        };
        free_slot(&p, "new", "new", WAIT)
    }
}

#[test]
fn a_free_slot_suspends_nothing() {
    let f = Fake::new(vec![s("a", 1, 10, 1, 1)]);
    f.held.set(1);
    assert_eq!(f.run(Some(0)), Ok(Freed::Free));
    assert!(f.asked.borrow().is_empty());
}

#[test]
fn full_slots_suspend_the_least_recently_used_and_wait_for_its_slot() {
    let f = Fake::new(vec![s("a", 1, 10, 1, 50), s("b", 1, 11, 1, 20)]);
    match f.run(Some(3)) {
        Ok(Freed::Suspended(s)) => assert_eq!(s.app, "b"),
        other => panic!("{other:?}"),
    }
    let asked = f.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, "b");
    assert!(asked[0].1.contains("new"), "{}", asked[0].1);
}

#[test]
fn a_shim_that_refuses_passes_the_turn_to_the_next() {
    let mut f = Fake::new(vec![s("a", 1, 10, 1, 50), s("b", 1, 11, 1, 20)]);
    f.refuse = vec!["b".into()];
    match f.run(Some(0)) {
        Ok(Freed::Suspended(s)) => assert_eq!(s.app, "a"),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.asked.borrow().len(), 2);
}

#[test]
fn every_session_busy_or_refusing_is_a_clear_refusal() {
    let mut f = Fake::new(vec![
        busy_as(s("a", 1, 10, 1, 50), Busy::Copy),
        s("b", 1, 11, 1, 20),
    ]);
    f.refuse = vec!["b".into()];
    match f.run(Some(0)) {
        Err(NoSlot::Refused(line)) => {
            assert!(line.contains("new"), "{line}");
            assert!(line.contains("a"), "{line}");
            assert!(line.contains("b said no"), "{line}");
        }
        other => panic!("{other:?}"),
    }
    // The busy session was never asked.
    assert_eq!(f.asked.borrow().len(), 1);
}

#[test]
fn slots_of_other_users_only_are_not_this_users_to_free() {
    let f = Fake::new(vec![]);
    assert_eq!(f.run(Some(0)), Err(NoSlot::NoneOfYours));
}

#[test]
fn a_slot_that_never_frees_ends_in_the_timeout_line() {
    let f = Fake::new(vec![s("a", 2, 10, 1, 1)]);
    match f.run(None) {
        Err(NoSlot::Refused(line)) => assert!(line.contains("a 2"), "{line}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_old_record_without_the_new_fields_reads_as_instance_one() {
    let json = r#"{"pid":1,"app":"x","home":"default","profile":"/p","started":5,"control":"/c"}"#;
    let s: Session = serde_json::from_str(json).unwrap();
    assert_eq!(s.instance(), 1);
    assert_eq!(s.key(), "x");
    assert!(!s.suspended);
    assert_eq!(last_used(&s), 5);
}
