//! IME1: the shim holds the host's input source only while it is the
//! active app and a session window is key. A fake Mac runs the actions of
//! the state machine, with the echo of each switch, and every case ends
//! with the user's own source on the host.

use super::*;
use std::time::{Duration, Instant};

const PINYIN: &str = "com.apple.inputmethod.SCIM.ITABC";
const GERMAN: &str = "com.apple.keylayout.German";

const HOLD: Focus = Focus {
    app_active: true,
    key_window: true,
};
/// The shim is active, no session window is key: a close, a hide, 0
/// windows.
const NO_WINDOW: Focus = Focus {
    app_active: true,
    key_window: false,
};
/// Another app is active.
const AWAY: Focus = Focus {
    app_active: false,
    key_window: false,
};

/// What the shell does, with a fake Mac in place of AppKit and TIS.
struct Mac {
    sync: InputSync,
    focus: Focus,
    current: String,
    /// The source that the user chose last: the host must show it
    /// whenever the shim does not hold the focus.
    own: String,
    now: Instant,
    selects: usize,
}

impl Mac {
    fn new(current: &str) -> Mac {
        Mac {
            sync: InputSync::new(),
            focus: AWAY,
            current: current.into(),
            own: current.into(),
            now: Instant::now(),
            selects: 0,
        }
    }

    fn host(&self) -> HostState {
        HostState {
            sources: vec![
                Source::method(PINYIN, "Pinyin - Simplified", "zh-Hans"),
                Source::layout(ABC, "ABC", "en"),
                Source::layout(GERMAN, "German", "de"),
            ],
            current: self.current.clone(),
            reduce_motion: false,
            increase_contrast: false,
        }
    }

    /// The actions on the host, and the echo of each change of the
    /// current source, as `kTISNotifySelectedKeyboardInputSourceChanged`.
    fn run(&mut self, actions: Vec<Action>) {
        for a in actions {
            if let Action::HostSelect(id) | Action::Restore(id) = a {
                self.selects += 1;
                assert!(self.selects < 100, "the shim switches the host in a loop");
                if id != self.current {
                    self.current = id;
                    self.host_changed();
                }
            }
        }
    }

    /// `input_check` of `run_input.rs`.
    fn check(&mut self) {
        let host = self.host();
        let out = self.sync.on_check(self.focus, self.now, || host);
        self.run(out);
    }

    /// `input_host_changed` of `run_input.rs`.
    fn host_changed(&mut self) {
        if !self.sync.is_key() {
            self.check();
            return;
        }
        let host = self.host();
        let out = self.sync.on_host_change_at(self.focus, self.now, &host);
        self.run(out);
    }

    /// The user picks a source in the menu bar.
    fn pick(&mut self, id: &str) {
        self.own = id.into();
        if self.current != id {
            self.current = id.into();
            self.host_changed();
        }
    }

    fn tick(&mut self, ms: u64) {
        self.now += Duration::from_millis(ms);
        self.check();
    }

    /// The end of the shim: `input_exit` of `run_input.rs`. An ending
    /// session has no focus (`focus` of `run_input.rs`).
    fn exit(&mut self) {
        self.focus = Focus::default();
        let host = self.host();
        let out = self.sync.on_leave(&host);
        self.run(out);
    }

    /// The host shows the user's own source.
    fn assert_own(&self, case: &str) {
        assert_eq!(self.current, self.own, "{case}");
    }
}

#[test]
fn key_holds_the_plain_layout_and_resign_gives_it_back() {
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    assert_eq!(m.current, ABC);
    assert!(m.sync.is_key());
    m.focus = AWAY;
    m.check();
    m.assert_own("another app active: the source is back at once");
}

#[test]
fn close_while_key_without_a_resign() {
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    assert_eq!(m.current, ABC);
    // The window closed and AppKit sent no resign: only the tick sees it.
    // The grace runs from the first tick that sees no key window.
    m.focus = NO_WINDOW;
    m.tick(50);
    m.tick(50);
    assert_eq!(m.current, ABC, "the grace waits for another session window");
    m.tick(50);
    m.assert_own("the grace is over");
    assert!(!m.sync.is_key());
    assert_eq!(m.sync.saved(), None);
}

#[test]
fn a_click_between_two_session_windows_switches_nothing() {
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    let selects = m.selects;
    m.focus = NO_WINDOW;
    m.tick(30);
    m.focus = HOLD;
    m.tick(30);
    m.focus = NO_WINDOW;
    m.tick(30);
    m.tick(90);
    assert_eq!(m.selects, selects);
    assert_eq!(m.current, ABC);
    m.tick(20);
    m.assert_own("the last window resigned");
}

#[test]
fn zero_windows_never_takes_the_keyboard() {
    let mut m = Mac::new(PINYIN);
    m.focus = NO_WINDOW;
    for _ in 0..20 {
        m.tick(50);
    }
    m.pick(GERMAN);
    m.pick(PINYIN);
    m.tick(50);
    m.assert_own("running, 0 windows");
    assert_eq!(m.selects, 0);
    // The windows go to 0 while key.
    m.focus = HOLD;
    m.check();
    assert_eq!(m.current, ABC);
    m.focus = NO_WINDOW;
    for _ in 0..20 {
        m.tick(50);
    }
    m.assert_own("0 windows after key");
}

#[test]
fn another_app_active_does_not_wait_for_the_grace() {
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    m.focus = AWAY;
    m.check();
    m.assert_own("no grace for another app");
    // A session window that AppKit still calls key, with another app
    // active, holds nothing.
    m.focus = Focus {
        app_active: false,
        key_window: true,
    };
    m.tick(500);
    m.assert_own("key window of an inactive app");
}

