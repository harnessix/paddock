//! The driver over a scripted harness. The tests that drive the real
//! custodians through the same seam are in seam_tests.rs.

use super::*;
use crate::horse::Binding;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const TICK: Duration = Duration::from_millis(5);
const NATIVE: &str = "script-1";

/// What the scripted harness does, and what it saw.
#[derive(Default)]
struct State {
    /// A turn runs.
    busy: bool,
    /// The main session has a store session.
    known: bool,
    typed: Option<Typed>,
    /// The harness closes its output when its input closes.
    exits: bool,
    /// `close_input` only asks the harness to end (dsh `shutdown`): it
    /// still takes inputs until it closes its output.
    takes: bool,
    /// The harness closed its output by itself.
    gone: bool,
    /// The next read fails with this text.
    fail: Option<String>,
    delivered: Vec<Input>,
    interrupts: usize,
    input_closed: bool,
    finished: Option<End>,
}

struct Script {
    st: Store,
    sid: String,
    s: Arc<Mutex<State>>,
}

impl Live for Script {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        std::thread::sleep(timeout.min(Duration::from_millis(1)));
        let s = self.s.lock().unwrap();
        if let Some(f) = &s.fail {
            anyhow::bail!("{f}");
        }
        Ok(if s.gone || (s.input_closed && s.exits) {
            Pump::Closed
        } else {
            Pump::Open
        })
    }

    fn main_session(&self) -> Option<String> {
        self.s.lock().unwrap().known.then(|| self.sid.clone())
    }

    fn sessions(&self) -> Vec<LiveSession> {
        let s = self.s.lock().unwrap();
        if !s.known {
            return Vec::new();
        }
        vec![LiveSession::turns(&self.sid, NATIVE, !s.busy, None)]
    }

    fn safe(&self, session: Option<&str>) -> bool {
        let s = self.s.lock().unwrap();
        let ours = match session {
            None => true,
            Some(x) => s.known && x == self.sid,
        };
        ours && !s.busy && (!s.input_closed || s.takes)
    }

    fn deliver(&mut self, input: Input) -> Result<()> {
        if let Input::Entry(e) = &input {
            self.st.record_delivery(e, None, &e.id)?;
        }
        let mut s = self.s.lock().unwrap();
        assert!(!s.busy, "a delivery during a turn");
        s.busy = true;
        s.delivered.push(input);
        Ok(())
    }

    fn interrupt(&mut self) -> Result<bool> {
        let mut s = self.s.lock().unwrap();
        s.interrupts += 1;
        Ok(s.busy)
    }

    fn close_input(&mut self) -> Result<()> {
        self.s.lock().unwrap().input_closed = true;
        Ok(())
    }

    fn takes_input(&self) -> bool {
        let s = self.s.lock().unwrap();
        s.takes && s.input_closed && !s.gone
    }

    fn finish(&mut self, end: End) -> Result<()> {
        self.s.lock().unwrap().finished = Some(end);
        Ok(())
    }

    fn typed(&self) -> Typed {
        self.s.lock().unwrap().typed.unwrap_or(Typed::Direct)
    }
}

struct Rig {
    _t: tempfile::TempDir,
    st: Store,
    sid: String,
    s: Arc<Mutex<State>>,
    ctl: Sender<Control>,
    join: Option<JoinHandle<Result<End>>>,
}

fn rig(init: State, close_wait: Option<Duration>) -> Rig {
    rig_ends(init, close_wait, Ends::ByRun, &[])
}

/// A rig whose driver ends by `ends`, with `queued` in the inbox of the
/// main session before the driver starts.
fn rig_ends(init: State, close_wait: Option<Duration>, ends: Ends, queued: &[&str]) -> Rig {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap().id;
    let b = Binding {
        harness: "dsh".into(),
        native_id: Some(NATIVE.into()),
        version: crate::horse::dsh::VERSION.into(),
        container: crate::horse::dsh::CONTAINER.into(),
    };
    let sid = st.new_session(&horse, b).unwrap().id;
    for q in queued {
        st.queue_append(&sid, "user", q).unwrap();
    }
    let s = Arc::new(Mutex::new(init));
    let (ctl, control) = channel();
    let mut live = Script {
        st: st.clone(),
        sid: sid.clone(),
        s: s.clone(),
    };
    let mut d = Driver::new(st.clone(), &horse, "sbx-script", TICK).ends(ends);
    if let Some(w) = close_wait {
        d = d.close_wait(w);
    }
    let join = std::thread::spawn(move || d.drive(&mut live, &control));
    Rig {
        _t: t,
        st,
        sid,
        s,
        ctl,
        join: Some(join),
    }
}

