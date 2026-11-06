//! The freeze fold of the shim.

use super::*;

fn t0() -> Instant {
    Instant::now()
}

#[test]
fn pause_then_a_second_pause_is_a_no_op_and_resume_thaws() {
    let now = t0();
    let mut p = Pause::new();
    assert!(!p.frozen());
    assert_eq!(p.ask(Ask::Pause), vec![Do::CallPause, Do::Wait]);
    assert!(p.frozen());
    // The same line while the framework works waits for the same answer.
    assert_eq!(p.ask(Ask::Pause), vec![Do::Wait]);
    assert_eq!(
        p.done(true, Ok(()), now),
        vec![Do::AnswerAll(Ok(())), Do::Frozen]
    );
    // Paused: a second freeze calls nothing.
    assert_eq!(p.ask(Ask::Pause), vec![Do::Answer(Ok(()))]);
    assert_eq!(p.ask(Ask::Resume), vec![Do::CallResume, Do::Wait]);
    let later = now + Duration::from_secs(7);
    assert_eq!(
        p.done(false, Ok(()), later),
        vec![Do::AnswerAll(Ok(())), Do::Thawed(Duration::from_secs(7))]
    );
    assert!(!p.frozen());
    // Running: a thaw calls nothing.
    assert_eq!(p.ask(Ask::Resume), vec![Do::Answer(Ok(()))]);
}

#[test]
fn a_quit_of_a_frozen_session_resumes_first() {
    let now = t0();
    let mut p = Pause::new();
    p.ask(Ask::Pause);
    p.done(true, Ok(()), now);
    assert_eq!(
        p.ask(Ask::Quit("stop request".into())),
        vec![Do::CallResume]
    );
    // A second quit while it resumes adds nothing.
    assert_eq!(p.ask(Ask::Quit("signal".into())), Vec::<Do>::new());
    let v = p.done(false, Ok(()), now + Duration::from_secs(1));
    assert_eq!(v.last(), Some(&Do::BeginQuit("stop request".into())));
    // Quitting: no freeze any more.
    assert!(matches!(p.ask(Ask::Pause)[..], [Do::Answer(Err(_))]));
}

#[test]
fn a_quit_while_pausing_resumes_once_the_pause_is_done() {
    let now = t0();
    let mut p = Pause::new();
    p.ask(Ask::Pause);
    assert!(p.ask(Ask::Quit("signal".into())).is_empty());
    assert_eq!(
        p.done(true, Ok(()), now),
        vec![Do::AnswerAll(Ok(())), Do::Frozen, Do::CallResume]
    );
    let v = p.done(false, Ok(()), now);
    assert_eq!(v.last(), Some(&Do::BeginQuit("signal".into())));
}

#[test]
fn a_quit_of_a_running_session_begins_at_once() {
    let mut p = Pause::new();
    assert_eq!(
        p.ask(Ask::Quit("signal".into())),
        vec![Do::BeginQuit("signal".into())]
    );
}

#[test]
fn failures_answer_with_one_line_and_keep_the_state() {
    let now = t0();
    let mut p = Pause::new();
    p.ask(Ask::Pause);
    let v = p.done(true, Err("E1".into()), now);
    assert!(matches!(&v[..], [Do::AnswerAll(Err(e))] if e.contains("E1")));
    assert!(!p.frozen(), "a failed pause leaves the VM running");
    p.ask(Ask::Pause);
    p.done(true, Ok(()), now);
    p.ask(Ask::Resume);
    let v = p.done(false, Err("E2".into()), now);
    assert!(matches!(&v[..], [Do::AnswerAll(Err(e))] if e.contains("E2")));
    assert!(p.frozen(), "a failed resume leaves the VM paused");
    // Crossed lines are refused, not queued.
    let mut p = Pause::new();
    p.ask(Ask::Pause);
    assert!(matches!(p.ask(Ask::Resume)[..], [Do::Answer(Err(_))]));
    p.done(true, Ok(()), now);
    p.ask(Ask::Resume);
    assert!(matches!(p.ask(Ask::Pause)[..], [Do::Answer(Err(_))]));
}

#[test]
fn a_quit_after_a_failed_resume_still_quits() {
    let now = t0();
    let mut p = Pause::new();
    p.ask(Ask::Pause);
    p.done(true, Ok(()), now);
    p.ask(Ask::Quit("signal".into()));
    let v = p.done(false, Err("E3".into()), now);
    assert_eq!(v.last(), Some(&Do::BeginQuit("signal".into())));
}
