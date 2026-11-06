use super::*;
use crate::macvm::proto::{ErrorCode, Outcome};

/// A fake agent and VM: what each action gets as its answer.
struct Fake {
    exits_on_quit: bool,
    exits_on_force: bool,
    /// The agent answers `app.quit` with `no_such_app`.
    not_running: bool,
    free: u64,
    pause: Result<(), String>,
    save: Result<u64, String>,
    guest_stops: bool,
    /// The agent answers `shutdown` with this error.
    shutdown_refused: Option<String>,
    /// Feed `Signal` right after this many actions.
    signal_after: Option<usize>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            exits_on_quit: false,
            exits_on_force: false,
            not_running: false,
            free: 0,
            pause: Ok(()),
            save: Ok(0),
            guest_stops: false,
            shutdown_refused: None,
            signal_after: None,
        }
    }
}

/// Drive the machine with the fake until it is done. Returns every action.
fn drive(cfg: Config, fake: &Fake) -> Vec<Action> {
    let mut q = Quit::new(cfg);
    let mut all = Vec::new();
    let mut pending = q.step(Input::Begin { agent: true });
    let mut signalled = false;
    let mut steps = 0;
    while !q.done() {
        steps += 1;
        assert!(steps < 100, "the machine loops: {all:?}");
        let Some(action) = (!pending.is_empty()).then(|| pending.remove(0)) else {
            panic!("no action and not done, phase {:?}: {all:?}", q.phase());
        };
        all.push(action.clone());
        if !signalled && fake.signal_after == Some(all.len()) {
            signalled = true;
            pending = q.step(Input::Signal);
            continue;
        }
        let input = match action {
            Action::SendQuit { .. } if fake.not_running => quit_answer(&Outcome::Error {
                code: ErrorCode::NoSuchApp,
                text: "no such app in the guest: ChatGPT does not run".into(),
            }),
            Action::SendQuit { force: false } => fake.exits_on_quit.then_some(Input::AppExited),
            Action::SendQuit { force: true } => fake.exits_on_force.then_some(Input::AppExited),
            Action::Deadline(_) => Some(Input::Timeout),
            Action::CheckSpace => Some(Input::Space(fake.free)),
            Action::Pause => Some(Input::Paused(fake.pause.clone())),
            Action::Save => Some(Input::Saved(fake.save.clone())),
            Action::Resume => Some(Input::Resumed(Ok(()))),
            Action::SendShutdown if fake.guest_stops => Some(Input::GuestStopped),
            Action::SendShutdown => fake.shutdown_refused.clone().map(Input::ShutdownRefused),
            Action::Stop => Some(Input::Stopped(Ok(()))),
            _ => None,
        };
        if let Some(input) = input {
            // An answer that comes before the deadline makes the deadline
            // moot: drop a pending deadline of the same step.
            pending.retain(|a| !matches!(a, Action::Deadline(_)));
            pending.extend(q.step(input));
        }
    }
    all.extend(pending);
    all
}

fn has(actions: &[Action], a: &Action) -> bool {
    actions.contains(a)
}

fn stamp(actions: &[Action]) -> (bool, Option<u64>) {
    let s: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Stamp { clean, saved } => Some((*clean, *saved)),
            _ => None,
        })
        .collect();
    assert_eq!(s.len(), 1, "one stamp: {actions:?}");
    s[0]
}

