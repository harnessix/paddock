//! The suspend path of the quit machine (`Input::Suspend`).

use super::*;

const GIB: u64 = 1 << 30;

fn cfg(save: bool) -> Config {
    Config::new("sample-notes", save, 4 * GIB)
}

fn kinds(v: &[Action]) -> Vec<String> {
    v.iter()
        .map(|a| match a {
            Action::Event { kind, .. } => format!("event:{kind}"), // not user text
            Action::SuspendRefused(_) => "refused".into(),
            other => format!("{other:?}"), // not user text
        })
        .collect()
}

#[test]
fn a_suspend_saves_without_app_quit_and_ends_suspended() {
    let mut q = Quit::new(cfg(true));
    let mut all = q.step(Input::Suspend { agent: true });
    assert_eq!(all, vec![Action::CheckSpace]);
    all.extend(q.step(Input::Space(10 * GIB)));
    assert_eq!(all.last(), Some(&Action::Pause));
    all.extend(q.step(Input::Paused(Ok(()))));
    assert_eq!(all.last(), Some(&Action::Save));
    let saved = q.step(Input::Saved(Ok(123)));
    // The windows close only after the save.
    let close = saved
        .iter()
        .position(|a| *a == Action::CloseWindows)
        .unwrap();
    let commit = saved
        .iter()
        .position(|a| *a == Action::CommitState)
        .unwrap();
    assert!(commit < close);
    assert_eq!(saved.last(), Some(&Action::Stop));
    all.extend(saved);
    let end = q.step(Input::Stopped(Ok(())));
    assert!(end.contains(&Action::Stamp {
        clean: true,
        saved: Some(123)
    }));
    assert_eq!(end.last(), Some(&Action::Suspended));
    assert!(!end.iter().any(|a| matches!(a, Action::Exit(_))));
    all.extend(end);
    assert!(!all.iter().any(|a| matches!(a, Action::SendQuit { .. })));
    assert!(!all.contains(&Action::SendShutdown));
    assert!(q.done());
}

#[test]
fn a_suspend_with_the_save_off_is_refused_at_once() {
    let mut q = Quit::new(cfg(false));
    let v = q.step(Input::Suspend { agent: true });
    assert_eq!(kinds(&v), vec!["refused"]);
    assert_eq!(q.phase(), Phase::Idle);
    // The session can still quit afterwards.
    assert!(q
        .step(Input::Begin { agent: true })
        .contains(&Action::SendQuit { force: false }));
}

#[test]
fn a_suspend_without_space_is_refused_and_the_vm_keeps_running() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Suspend { agent: true });
    let v = q.step(Input::Space(GIB));
    assert_eq!(kinds(&v), vec!["event:suspend", "refused"]);
    assert!(!v
        .iter()
        .any(|a| matches!(a, Action::Pause | Action::CloseWindows)));
    assert_eq!(q.phase(), Phase::Idle);
}

#[test]
fn a_failed_pause_refuses_the_suspend() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Suspend { agent: true });
    q.step(Input::Space(10 * GIB));
    let v = q.step(Input::Paused(Err("busy".into())));
    assert!(matches!(v.last(), Some(Action::SuspendRefused(l)) if l.contains("busy")));
    assert_eq!(q.phase(), Phase::Idle);
}

#[test]
fn a_failed_save_resumes_the_vm_and_refuses_the_suspend() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Suspend { agent: true });
    q.step(Input::Space(10 * GIB));
    q.step(Input::Paused(Ok(())));
    let v = q.step(Input::Saved(Err("disk".into())));
    assert!(v.contains(&Action::RemovePartial));
    assert!(v.contains(&Action::Resume));
    assert!(!v.contains(&Action::CloseWindows));
    let v = q.step(Input::Resumed(Ok(())));
    assert_eq!(kinds(&v), vec!["event:suspend", "refused"]);
    assert!(!v.contains(&Action::SendShutdown));
    assert_eq!(q.phase(), Phase::Idle);
}

#[test]
fn a_quit_during_the_suspend_ends_as_a_quit_with_save() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Suspend { agent: true });
    q.step(Input::Space(10 * GIB));
    assert!(q.step(Input::Begin { agent: true }).is_empty());
    q.step(Input::Paused(Ok(())));
    q.step(Input::Saved(Ok(9)));
    let end = q.step(Input::Stopped(Ok(())));
    assert_eq!(end.last(), Some(&Action::Exit(0)));
    assert!(!end.contains(&Action::Suspended));
}

#[test]
fn a_quit_during_a_refused_suspend_goes_on_as_a_quit() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Suspend { agent: true });
    q.step(Input::Space(10 * GIB));
    q.step(Input::Begin { agent: true });
    let v = q.step(Input::Paused(Err("no".into())));
    assert!(v.contains(&Action::SendQuit { force: false }), "{v:?}");
}

#[test]
fn a_second_signal_during_a_suspend_is_a_hard_stop() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Suspend { agent: true });
    q.step(Input::Space(10 * GIB));
    q.step(Input::Paused(Ok(())));
    let v = q.step(Input::Signal);
    assert_eq!(v.last(), Some(&Action::Stop));
    let end = q.step(Input::Stopped(Ok(())));
    assert_eq!(end.last(), Some(&Action::Exit(EXIT_HARD_STOP)));
}

#[test]
fn a_suspend_while_quitting_does_nothing() {
    let mut q = Quit::new(cfg(true));
    q.step(Input::Begin { agent: true });
    assert!(q.step(Input::Suspend { agent: true }).is_empty());
    assert_eq!(q.phase(), Phase::Quitting);
}