#[test]
fn a_pick_in_another_app_is_never_switched_back() {
    // The lost resign of the report: the shim believes a session window
    // is key, the user is in another app and picks Pinyin again.
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    assert_eq!(m.current, ABC);
    m.focus = AWAY;
    m.pick(PINYIN);
    m.assert_own("the user's Pinyin stays");
    assert_eq!(m.sync.saved(), None, "no restore is owed any more");
    m.tick(1000);
    m.pick(ABC);
    m.pick(PINYIN);
    m.pick(GERMAN);
    m.tick(1000);
    m.assert_own("every pick stays");
    m.exit();
    m.assert_own("the exit restores nothing");

    // The same in the grace, with the shim active and no window key.
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    m.focus = NO_WINDOW;
    m.pick(GERMAN);
    m.tick(500);
    m.assert_own("a pick in the grace stays");
}

#[test]
fn a_pick_in_the_session_window_still_follows() {
    let mut m = Mac::new(GERMAN);
    m.focus = HOLD;
    m.check();
    assert_eq!(m.current, GERMAN);
    m.pick(PINYIN);
    assert_eq!(m.current, ABC, "the guest composes, the host types ABC");
    assert_eq!(m.sync.guest(), Some(PINYIN));
    m.focus = AWAY;
    m.check();
    m.assert_own("the pick of the session window comes back");
}

#[test]
fn the_globe_key_needs_the_focus() {
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    let h = m.host();
    m.focus = AWAY;
    let out = m.sync.on_globe_at(m.focus, m.now, &h);
    assert!(!out.iter().any(|a| matches!(a, Action::GuestSet { .. })));
    m.run(out);
    m.assert_own("a Globe without the focus releases");
}

#[test]
fn key_before_the_app_is_active_waits_for_the_activation() {
    let mut m = Mac::new(PINYIN);
    // windowDidBecomeKey can come before NSApplicationDidBecomeActive.
    m.focus = Focus {
        app_active: false,
        key_window: true,
    };
    m.check();
    assert_eq!(m.current, PINYIN);
    m.focus = HOLD;
    m.check();
    assert_eq!(m.current, ABC);
}

#[test]
fn the_exit_and_the_crash_give_the_source_back() {
    // The exit while key.
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    m.exit();
    m.assert_own("exit while key");

    // A crash while key: the record holds the saved source, and the
    // sweep of `paddock app list` selects it (`host::sweep`).
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    let record = m.sync.saved().map(str::to_string);
    assert_eq!(record.as_deref(), Some(PINYIN));
    m.current = record.unwrap();
    m.assert_own("the sweep after a crash");

    // A crash after a pick in another app: the record is empty, so the
    // sweep does not undo the pick.
    let mut m = Mac::new(PINYIN);
    m.focus = HOLD;
    m.check();
    m.focus = AWAY;
    m.pick(GERMAN);
    assert_eq!(m.sync.saved(), None);
    m.assert_own("nothing for the sweep");
}

#[derive(Debug, Clone, Copy)]
enum Step {
    /// A click on a session window: the shim active, the window key.
    Key,
    /// The key window closes or goes off screen, with no resign.
    Close,
    /// Another app becomes active, with the notification.
    Away,
    /// Another app becomes active, and every notification is lost.
    AwaySilent,
    /// The user picks a source in the menu bar.
    Pick(&'static str),
    /// Time goes by: the tick of the run loop.
    Wait,
}

fn apply(m: &mut Mac, step: Step) {
    match step {
        Step::Key => {
            m.focus = HOLD;
            m.check();
        }
        Step::Close => m.focus = NO_WINDOW,
        Step::Away => {
            m.focus = AWAY;
            m.check();
        }
        Step::AwaySilent => m.focus = AWAY,
        Step::Pick(id) => m.pick(id),
        Step::Wait => {
            for _ in 0..4 {
                m.tick(50);
            }
        }
    }
}

/// Every order of the steps: after each tick without the focus the host
/// shows the user's own source, and so it does at the end.
#[test]
fn every_order_of_events_ends_with_the_users_source() {
    let steps = [
        Step::Key,
        Step::Close,
        Step::Away,
        Step::AwaySilent,
        Step::Pick(GERMAN),
        Step::Pick(PINYIN),
        Step::Wait,
    ];
    let mut order: Vec<usize> = (0..steps.len()).collect();
    let mut runs = 0;
    permute(&mut order, 0, &mut |order| {
        runs += 1;
        for start in [PINYIN, GERMAN] {
            let mut m = Mac::new(start);
            for &i in order {
                apply(&mut m, steps[i]);
                if !m.focus.app_active {
                    m.tick(50);
                    m.assert_own(&format!("{order:?} from {start}: another app"));
                }
            }
            m.focus = NO_WINDOW;
            for _ in 0..4 {
                m.tick(50);
            }
            m.assert_own(&format!("{order:?} from {start}: no window"));
            m.exit();
            m.assert_own(&format!("{order:?} from {start}: exit"));
            assert_eq!(m.sync.saved(), None);
        }
    });
    assert_eq!(runs, 5040);
}

fn permute(v: &mut Vec<usize>, k: usize, f: &mut dyn FnMut(&[usize])) {
    if k == v.len() {
        f(v);
        return;
    }
    for i in k..v.len() {
        v.swap(k, i);
        permute(v, k + 1, f);
        v.swap(k, i);
    }
}
