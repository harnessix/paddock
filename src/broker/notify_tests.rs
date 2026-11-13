//! Notices on the event stream, and who posts them.

use super::*;
use crate::eventstream::bus::{queue, Follower};
use crate::eventstream::{Bus, Line};
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// An env in a temporary folder: no session bus and not the real home, so
/// paddock itself can never post a notification from these tests.
fn env_in(t: &std::path::Path) -> HostEnv {
    let run = t.join("run");
    std::fs::create_dir_all(&run).unwrap();
    HostEnv::from_parts(
        t.join("home"),
        "demo".into(),
        unsafe { libc::geteuid() },
        20,
        [("XDG_RUNTIME_DIR".to_string(), run.display().to_string())]
            .into_iter()
            .collect(),
    )
}

fn note() -> Note {
    Note {
        summary: "paddock: claude asks for clipboard-read".into(),
        body: "Read the clipboard of the host".into(),
        actions: vec![("once".into(), "Allow once".into())],
        critical: true,
        what: "approval".into(),
        action: Some("r12".into()),
    }
}

fn next(rx: &Receiver<String>) -> Line {
    let raw = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a notice line");
    serde_json::from_str(&raw).unwrap()
}

/// A plain follower, as `paddock events --follow --json` without the
/// launcher: it gets every notice, and paddock still posts.
fn plain(env: &HostEnv) -> (Follower, Receiver<String>) {
    let f = Follower::bind(&Bus::of(env), std::process::id()).unwrap();
    let (tx, rx) = queue();
    f.serve(tx).unwrap();
    (f, rx)
}

/// Every notice goes into the stream with its kind, title, text and
/// action, also when no launcher shows it.
#[test]
fn every_notice_goes_into_the_stream() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path());
    let (_f, rx) = plain(&env);
    // No launcher: paddock tries to post, and the fake env refuses.
    assert!(show(&env, &note()).is_err());
    let line = next(&rx);
    let crate::eventstream::schema::Event::Notice(n) = line.event else {
        panic!("{line:?}");
    };
    assert_eq!(n.what, "approval");
    assert_eq!(n.title, "paddock: claude asks for clipboard-read");
    assert_eq!(n.text, "Read the clipboard of the host");
    assert!(n.critical);
    assert_eq!(n.action.as_deref(), Some("r12"));
}

/// `ask_or_show` publishes once, also when the question falls back to a
/// plain notification.
#[test]
fn a_question_is_published_once() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path());
    let (_f, rx) = plain(&env);
    let shown = ask_or_show(&env, &note(), |_| {});
    let _ = next(&rx);
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a second notice line"
    );
    if cfg!(target_os = "macos") {
        assert!(shown.is_none(), "no launcher, no buttons");
    }
    let _ = shown;
}

/// With a follower that registered with `--notify`: on macOS paddock
/// leaves the notice to it and posts nothing. On Linux paddock still
/// posts as before.
#[test]
fn the_launcher_shows_the_notice_on_macos_only() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path());
    let mut launcher_pid = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let launcher = Follower::bind_with(&Bus::of(&env), launcher_pid.id(), true).unwrap();
    let (tx, rx) = queue();
    launcher.serve(tx).unwrap();

    let shown = show(&env, &note());
    let asked = ask(&env, &note(), |_| {
        panic!("the launcher answers through the CLI")
    });
    if cfg!(target_os = "macos") {
        // A post would fail here: the home of the env is not real.
        assert!(shown.is_ok(), "{shown:?}");
        assert!(asked.is_ok());
        assert!(announce(&env, &note()));
    } else {
        // Linux: the D-Bus path runs as before and finds no session bus.
        assert!(shown.is_err());
        assert!(asked.is_err());
        assert!(!announce(&env, &note()));
    }
    for _ in 0..3 {
        assert!(matches!(
            next(&rx).event,
            crate::eventstream::schema::Event::Notice(_)
        ));
    }

    // The launcher quits: paddock posts again.
    drop(launcher);
    assert!(!announce(&env, &note()));
    assert!(show(&env, &note()).is_err());
    let _ = launcher_pid.kill();
    let _ = launcher_pid.wait();
}

/// A note from an older executor job has no `what` and no `action`.
#[test]
fn an_old_note_reads_as_info() {
    let n: Note =
        serde_json::from_str(r#"{"summary":"s","body":"b","actions":[],"critical":false}"#)
            .unwrap();
    assert_eq!(notice(&n).what, "info");
    assert_eq!(notice(&n).action, None);
}
