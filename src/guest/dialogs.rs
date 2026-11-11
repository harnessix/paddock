//! The disk dialogs of the guest, platform-neutral so the tests run on
//! Linux. A disk that macOS cannot read makes diskarbitrationd show "The
//! disk you attached was not readable by this computer" with the buttons
//! Initialize, Ignore and Eject. paddock's data disk is formatted by the
//! host before the first boot (`macvm::data_disk`), so the dialog is not
//! expected; when one shows anyway (an old profile with a blank disk, any
//! other disk), the agent presses Ignore through Accessibility, never Eject
//! or Initialize, and tells the host, which names it in the session log.
//! A dialog that the agent cannot dismiss also shows on the host as a
//! window of the app `system` (`windows::SYSTEM_PROMPTS`), so the user can
//! answer it.

use super::windows::RawWindow;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The processes that show the disk dialogs.
pub const DISK_DIALOG_OWNERS: &[&str] = &["DiskArbitrationAgent", "diskarbitrationd"];
/// The process that shows the alerts of system daemons. Its window is a
/// disk dialog only when it has the buttons Ignore and Eject.
pub const NOTIFICATION_OWNER: &str = "UserNotificationCenter";

/// The tries per dialog window, and the gap between two tries.
pub const MAX_TRIES: u32 = 3;
pub const RETRY: Duration = Duration::from_secs(2);

/// What the agent saw in a dialog and what it did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// The static texts of the dialog, in order.
    pub texts: Vec<String>,
    /// The titles of its buttons, in order.
    pub buttons: Vec<String>,
    /// The title of the button that the agent pressed.
    pub pressed: Option<String>,
    /// Why nothing was pressed, when nothing was.
    pub why: Option<String>,
}

/// The button to press: Ignore, matched without case and without an
/// ellipsis. Never Eject (the data disk would leave the guest) and never
/// Initialize (it opens Disk Utility).
pub fn ignore_button(buttons: &[String]) -> Option<usize> {
    buttons.iter().position(|b| {
        let t = b.trim().trim_end_matches(['…', '.']).trim();
        t.eq_ignore_ascii_case("ignore")
    })
}

/// True for a window that can be a disk dialog: the agent reads its
/// buttons to know.
pub fn is_disk_dialog(w: &RawWindow) -> bool {
    DISK_DIALOG_OWNERS.contains(&w.owner.as_str()) || w.owner == NOTIFICATION_OWNER
}

/// True when the dialog of `owner` with these buttons is a disk dialog:
/// any window of the disk owners, and an alert with Ignore and Eject.
pub fn relevant(owner: &str, buttons: &[String]) -> bool {
    DISK_DIALOG_OWNERS.contains(&owner)
        || (ignore_button(buttons).is_some()
            && buttons
                .iter()
                .any(|b| b.trim().eq_ignore_ascii_case("eject")))
}

/// One line for the host: the texts and the buttons, at most 300 chars.
pub fn describe(seen: &Seen) -> String {
    let mut s = seen.texts.join(" ");
    if !seen.buttons.is_empty() {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&format!("[{}]", seen.buttons.join(" / "))); // not user text
    }
    match (&seen.pressed, &seen.why) {
        (Some(b), _) => s.push_str(&format!(": pressed {b}")), // not user text
        (None, Some(why)) => s.push_str(&format!(": not dismissed, {why}")), // not user text
        (None, None) => {}
    }
    let s = s.replace(['\n', '\r'], " ");
    if s.chars().count() > 300 {
        s.chars().take(299).collect::<String>() + "…"
    } else {
        s
    }
}

#[derive(Debug, Clone, Copy)]
struct Try {
    tries: u32,
    last: Instant,
    reported: bool,
}

/// The dialog windows that the agent tries to dismiss: each window id at
/// most [`MAX_TRIES`] times, [`RETRY`] apart, and a gone window is
/// forgotten.
#[derive(Debug, Default)]
pub struct Tracker {
    seen: BTreeMap<u32, Try>,
}

impl Tracker {
    /// The dialog windows of `raw` to try now.
    pub fn due(&mut self, raw: &[RawWindow], now: Instant) -> Vec<RawWindow> {
        self.seen
            .retain(|id, _| raw.iter().any(|w| w.id == *id && is_disk_dialog(w)));
        let mut out = Vec::new();
        for w in raw.iter().filter(|w| is_disk_dialog(w)) {
            match self.seen.get(&w.id) {
                None => out.push(w.clone()),
                Some(t) if t.tries < MAX_TRIES && now.duration_since(t.last) >= RETRY => {
                    out.push(w.clone())
                }
                Some(_) => {}
            }
        }
        out
    }

