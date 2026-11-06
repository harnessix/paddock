use super::*;
use crate::macvm::proto::{ErrorCode, Outcome, ReplyData};

fn hello<'a>(prepared: bool, warm: bool, password: Option<&'a str>) -> Hello<'a> {
    Hello {
        prepared,
        warm,
        password,
        data_id: "abcd-3",
    }
}

fn ok(verify: &str) -> Outcome {
    Outcome::Ok {
        data: Some(ReplyData::DataPrepare {
            verify: verify.into(),
        }),
    }
}

fn err(code: ErrorCode, t: &str) -> Outcome {
    Outcome::Error {
        code,
        text: t.into(),
    }
}

fn sends(v: &[Action]) -> usize {
    v.iter()
        .filter(|a| matches!(a, Action::Send { .. }))
        .count()
}

fn starts(v: &[Action]) -> bool {
    v.contains(&Action::StartApp)
}

fn stamp(v: &[Action]) -> Option<(bool, Option<String>)> {
    v.iter().find_map(|a| match a {
        Action::Stamp { prepared, error } => Some((*prepared, error.clone())),
        _ => None,
    })
}

fn events(v: &[Action]) -> Vec<(&'static str, String)> {
    v.iter()
        .filter_map(|a| match a {
            Action::Event { kind, text } => Some((*kind, text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_new_profile_sends_prepare_and_the_app_waits() {
    let mut d = DataStart::new();
    let v = d.hello(&hello(false, false, Some("pw")));
    assert_eq!(
        v.last(),
        Some(&Action::Send {
            password: "pw".into(),
            data_id: "abcd-3".into()
        })
    );
    assert!(!starts(&v), "the app waits for the home: {v:?}");
    assert_eq!(d.phase(), Phase::Waiting);
    let v = d.answer(&ok("Checking\nThe volume appears to be OK\n"));
    assert_eq!(stamp(&v), Some((true, None)));
    assert!(starts(&v));
    let ev = events(&v);
    assert_eq!(ev[0].0, "data");
    assert!(ev[0].1.contains("the data disk is prepared"), "{ev:?}");
    assert!(ev[0].1.contains("The volume appears to be OK"), "{ev:?}");
    assert_eq!(d.phase(), Phase::Done);
    // A second answer, or a timeout after the answer, does nothing.
    assert!(d.answer(&ok("x")).is_empty());
    assert!(d.timeout(Duration::from_secs(120)).is_empty());
}

#[test]
fn a_prepared_profile_starts_the_app_at_once() {
    let mut d = DataStart::new();
    let v = d.hello(&hello(true, false, Some("pw")));
    assert_eq!(v, vec![Action::StartApp]);
    // The same on a warm start and without a password.
    let mut d = DataStart::new();
    assert_eq!(d.hello(&hello(true, true, None)), vec![Action::StartApp]);
}

#[test]
fn a_warm_start_never_moves_the_home_under_the_running_app() {
    let mut d = DataStart::new();
    let v = d.hello(&hello(false, true, Some("pw")));
    assert_eq!(sends(&v), 0, "{v:?}");
    assert!(starts(&v));
    assert!(events(&v)[0].1.contains("next start prepares"));
    // No stamp change: the next cold start still prepares.
    assert_eq!(stamp(&v), None);
}

#[test]
fn no_password_is_a_failure_that_names_the_keyring() {
    let mut d = DataStart::new();
    let v = d.hello(&hello(false, false, None));
    assert_eq!(sends(&v), 0);
    assert!(starts(&v));
    let (prepared, error) = stamp(&v).unwrap();
    assert!(!prepared);
    assert!(error.unwrap().contains("keyring"));
    assert_eq!(events(&v)[0].0, "error");
    assert!(v.iter().any(|a| matches!(a, Action::Line(_))));
}

#[test]
fn a_failed_step_names_it_and_the_app_starts_on_the_system_disk() {
    let mut d = DataStart::new();
    d.hello(&hello(false, false, Some("pw")));
    let v = d.answer(&err(ErrorCode::Io, "copy the home: ditto failed: No space"));
    assert!(starts(&v));
    let (prepared, error) = stamp(&v).unwrap();
    assert!(!prepared);
    let error = error.unwrap();
    assert!(error.contains("not prepared"), "{error}");
    assert!(error.contains("copy the home: ditto failed"), "{error}");
    assert_eq!(events(&v)[0], ("error", error));
}

#[test]
fn a_locked_disk_ends_the_start_and_starts_no_app() {
    let mut d = DataStart::new();
    d.hello(&hello(false, false, Some("pw")));
    let v = d.answer(&err(ErrorCode::Locked, "mount the volume: no mount"));
    assert!(!starts(&v), "{v:?}");
    assert!(v.contains(&Action::EndLocked {
        detail: "mount the volume: no mount".into()
    }));
    assert_eq!(
        stamp(&v),
        Some((false, Some("mount the volume: no mount".into())))
    );
}

#[test]
fn the_timeout_starts_the_app_and_a_late_answer_changes_nothing() {
    let mut d = DataStart::new();
    d.hello(&hello(false, false, Some("pw")));
    let v = d.timeout(Duration::from_secs(120));
    assert!(starts(&v));
    let (prepared, error) = stamp(&v).unwrap();
    assert!(!prepared);
    assert!(error.unwrap().contains("120 s"));
    assert!(d.answer(&ok("late")).is_empty());
    // A hello after that (the agent restarted) only starts the app.
    assert_eq!(
        d.hello(&hello(false, false, Some("pw"))),
        vec![Action::StartApp]
    );
}

#[test]
fn an_agent_restart_during_the_prepare_sends_it_again_a_few_times() {
    let mut d = DataStart::new();
    let h = hello(false, false, Some("pw"));
    assert_eq!(sends(&d.hello(&h)), 1);
    // The second send has no second "preparing" event.
    let v = d.hello(&h);
    assert_eq!((sends(&v), events(&v).len()), (1, 0));
    assert_eq!(sends(&d.hello(&h)), 1);
    let v = d.hello(&h);
    assert_eq!(sends(&v), 0);
    assert!(starts(&v));
    assert!(stamp(&v).unwrap().1.unwrap().contains("restarted 3 times"));
}

#[test]
fn the_quit_saves_only_a_prepared_profile_when_a_password_exists() {
    assert!(save_at_quit(true, true, true));
    assert!(!save_at_quit(true, false, true));
    // Nothing can prepare without a password: the save stays.
    assert!(save_at_quit(true, false, false));
    assert!(!save_at_quit(false, true, true));
}

#[test]
fn the_session_password_is_read_never_made() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("guest-password");
    assert_eq!(session_password(&f), Ok(None));
    assert!(!f.exists(), "a session never makes a password");
    std::fs::write(&f, "a pass word\n").unwrap();
    assert_eq!(session_password(&f), Ok(Some("a pass word".into())));
    std::fs::write(&f, "crlf\r\n").unwrap();
    assert_eq!(session_password(&f), Ok(Some("crlf".into())));
    std::fs::write(&f, "").unwrap();
    let e = session_password(&f).unwrap_err();
    assert!(e.contains("damaged"), "{e}");
    // The file stays: only setup replaces a password.
    assert!(f.exists());
}

#[test]
fn the_keyring_file_is_the_one_of_setup() {
    let dir = tempfile::tempdir().unwrap();
    let env =
        crate::env::HostEnv::from_parts(dir.path().into(), "u".into(), 501, 20, Default::default());
    let k = crate::macvm::golden::Keyring::open(&env).unwrap();
    assert_eq!(keyring_file(&env), k.guest_password);
    let pw = k.password().unwrap();
    assert_eq!(session_password(&keyring_file(&env)), Ok(Some(pw)));
}

#[test]
fn a_progress_line_makes_the_host_wait_longer_while_the_prepare_runs() {
    let mut d = DataStart::new();
    assert_eq!(d.progress("x", Duration::from_secs(5)), vec![]);
    d.hello(&hello(false, false, Some("pw")));
    let v = d.progress("copying the home: 3 of 9 parts", Duration::from_secs(160));
    assert_eq!(
        v,
        vec![
            Action::Event {
                kind: "data",
                text: "copying the home: 3 of 9 parts".into()
            },
            Action::Extend(Duration::from_secs(160) + PROGRESS_MARGIN)
        ]
    );
    d.answer(&ok("fine"));
    assert_eq!(d.progress("late", Duration::from_secs(5)), vec![]);
}

fn at(prepared: bool, free: u64) -> AtQuit {
    AtQuit {
        save: true,
        prepared,
        password: true,
        free,
        needs: 5 << 30,
    }
}

#[test]
fn a_quit_while_the_prepare_runs_ends_it_with_a_clear_event_and_no_save() {
    let mut d = DataStart::new();
    d.hello(&hello(false, false, Some("pw")));
    let (save, v) = d.at_quit(&at(false, 100 << 30));
    assert!(!save);
    let (prepared, error) = stamp(&v).unwrap();
    assert!(!prepared);
    assert!(error.unwrap().contains("quit came while"));
    assert!(v.iter().any(
        |a| matches!(a, Action::Event { kind: "data", text } if text.contains("quit came while"))
    ));
    assert!(v.iter().any(|a| matches!(a, Action::Event { kind: "save", text } if *text == text::data_no_save_unprepared())));
    assert_eq!(d.phase(), Phase::Done);
    // The late answer of the guest starts nothing.
    assert_eq!(d.answer(&err(ErrorCode::Io, "the quit came first")), vec![]);
}

#[test]
fn an_unprepared_quit_still_names_too_little_space() {
    let mut d = DataStart::new();
    d.hello(&hello(false, false, None));
    d.answer(&err(ErrorCode::Io, "copy failed"));
    // With a password the save is off for the data disk; the space line
    // comes too when the volume is short.
    let (save, v) = d.at_quit(&at(false, 2 << 30));
    assert!(!save);
    assert!(stamp(&v).is_none(), "the prepare had ended: {v:?}");
    let line = v
        .iter()
        .find_map(|a| match a {
            Action::Line(l) => Some(l.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        line.contains("not enough space for the saved state"),
        "{line}"
    );
    // Enough space: no line. Prepared: the save is on, and the quit
    // machine checks the space itself.
    assert!(!d
        .at_quit(&at(false, 100 << 30))
        .1
        .iter()
        .any(|a| matches!(a, Action::Line(_))));
    let (save, v) = d.at_quit(&at(true, 2 << 30));
    assert!(save && v.is_empty(), "{v:?}");
}
