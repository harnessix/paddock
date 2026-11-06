//! The disk dialog in the main loop: the agent presses Ignore once and the
//! host gets one `dialog` event; a dialog it cannot dismiss is told too,
//! and another alert of the notification center is left to the user.

use super::*;
use crate::guest::dialogs::Seen;

fn dialog_window(id: u32, owner: &str) -> RawWindow {
    RawWindow {
        id,
        pid: 300,
        owner: owner.into(),
        bundle: None,
        layer: 8,
        bounds: Frame::new(500.0, 300.0, 420.0, 180.0),
        alpha: 1.0,
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn disk_dialog_is_dismissed_and_told() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    {
        let mut s = st.lock().unwrap();
        s.windows = vec![dialog_window(40, "DiskArbitrationAgent")];
        s.dialog = Seen {
            texts: strings(&["The disk you attached was not readable by this computer."]),
            buttons: strings(&["Initialize…", "Ignore", "Eject"]),
            pressed: Some("Ignore".into()),
            why: None,
        };
    }
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let Event::Dialog {
        owner,
        text,
        dismissed,
    } = host.until(|e| matches!(e, Event::Dialog { .. }))
    else {
        unreachable!()
    };
    assert_eq!(owner, "DiskArbitrationAgent");
    assert!(dismissed);
    assert!(text.contains("not readable") && text.ends_with("pressed Ignore"));
    // A few polls later: one try, one event.
    std::thread::sleep(Duration::from_millis(400));
    let req = host.send(Request::AgentRestart);
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent.join().unwrap(), 0);
    assert_eq!(st.lock().unwrap().dialog_asks, vec![40]);
    let events = host
        .seen
        .iter()
        .filter(|e| matches!(e, Event::Dialog { .. }))
        .count();
    assert_eq!(events, 1);
    // The dialog window is a `system` window on the host too.
    assert!(host.seen.iter().any(
        |e| matches!(e, Event::Windows(w) if w.changed.iter().any(|x| x.id == 40 && x.app == windows::SYSTEM_APP))
    ));
}

#[test]
fn dialog_without_permission_is_told_as_open() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    {
        let mut s = st.lock().unwrap();
        s.windows = vec![
            dialog_window(41, "DiskArbitrationAgent"),
            dialog_window(42, "UserNotificationCenter"),
        ];
        s.dialog = Seen {
            why: Some("the agent has no Accessibility permission".into()),
            ..Default::default()
        };
    }
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let Event::Dialog {
        owner, dismissed, ..
    } = host.until(|e| matches!(e, Event::Dialog { .. }))
    else {
        unreachable!()
    };
    assert_eq!(owner, "DiskArbitrationAgent");
    assert!(!dismissed);
    let req = host.send(Request::AgentRestart);
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent.join().unwrap(), 0);
    // The alert of the notification center without the disk buttons is
    // read once and left alone: no event for it.
    assert!(!host
        .seen
        .iter()
        .any(|e| matches!(e, Event::Dialog { owner, .. } if owner == "UserNotificationCenter")));
    let asks = st.lock().unwrap().dialog_asks.clone();
    assert_eq!(asks.iter().filter(|&&id| id == 42).count(), 1);
}