impl Rig {
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.s.lock().unwrap())
    }
    fn delivered(&self) -> Vec<Input> {
        self.with(|s| s.delivered.clone())
    }
    fn end_turn(&self) {
        self.with(|s| s.busy = false);
    }
    fn end(&mut self) -> Result<End> {
        self.join.take().unwrap().join().unwrap()
    }
    fn record(&self) -> Option<Vec<u8>> {
        self.st.read_ref("live/sbx-script.json").unwrap()
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Some passes of the driver, to show that something does not happen.
fn settle() {
    std::thread::sleep(TICK * 10);
}

fn known_busy() -> State {
    State {
        known: true,
        busy: true,
        exits: true,
        ..State::default()
    }
}

#[test]
fn typed_lines_go_first_and_entries_wait_for_the_safe_point() {
    let mut r = rig(known_busy(), None);
    let e = r.st.queue_append(&r.sid, "note", "from the host").unwrap();
    r.ctl.send(Control::User("typed".into())).unwrap();
    r.ctl.send(Control::User("   ".into())).unwrap();
    settle();
    assert!(r.delivered().is_empty(), "nothing goes out during a turn");
    r.end_turn();
    wait_for("the typed line", || r.delivered().len() == 1);
    assert_eq!(r.delivered()[0], Input::Typed("typed".into()));
    settle();
    assert_eq!(r.delivered().len(), 1, "the entry waits for the next turn");
    r.end_turn();
    wait_for("the entry", || r.delivered().len() == 2);
    assert_eq!(r.delivered()[1], Input::Entry(e));
    assert!(r.st.inbox(&r.sid).unwrap().is_empty());
    r.end_turn();
    settle();
    assert_eq!(r.delivered().len(), 2, "the blank line is not a prompt");
    r.ctl.send(Control::Close).unwrap();
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn inbox_typed_lines_wait_for_the_main_session_then_queue() {
    let mut r = rig(
        State {
            typed: Some(Typed::Inbox),
            exits: true,
            ..State::default()
        },
        None,
    );
    r.ctl.send(Control::User("typed".into())).unwrap();
    settle();
    assert!(r.st.inbox(&r.sid).unwrap().is_empty());
    assert!(
        r.delivered().is_empty(),
        "an inbox harness gets no Typed input"
    );
    r.with(|s| s.known = true);
    wait_for("the entry", || r.delivered().len() == 1);
    let Input::Entry(e) = &r.delivered()[0] else {
        panic!("{:?}", r.delivered());
    };
    assert_eq!((e.role.as_str(), e.text.as_str()), ("user", "typed"));
    assert_eq!(r.st.deliveries(&r.sid).unwrap().len(), 1);
    r.ctl.send(Control::Close).unwrap();
    r.end_turn();
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn close_waits_for_the_safe_point_after_the_held_lines() {
    let mut r = rig(known_busy(), None);
    r.st.queue_append(&r.sid, "user", "stays").unwrap();
    r.ctl.send(Control::User("last".into())).unwrap();
    r.ctl.send(Control::Close).unwrap();
    settle();
    assert!(!r.with(|s| s.input_closed), "the turn runs");
    r.end_turn();
    wait_for("the held line", || r.delivered().len() == 1);
    assert_eq!(r.delivered()[0], Input::Typed("last".into()));
    assert!(!r.with(|s| s.input_closed));
    r.end_turn();
    assert_eq!(r.end().unwrap(), End::Closed);
    assert_eq!(r.delivered().len(), 1, "no entry goes out while closing");
    assert_eq!(r.st.inbox(&r.sid).unwrap().len(), 1);
    assert_eq!(r.with(|s| s.finished), Some(End::Closed));
}

#[test]
fn a_second_close_closes_during_a_turn() {
    let mut r = rig(known_busy(), None);
    r.ctl.send(Control::Close).unwrap();
    settle();
    assert!(!r.with(|s| s.input_closed));
    r.ctl.send(Control::Close).unwrap();
    assert_eq!(r.end().unwrap(), End::Closed);
    assert!(r.with(|s| s.busy && s.input_closed));
}

#[test]
fn a_harness_that_does_not_exit_ends_after_the_close_wait() {
    let mut init = known_busy();
    init.busy = false;
    init.exits = false;
    let mut r = rig(init, Some(Duration::from_millis(40)));
    let at = Instant::now();
    r.ctl.send(Control::Close).unwrap();
    assert_eq!(r.end().unwrap(), End::TimedOut);
    assert!(at.elapsed() >= Duration::from_millis(40));
    assert_eq!(r.with(|s| s.finished), Some(End::TimedOut));
}

/// An idle harness that was asked to end but did not end yet.
fn idle_asked_to_end() -> State {
    State {
        known: true,
        takes: true,
        ..State::default()
    }
}

#[test]
fn an_idle_session_closes_at_once_and_still_takes_a_late_entry() {
    let mut r = rig(idle_asked_to_end(), None);
    // The end of the input comes first, as with stdin from /dev/null.
    r.ctl.send(Control::Close).unwrap();
    wait_for("the close", || r.with(|s| s.input_closed));
    assert!(r.delivered().is_empty());
    // A DNA hot-load or a graft, queued while the session is idle.
    let e = r.st.queue_append(&r.sid, "system", "hot-load").unwrap();
    wait_for("the late entry", || r.delivered().len() == 1);
    assert_eq!(r.delivered()[0], Input::Entry(e));
    assert!(r.st.inbox(&r.sid).unwrap().is_empty());
    r.end_turn();
    settle();
    assert_eq!(r.delivered().len(), 1);
    r.with(|s| s.gone = true);
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn an_entry_waits_for_the_safe_point_also_after_the_close() {
    let mut r = rig(idle_asked_to_end(), None);
    r.ctl.send(Control::Close).unwrap();
    wait_for("the close", || r.with(|s| s.input_closed));
    r.with(|s| s.busy = true);
    let e = r.st.queue_append(&r.sid, "system", "graft").unwrap();
    settle();
    assert!(r.delivered().is_empty(), "nothing goes out during a turn");
    r.end_turn();
    wait_for("the entry", || r.delivered().len() == 1);
    assert_eq!(r.delivered()[0], Input::Entry(e));
    r.with(|s| s.gone = true);
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn a_second_close_delivers_nothing_more() {
    let mut init = idle_asked_to_end();
    init.busy = true;
    let mut r = rig(init, None);
    r.ctl.send(Control::Close).unwrap();
    r.ctl.send(Control::Close).unwrap();
    wait_for("the close", || r.with(|s| s.input_closed));
    r.st.queue_append(&r.sid, "system", "stays").unwrap();
    r.end_turn();
    settle();
    assert!(r.delivered().is_empty());
    assert_eq!(r.st.inbox(&r.sid).unwrap().len(), 1);
    r.with(|s| s.gone = true);
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn a_closed_pipe_takes_no_entry_after_the_close() {
    let mut init = idle_asked_to_end();
    init.takes = false;
    let mut r = rig(init, None);
    r.ctl.send(Control::Close).unwrap();
    wait_for("the close", || r.with(|s| s.input_closed));
    r.st.queue_append(&r.sid, "system", "stays").unwrap();
    settle();
    assert!(r.delivered().is_empty());
    assert_eq!(r.st.inbox(&r.sid).unwrap().len(), 1);
    r.with(|s| s.gone = true);
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn a_dropped_control_channel_closes_at_the_safe_point() {
    let mut r = rig(known_busy(), None);
    r.ctl = channel().0;
    settle();
    assert!(!r.with(|s| s.input_closed));
    r.end_turn();
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn the_record_shows_changes_only_and_goes_with_the_exit() {
    let mut r = rig(
        State {
            busy: true,
            ..State::default()
        },
        None,
    );
    settle();
    assert!(r.record().is_none(), "no record before the main session");
    r.with(|s| s.known = true);
    wait_for("the record", || r.record().is_some());
    let one = r.record().unwrap();
    let rec: Record = serde_json::from_slice(&one).unwrap();
    assert_eq!((rec.main.as_str(), rec.sessions.len()), (NATIVE, 1));
    assert!(!rec.sessions[0].idle);
    assert_eq!(rec.pid, std::process::id());
    settle();
    assert_eq!(
        r.record().unwrap(),
        one,
        "the same state is not written again"
    );
    r.end_turn();
    wait_for("the idle record", || {
        let rec: Record = serde_json::from_slice(&r.record().unwrap()).unwrap();
        rec.sessions[0].idle
    });
    r.ctl.send(Control::Interrupt).unwrap();
    wait_for("the interrupt", || r.with(|s| s.interrupts) == 1);
    r.with(|s| s.gone = true);
    assert_eq!(r.end().unwrap(), End::Exited);
    assert!(r.record().is_none());
    assert_eq!(r.with(|s| s.finished), Some(End::Exited));
}

#[test]
fn an_error_of_the_harness_ends_the_custody_without_a_record() {
    let mut r = rig(known_busy(), None);
    wait_for("the record", || r.record().is_some());
    r.with(|s| s.fail = Some("the harness reports session x".into()));
    let e = r.end().unwrap_err();
    assert!(e.to_string().contains("session x"), "{e}");
    assert!(r.record().is_none());
    assert_eq!(
        r.with(|s| s.finished),
        None,
        "no last ingest after an error"
    );
}

fn idle_headless() -> State {
    State {
        known: true,
        exits: true,
        ..State::default()
    }
}

#[test]
fn a_headless_custody_delivers_its_inbox_then_closes() {
    let mut r = rig_ends(idle_headless(), None, Ends::WhenDone, &["dna", "task"]);
    wait_for("the first entry", || r.delivered().len() == 1);
    assert!(!r.with(|s| s.input_closed));
    r.end_turn();
    wait_for("the task", || r.delivered().len() == 2);
    // An append during the last turn keeps the run going.
    r.st.queue_append(&r.sid, "user", "more").unwrap();
    r.end_turn();
    wait_for("the append", || r.delivered().len() == 3);
    settle();
    assert!(!r.with(|s| s.input_closed), "no close during a turn");
    r.end_turn();
    // The control channel stays open: the work ends the custody.
    assert_eq!(r.end().unwrap(), End::Closed);
    assert!(r.st.inbox(&r.sid).unwrap().is_empty());
    let texts: Vec<String> = r
        .delivered()
        .into_iter()
        .map(|i| match i {
            Input::Entry(e) => e.text,
            Input::Typed(t) => t,
        })
        .collect();
    assert_eq!(texts, ["dna", "task", "more"]);
}

#[test]
fn a_headless_custody_with_an_empty_inbox_closes_at_once() {
    let mut r = rig_ends(idle_headless(), None, Ends::WhenDone, &[]);
    assert_eq!(r.end().unwrap(), End::Closed);
    assert!(r.delivered().is_empty());
}

#[test]
fn a_headless_custody_waits_for_its_main_session() {
    let mut init = idle_headless();
    init.known = false;
    let mut r = rig_ends(init, None, Ends::WhenDone, &[]);
    settle();
    assert!(!r.with(|s| s.input_closed), "the session is not known yet");
    r.with(|s| s.known = true);
    assert_eq!(r.end().unwrap(), End::Closed);
}

#[test]
fn a_headless_harness_that_does_not_exit_ends_after_the_close_wait() {
    let mut init = idle_headless();
    init.exits = false;
    let mut r = rig_ends(
        init,
        Some(Duration::from_millis(50)),
        Ends::WhenDone,
        &["task"],
    );
    wait_for("the task", || r.delivered().len() == 1);
    r.end_turn();
    assert_eq!(r.end().unwrap(), End::TimedOut);
    assert_eq!(r.with(|s| s.finished), Some(End::TimedOut));
}

#[test]
fn harness_names_map_to_custodies() {
    assert_eq!(Harness::of("dsh"), Some(Harness::Dsh));
    assert_eq!(Harness::of("claude"), Some(Harness::Claude));
    assert_eq!(Harness::of("codex"), Some(Harness::Codex));
    assert_eq!(Harness::of("gemini"), None);
}

#[test]
fn read_user_ends_with_close() {
    let (tx, rx) = channel();
    read_user(std::io::Cursor::new(b"one\ntwo\n".to_vec()), tx);
    let got: Vec<Control> = rx.iter().collect();
    assert_eq!(
        got,
        [
            Control::User("one".into()),
            Control::User("two".into()),
            Control::Close
        ]
    );
}