fn exit(actions: &[Action]) -> i32 {
    match actions.last() {
        Some(Action::Exit(c)) => *c,
        other => panic!("the last action is {other:?}"),
    }
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
fn quit_state_machine_cases() {
    let cfg = Config::new("ChatGPT", true, 4 << 30);
    let plenty = 100 << 30;

    // 1. The app exits in time, the save succeeds: quit with save.
    let fake = Fake {
        exits_on_quit: true,
        free: plenty,
        save: Ok(4_000_000_000),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    let order: Vec<&Action> = a
        .iter()
        .filter(|a| !matches!(a, Action::Event { .. } | Action::Deadline(_)))
        .collect();
    assert_eq!(
        order,
        vec![
            &Action::SendQuit { force: false },
            &Action::CloseWindows,
            &Action::CheckSpace,
            &Action::Pause,
            &Action::Save,
            &Action::CommitState,
            &Action::Stop,
            &Action::Stamp {
                clean: true,
                saved: Some(4_000_000_000)
            },
            &Action::Exit(0),
        ]
    );
    assert!(!has(&a, &Action::SendQuit { force: true }));
    assert!(lines(&a).is_empty());
    assert!(has(
        &a,
        &Action::Event {
            kind: "stop".into(),
            text: "quit with save".into()
        }
    ));

    // 2. The app ignores the quit: the force path, then the save.
    let fake = Fake {
        exits_on_force: true,
        free: plenty,
        save: Ok(1),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    assert!(has(&a, &Action::SendQuit { force: true }));
    assert!(has(&a, &Action::Deadline(QUIT_WAIT)));
    assert!(has(&a, &Action::Save));
    assert_eq!(stamp(&a), (true, Some(1)));
    assert_eq!(exit(&a), 0);

    // 2b. The force fails too: the VM stops without a save.
    let fake = Fake {
        free: plenty,
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    assert!(has(&a, &Action::Deadline(FORCE_WAIT)));
    assert!(!has(&a, &Action::Save));
    assert!(has(&a, &Action::Stop));
    assert_eq!(stamp(&a), (false, None));
    assert_eq!(
        lines(&a),
        vec!["ChatGPT did not quit in 70 s: the VM stops without a save"]
    );

    // 3. The save is off: no pause, no save, the guest shuts down.
    let off = Config::new("ChatGPT", false, 4 << 30);
    let fake = Fake {
        exits_on_quit: true,
        guest_stops: true,
        free: plenty,
        ..Default::default()
    };
    let a = drive(off, &fake);
    assert!(!has(&a, &Action::Pause) && !has(&a, &Action::Save));
    assert!(!has(&a, &Action::CheckSpace));
    assert!(has(&a, &Action::SendShutdown));
    assert!(!has(&a, &Action::Stop), "{a:?}");
    assert_eq!(stamp(&a), (true, None));
    assert_eq!(exit(&a), 0);
    assert!(has(
        &a,
        &Action::Event {
            kind: "stop".into(),
            text: "quit without save".into()
        }
    ));

    // 4. Too little space: the save is skipped with the line.
    let fake = Fake {
        exits_on_quit: true,
        guest_stops: true,
        free: 1_288_490_188,
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    assert!(has(&a, &Action::CheckSpace));
    assert!(!has(&a, &Action::Pause) && !has(&a, &Action::Save));
    assert_eq!(
        lines(&a),
        vec!["not enough space for the saved state (needs 5 GiB, 1.2 GiB free): the next start is a cold boot. Free space, for example with paddock gc"]
    );
    assert!(has(&a, &Action::SendShutdown));
    assert_eq!(stamp(&a), (true, None));
    assert_eq!(exit(&a), 0);
    // Exactly enough is enough.
    let fake = Fake {
        exits_on_quit: true,
        free: 5 << 30,
        save: Ok(2),
        ..Default::default()
    };
    assert_eq!(stamp(&drive(cfg.clone(), &fake)), (true, Some(2)));

    // 5. The save fails: remove the partial, resume, shutdown, cold next.
    let fake = Fake {
        exits_on_quit: true,
        guest_stops: true,
        free: plenty,
        save: Err("No space left on device".into()),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    let tail: Vec<&Action> = a
        .iter()
        .skip_while(|a| **a != Action::Save)
        .filter(|a| !matches!(a, Action::Event { .. } | Action::Deadline(_)))
        .collect();
    assert_eq!(
        tail,
        vec![
            &Action::Save,
            &Action::RemovePartial,
            &Action::Line(
                "the save failed (No space left on device): the next start is a cold boot".into()
            ),
            &Action::Resume,
            &Action::SendShutdown,
            &Action::Stamp {
                clean: true,
                saved: None
            },
            &Action::Exit(0),
        ]
    );
    assert!(!has(&a, &Action::CommitState));
    // 5b. The guest does not stop after the shutdown: a forced stop,
    // not clean.
    let fake = Fake {
        exits_on_quit: true,
        guest_stops: false,
        free: plenty,
        save: Err("x".into()),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    assert!(has(&a, &Action::Deadline(SHUTDOWN_WAIT)));
    assert!(has(&a, &Action::Stop));
    assert_eq!(stamp(&a), (false, None));

    // 5c. The agent refuses the shutdown (sudo refused the password):
    // the VM stops at once with the reason, no wait for the deadline.
    let fake = Fake {
        exits_on_quit: true,
        free: plenty,
        save: Err("x".into()),
        shutdown_refused: Some("exit 1: Sorry, try again.".into()),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    let at = a.iter().position(|x| *x == Action::SendShutdown).unwrap();
    assert!(
        has(
            &a[at..],
            &Action::Event {
                kind: "error".into(),
                text: "the guest did not shut down (exit 1: Sorry, try again.), stopping the VM"
                    .into(),
            }
        ),
        "{a:?}"
    );
    assert!(has(&a[at..], &Action::Stop));
    assert_eq!(stamp(&a), (false, None));
    // Out of its phase, a refusal does nothing.
    let mut q = Quit::new(cfg.clone());
    assert!(q.step(Input::ShutdownRefused("x".into())).is_empty());

    // 6. A second signal while the app is asked to quit: hard stop, no
    // save, clean = false, exit 3.
    let fake = Fake {
        exits_on_quit: true,
        free: plenty,
        save: Ok(1),
        signal_after: Some(1),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    assert!(!has(&a, &Action::Save) && !has(&a, &Action::Pause));
    assert!(has(&a, &Action::Stop));
    assert!(has(&a, &Action::RemovePartial));
    assert_eq!(stamp(&a), (false, None));
    assert_eq!(exit(&a), EXIT_HARD_STOP);
    assert!(has(
        &a,
        &Action::Event {
            kind: "stop".into(),
            text: "stopped without save".into()
        }
    ));
    // 6b. A second signal during the save: the stop is still hard.
    let fake = Fake {
        exits_on_quit: true,
        free: plenty,
        save: Ok(1),
        signal_after: Some(6),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    assert_eq!(stamp(&a), (false, None));
    assert_eq!(exit(&a), EXIT_HARD_STOP);

    // 7. No agent: the app cannot be asked, the save still runs.
    let mut q = Quit::new(cfg.clone());
    let a = q.step(Input::Begin { agent: false });
    assert!(!a.iter().any(|a| matches!(a, Action::SendQuit { .. })));
    assert!(has(&a, &Action::CloseWindows) && has(&a, &Action::CheckSpace));
    let a = q.step(Input::Space(plenty));
    assert_eq!(a, vec![Action::Pause]);
    let a = q.step(Input::Paused(Err("busy".into())));
    assert_eq!(
        lines(&a),
        vec!["the VM did not pause (busy): the next start is a cold boot"]
    );
    // Without an agent there is no shutdown: the VM stops.
    assert!(has(&a, &Action::Stop));
    let a = q.step(Input::Stopped(Err("late".into())));
    assert_eq!(stamp(&a), (false, None));
    assert!(q.done());
    assert!(q.step(Input::Signal).is_empty());
    // An input out of phase does nothing.
    let mut q = Quit::new(cfg);
    assert!(q.step(Input::AppExited).is_empty());
    assert!(q.step(Input::Saved(Ok(1))).is_empty());
    assert_eq!(q.phase(), Phase::Idle);
}

#[test]
fn quit_of_an_app_that_does_not_run_goes_on_to_the_save() {
    // The scratch profile of a check script, or the user quit the app: the
    // agent answers `no_such_app`, and the save follows at once, with no
    // wait and no force.
    let cfg = Config::new("ChatGPT", true, 4 << 30);
    let fake = Fake {
        not_running: true,
        free: 100 << 30,
        save: Ok(4_000),
        ..Default::default()
    };
    let a = drive(cfg.clone(), &fake);
    let order: Vec<&Action> = a
        .iter()
        .filter(|a| !matches!(a, Action::Event { .. } | Action::Deadline(_)))
        .collect();
    assert_eq!(
        order,
        vec![
            &Action::SendQuit { force: false },
            &Action::CloseWindows,
            &Action::CheckSpace,
            &Action::Pause,
            &Action::Save,
            &Action::CommitState,
            &Action::Stop,
            &Action::Stamp {
                clean: true,
                saved: Some(4_000)
            },
            &Action::Exit(0),
        ]
    );
    assert!(!has(&a, &Action::SendQuit { force: true }));
    assert!(lines(&a).is_empty(), "no line for the user: {a:?}");
    assert!(has(
        &a,
        &Action::Event {
            kind: "quit".into(),
            text: text::quit_app_not_running("ChatGPT"),
        }
    ));

    // The same answer to the forced quit: the app is gone, the save runs.
    let mut q = Quit::new(cfg.clone());
    q.step(Input::Begin { agent: true });
    let forced = q.step(Input::Timeout);
    assert!(forced.contains(&Action::SendQuit { force: true }));
    let after = q.step(Input::AppNotRunning);
    assert!(after.contains(&Action::CheckSpace), "{after:?}");
    assert_eq!(q.phase(), Phase::Pausing);

    // With the save off, the guest shuts down at once.
    let off = Config::new("ChatGPT", false, 4 << 30);
    let fake = Fake {
        not_running: true,
        guest_stops: true,
        ..Default::default()
    };
    let a = drive(off, &fake);
    assert!(has(&a, &Action::SendShutdown) && !has(&a, &Action::Save));
    assert_eq!(stamp(&a), (true, None));

    // A late answer after the quit moved on changes nothing.
    let mut q = Quit::new(cfg);
    q.step(Input::Begin { agent: true });
    q.step(Input::AppExited);
    assert_eq!(q.step(Input::AppNotRunning), vec![]);
    assert_eq!(q.phase(), Phase::Pausing);
}

#[test]
fn quit_answer_maps_only_no_such_app() {
    let err = |code| Outcome::Error {
        code,
        text: "x".into(),
    };
    assert_eq!(
        quit_answer(&err(ErrorCode::NoSuchApp)),
        Some(Input::AppNotRunning)
    );
    // A success waits for `app_exited`; another error waits for the
    // deadline, which forces the quit.
    assert_eq!(quit_answer(&Outcome::Ok { data: None }), None);
    assert_eq!(quit_answer(&err(ErrorCode::Io)), None);
}