    /// Record one try. True when the host hears of it: the first try, and
    /// a later one that dismissed the dialog or was the last.
    pub fn tried(&mut self, id: u32, dismissed: bool, now: Instant) -> bool {
        let t = self.seen.entry(id).or_insert(Try {
            tries: 0,
            last: now,
            reported: false,
        });
        t.tries += 1;
        t.last = now;
        if dismissed {
            t.tries = MAX_TRIES;
        }
        let tell = !t.reported || dismissed || t.tries == MAX_TRIES;
        t.reported = true;
        tell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macvm::proto::Frame;

    fn win(id: u32, owner: &str) -> RawWindow {
        RawWindow {
            id,
            pid: 77,
            owner: owner.into(),
            bundle: None,
            layer: 0,
            bounds: Frame::new(0.0, 0.0, 400.0, 200.0),
            alpha: 1.0,
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn presses_ignore_never_eject() {
        assert_eq!(
            ignore_button(&v(&["Initialize…", "Ignore", "Eject"])),
            Some(1)
        );
        assert_eq!(ignore_button(&v(&["IGNORE "])), Some(0));
        assert_eq!(ignore_button(&v(&["Initialize…", "Eject"])), None);
        assert_eq!(ignore_button(&[]), None);
    }

    #[test]
    fn which_dialogs_count() {
        assert!(is_disk_dialog(&win(1, "DiskArbitrationAgent")));
        assert!(is_disk_dialog(&win(1, "UserNotificationCenter")));
        assert!(!is_disk_dialog(&win(1, "Finder")));
        assert!(relevant("DiskArbitrationAgent", &[]));
        assert!(relevant(
            "UserNotificationCenter",
            &v(&["Initialize…", "Ignore", "Eject"])
        ));
        // Another alert of the notification center is the user's.
        assert!(!relevant("UserNotificationCenter", &v(&["OK"])));
        assert!(!relevant("UserNotificationCenter", &v(&["Ignore"])));
    }

    #[test]
    fn line_for_the_host() {
        let seen = Seen {
            texts: v(&["The disk you attached was not readable by this computer."]),
            buttons: v(&["Initialize…", "Ignore", "Eject"]),
            pressed: Some("Ignore".into()),
            why: None,
        };
        assert_eq!(
            describe(&seen),
            "The disk you attached was not readable by this computer. [Initialize… / Ignore / Eject]: pressed Ignore"
        );
        let stuck = Seen {
            why: Some("no Accessibility permission".into()),
            ..Default::default()
        };
        assert_eq!(
            describe(&stuck),
            ": not dismissed, no Accessibility permission"
        );
        let long = Seen {
            texts: vec!["x\n".repeat(400)],
            ..Default::default()
        };
        let d = describe(&long);
        assert_eq!(d.chars().count(), 300);
        assert!(!d.contains('\n'));
    }

    #[test]
    fn tries_three_times_and_reports_once_or_at_the_end() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        let raw = vec![win(5, "DiskArbitrationAgent"), win(6, "Finder")];
        let due = tr.due(&raw, t0);
        assert_eq!(due.iter().map(|w| w.id).collect::<Vec<_>>(), [5]);
        assert!(tr.tried(5, false, t0), "the first try is told");
        // Too soon for the second try.
        assert!(tr.due(&raw, t0 + Duration::from_millis(500)).is_empty());
        let t1 = t0 + RETRY;
        assert_eq!(tr.due(&raw, t1).len(), 1);
        assert!(
            !tr.tried(5, false, t1),
            "a middle failure is not told again"
        );
        let t2 = t1 + RETRY;
        assert_eq!(tr.due(&raw, t2).len(), 1);
        assert!(tr.tried(5, false, t2), "the last failure is told");
        assert!(tr.due(&raw, t2 + RETRY * 5).is_empty(), "no fourth try");
        // The window goes and a new dialog comes with a new id.
        assert!(tr.due(&[], t2).is_empty());
        let again = vec![win(9, "DiskArbitrationAgent")];
        assert_eq!(tr.due(&again, t2).len(), 1);
    }

    #[test]
    fn dismissed_on_a_retry_is_told_and_ends() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        let raw = vec![win(5, "DiskArbitrationAgent")];
        tr.due(&raw, t0);
        tr.tried(5, false, t0);
        tr.due(&raw, t0 + RETRY);
        assert!(tr.tried(5, true, t0 + RETRY));
        assert!(tr.due(&raw, t0 + RETRY * 3).is_empty());
    }
}
