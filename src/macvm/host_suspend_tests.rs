use super::*;
use std::cell::{Cell, RefCell};

fn record(dir: &Path, app: &str, instance: u32, last_key: u64, pid: u32) -> Session {
    let s = Session {
        pid,
        app: app.into(),
        home: crate::macvm::instance::folder(instance),
        profile: dir.join("p").join(app),
        instance,
        started: 1,
        last_key,
        control: dir.join(format!("{app}.sock")),
        ..Default::default()
    };
    let path = dir.join(format!("{}.json", s.key()));
    std::fs::write(path, serde_json::to_vec(&s).unwrap()).unwrap();
    s
}

#[test]
fn auto_suspend_is_on_unless_the_configuration_says_false() {
    assert!(auto_suspend(None));
    let mut cfg = crate::manifest::MacvmCfg::default();
    assert!(auto_suspend(Some(&cfg)));
    cfg.auto_suspend = Some(false);
    assert!(!auto_suspend(Some(&cfg)));
}

#[test]
fn make_room_suspends_the_least_recently_used_live_session() {
    let dir = tempfile::tempdir().unwrap();
    let me = std::process::id();
    record(dir.path(), "a", 1, 900, me);
    record(dir.path(), "b", 2, 100, me);
    // A record of a dead shim never counts.
    record(dir.path(), "c", 1, 1, 0);
    let held = Cell::new(2);
    let asked = RefCell::new(Vec::new());
    let full = || held.get() >= 2;
    let ask = |s: &Session, why: &str| {
        asked.borrow_mut().push((s.key(), why.to_string()));
        held.set(1);
        Ok(())
    };
    let lines = make_room(
        dir.path(),
        "new",
        "new",
        &full,
        &ask,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(asked.borrow()[0].0, "b--2");
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("b 2") && lines[0].contains("new"),
        "{}",
        lines[0]
    );
}

#[test]
fn make_room_without_own_sessions_leaves_the_refusal_to_the_slots() {
    let dir = tempfile::tempdir().unwrap();
    let full = || true;
    let ask = |_: &Session, _: &str| Ok(());
    assert_eq!(
        make_room(dir.path(), "x", "x", &full, &ask, Duration::from_secs(1)),
        Err(None)
    );
}

#[test]
fn make_room_with_a_free_slot_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "a", 1, 1, std::process::id());
    let full = || false;
    let ask = |_: &Session, _: &str| -> Result<(), String> { panic!("no ask") };
    assert_eq!(
        make_room(dir.path(), "x", "x", &full, &ask, Duration::from_secs(1)),
        Ok(vec![])
    );
}

#[test]
fn handover_ends_the_suspended_shim_and_records_the_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = record(dir.path(), "a", 1, 1, 4242);
    std::fs::create_dir_all(&s.profile).unwrap();
    s.suspended = true;
    let signalled = Cell::new(0);
    let alive = |_: u32| signalled.get() == 0;
    let signal = |pid: u32| signalled.set(pid);
    handover(
        &s,
        "a click on its Dock icon",
        &alive,
        &signal,
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(signalled.get(), 4242);
    let log = std::fs::read_to_string(Profile::new(&s.profile).events_log()).unwrap();
    assert!(log.contains("\"resume\""), "{log}");
    assert!(log.contains("Dock icon"), "{log}");
}

#[test]
fn handover_of_a_shim_that_stays_is_a_clear_line() {
    let dir = tempfile::tempdir().unwrap();
    let s = record(dir.path(), "a", 3, 1, 4242);
    let alive = |_: u32| true;
    let signal = |_: u32| {};
    let e = handover(&s, "x", &alive, &signal, Duration::from_millis(60)).unwrap_err();
    assert!(e.contains("pid 4242") && e.contains("a 3"), "{e}");
}

#[test]
fn a_reason_is_one_control_line() {
    assert_eq!(one_line("a\nb\tc"), "a b c");
    assert_eq!(one_line(&"x".repeat(500)).len(), 200);
}

/// A shim's control socket that accepts a suspend once, then refuses.
struct Shim {
    asked: std::sync::Mutex<Vec<String>>,
}

impl crate::vm::bridge::Guest for Shim {
    fn connect(&self, _port: u32) -> anyhow::Result<std::os::fd::OwnedFd> {
        anyhow::bail!("no ports")
    }
    fn stop(&self) {}
    fn suspend(&self, reason: &str) -> anyhow::Result<()> {
        let mut asked = self.asked.lock().unwrap();
        asked.push(reason.to_string());
        if asked.len() > 1 {
            anyhow::bail!(text::suspend_while_quitting());
        }
        Ok(())
    }
}

#[test]
fn ask_speaks_the_suspend_line_of_the_control_socket() {
    use crate::vm::bridge;
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let shim = std::sync::Arc::new(Shim {
        asked: Default::default(),
    });
    bridge::serve_control(bridge::bind_control(&sock).unwrap(), shim.clone());
    let s = Session {
        control: sock,
        ..Default::default()
    };
    assert_eq!(ask(&s, "both slots\nin use"), Ok(()));
    let e = ask(&s, "again").unwrap_err();
    assert_eq!(e, text::suspend_while_quitting());
    assert_eq!(
        *shim.asked.lock().unwrap(),
        vec!["both slots in use".to_string(), "again".to_string()]
    );
    // A shim that is gone is a refusal too, never a hang.
    let gone = Session {
        control: dir.path().join("none.sock"),
        ..Default::default()
    };
    assert!(ask(&gone, "x").is_err());
}
