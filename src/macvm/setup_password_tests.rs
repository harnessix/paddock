//! The configure step when the guest refuses the password: the panel
//! asks for the one the person set, the keyring keeps it, and configure
//! runs again. A child of setup_tests.rs, for its fake backend.

use super::*;

/// A world in `Configure`, as after a guided step that ended.
fn at_configure() -> World {
    let w = world();
    installed_at(&w, State::Configure);
    w
}

fn events(w: &World) -> Vec<super::super::setup_events::Line> {
    super::super::setup_events::read(&w.layout.golden.events)
}

#[test]
fn a_refused_password_asks_keeps_the_typed_one_and_runs_configure_again() {
    let w = at_configure();
    let keyring = Keyring::open(&w.env).unwrap();
    let made = keyring.password().unwrap();
    let typed = "my Own pass 7!";
    let mut fake = Fake::new(&w.layout.golden);
    fake.configure = vec![Ok(End::WrongPassword), Ok(End::Done)];
    fake.asked = vec![Ok(Some(typed.into()))];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(state_of(&w), State::Sealed);
    assert_eq!(
        *fake.configured_with.borrow(),
        vec![made.clone(), typed.to_string()]
    );
    assert_eq!(keyring.password().unwrap(), typed, "the keyring keeps it");
    let calls = fake.calls.borrow();
    let at = |c: &str| calls.iter().position(|x| x == c).unwrap();
    assert!(at("configure") < at("ask_password"));
    assert_eq!(calls.iter().filter(|c| *c == "configure").count(), 2);
    assert!(out.contains(&text::configure_wrong_password()), "{out}");
    assert!(out.contains(&text::password_replaced()), "{out}");
    // Neither password reaches the terminal, the manifest or the log.
    let log = std::fs::read_to_string(&w.layout.golden.events).unwrap();
    let manifest = std::fs::read_to_string(&w.layout.golden.manifest).unwrap();
    for secret in [made.as_str(), typed] {
        assert!(!out.contains(secret), "{out}");
        assert!(!log.contains(secret), "{log}");
        assert!(!manifest.contains(secret));
    }
    let kinds: Vec<String> = events(&w).into_iter().map(|l| l.kind).collect();
    assert!(kinds.contains(&"password refused".to_string()), "{kinds:?}");
    assert!(
        kinds.contains(&"password replaced".to_string()),
        "{kinds:?}"
    );
    assert!(events(&w)
        .iter()
        .all(|l| !l.person || l.kind != "password refused"));
}

#[test]
fn a_stop_at_the_question_stays_in_configure_with_the_old_password() {
    let w = at_configure();
    let keyring = Keyring::open(&w.env).unwrap();
    let made = keyring.password().unwrap();
    let mut fake = Fake::new(&w.layout.golden);
    fake.configure = vec![Ok(End::WrongPassword)];
    fake.asked = vec![Ok(None)];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert_eq!(state_of(&w), State::Configure);
    assert!(
        out.contains(&text::setup_stopped(State::Configure)),
        "{out}"
    );
    assert_eq!(keyring.password().unwrap(), made);
    // The next setup continues at configure, and a right password ends it.
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(*fake.configured_with.borrow(), vec![made]);
}

#[test]
fn a_password_refused_every_time_fails_after_the_tries() {
    let w = at_configure();
    let mut fake = Fake::new(&w.layout.golden);
    fake.configure = vec![Ok(End::WrongPassword); PASSWORD_TRIES];
    fake.asked = (1..PASSWORD_TRIES)
        .map(|i| Ok(Some(format!("guess {i}"))))
        .collect();
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert_eq!(state_of(&w), State::Configure);
    let calls = fake.calls.borrow();
    assert_eq!(
        calls.iter().filter(|c| *c == "configure").count(),
        PASSWORD_TRIES
    );
    assert_eq!(
        calls.iter().filter(|c| *c == "ask_password").count(),
        PASSWORD_TRIES - 1
    );
    let reason = text::password_refused(PASSWORD_TRIES);
    assert!(
        out.contains(&text::state_failed(State::Configure, &reason)),
        "{out}"
    );
}

#[test]
fn a_typed_password_the_keyring_refuses_fails_and_keeps_the_old_one() {
    let w = at_configure();
    let keyring = Keyring::open(&w.env).unwrap();
    let made = keyring.password().unwrap();
    let mut fake = Fake::new(&w.layout.golden);
    fake.configure = vec![Ok(End::WrongPassword)];
    fake.asked = vec![Ok(Some("two\nlines".into()))];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert!(
        out.contains(&text::keyring_password_invalid(golden::PASSWORD_MAX)),
        "{out}"
    );
    assert_eq!(keyring.password().unwrap(), made);
    assert_eq!(state_of(&w), State::Configure);
}

#[test]
fn a_window_that_breaks_at_the_question_fails_the_state() {
    let w = at_configure();
    let mut fake = Fake::new(&w.layout.golden);
    fake.configure = vec![Ok(End::WrongPassword)];
    fake.asked = vec![Err("the guided window is gone".into())];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert!(out.contains("the guided window is gone"), "{out}");
    assert_eq!(state_of(&w), State::Configure);
}

#[test]
fn a_wrong_password_from_the_guided_step_fails_it_with_the_reason() {
    let w = world();
    installed_at(&w, State::Guided);
    let mut fake = Fake::new(&w.layout.golden);
    fake.guided = vec![Ok(End::WrongPassword)];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert!(out.contains(&text::password_refused(1)), "{out}");
    assert_eq!(state_of(&w), State::Guided);
}
