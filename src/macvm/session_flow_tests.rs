//! The start and the quit of a session on Linux: the guest agent's main
//! loop on the fake platform, and on the host side the two machines that
//! the shim runs (`data_start.rs` for the data disk, `quit.rs` for the
//! quit), with a real profile folder, stamp and `events.jsonl`. The test
//! host does what `run_agent.rs` and `run_quit.rs` do with their actions;
//! only the VM itself (the save, the stop) is not there.

use super::data_start::{self, DataStart};
use super::events::Events;
use super::proto::{read_line, write_line, Call, ErrorCode, Event, Outcome, Request};
use super::quit::{self, Quit};
use super::session::{self, Profile, Stamp};
use super::text;
use crate::guest::dialogs::Seen;
use crate::guest::main_loop;
use crate::guest::platform::fake::{Fake, FakeState};
use crate::guest::windows::RawWindow;
use std::collections::VecDeque;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "guest pass word";
const GIB: u64 = 1 << 30;
/// The RAM of the test VM: a save needs 5 GiB free.
const RAM: u64 = 4 * GIB;
/// The line of the fake shim when the quit machine ends suspended.
const SUSPENDED: &str = "suspended";

/// The host side of one start of a session.
struct Host {
    s: UnixStream,
    rx: Receiver<Event>,
    seen: Vec<Event>,
    req: u64,
    data_req: Option<u64>,
    /// The wait for the answer of `data.prepare` that its progress gave.
    data_until: Option<std::time::Instant>,
    quit_req: Option<u64>,
    shutdown_req: Option<u64>,
    profile: Profile,
    events: Events,
    stamp: Stamp,
    password: Option<String>,
    data: DataStart,
    quit: Quit,
    app_started: bool,
    locked: bool,
    lines: Vec<String>,
    exit: Option<i32>,
}

fn agent(fake: Fake, stream: UnixStream) {
    let queue = Arc::new(Mutex::new(VecDeque::from(vec![stream])));
    std::thread::spawn(move || {
        let mut fake = fake;
        main_loop::run(&mut fake, || {
            queue
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| std::io::Error::other("no more connections"))
        })
    });
}

/// A profile folder with the stamp of `session::begin`.
fn profile(dir: &Path, prepared: bool) -> Profile {
    let p = Profile::new(dir.join("app/default"));
    std::fs::create_dir_all(p.logs()).unwrap();
    let mut s = super::session::tests::now().stamp("abcd-0".into());
    s.data_prepared = prepared;
    s.write(&p.stamp()).unwrap();
    p
}

impl Host {
    fn start(p: &Profile, fake: Fake, password: Option<&str>, save: bool) -> Host {
        Self::start_with(p, fake, password, save, false)
    }

    /// `early`: the hello of the data disk goes out right behind the
    /// token, before its reply, as on a busy control line.
    fn start_with(
        p: &Profile,
        fake: Fake,
        password: Option<&str>,
        save: bool,
        early: bool,
    ) -> Host {
        let (a, b) = UnixStream::pair().unwrap();
        agent(fake, a);
        let mut reader = b.try_clone().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(ev)) = read_line::<Event>(&mut reader) {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
        let stamp = session::begin(p, &super::session::tests::now(), session::Start::Cold).unwrap();
        let mut h = Host {
            s: b,
            rx,
            seen: Vec::new(),
            req: 0,
            data_req: None,
            data_until: None,
            quit_req: None,
            shutdown_req: None,
            profile: p.clone(),
            events: Events::open(p),
            stamp,
            password: password.map(str::to_string),
            data: DataStart::new(),
            quit: Quit::new(quit::Config::new("TextEdit", save, RAM)),
            app_started: false,
            locked: false,
            lines: Vec::new(),
            exit: None,
        };
        let hello = h.next(Duration::from_secs(10)).expect("a hello");
        assert!(matches!(hello, Event::Hello(_)), "{hello:?}");
        let req = h.send(Request::Token {
            token: TOKEN.into(),
            v: super::proto::VERSION,
            bench: false,
            now_ms: None,
        });
        if early {
            h.hello(false);
        }
        h.until_reply(req);
        h
    }

    fn next(&mut self, wait: Duration) -> Option<Event> {
        let ev = self.rx.recv_timeout(wait).ok()?;
        self.seen.push(ev.clone());
        Some(ev)
    }

    fn send(&mut self, request: Request) -> u64 {
        self.req += 1;
        let call = Call {
            req: self.req,
            request,
        };
        write_line(&mut self.s, &call).unwrap();
        self.req
    }

    fn until_reply(&mut self, req: u64) -> Outcome {
        loop {
            match self.next(Duration::from_secs(10)) {
                Some(Event::Reply(r)) if r.req == req => return r.outcome,
                Some(_) => {}
                None => panic!("no reply to {req}: {:?}", self.seen),
            }
        }
    }

    /// The hello of `run_agent.rs`: the data disk machine decides.
    fn hello(&mut self, warm: bool) {
        let actions = self.data.hello(&data_start::Hello {
            prepared: self.stamp.data_prepared,
            warm,
            password: self.password.as_deref(),
            data_id: &self.stamp.data_id.clone(),
        });
        self.data_actions(actions);
    }

    fn data_actions(&mut self, actions: Vec<data_start::Action>) {
        use data_start::Action;
        for a in actions {
            match a {
                Action::Send { password, data_id } => {
                    self.data_req = Some(self.send(Request::DataPrepare { password, data_id }));
                }
                Action::StartApp => self.app_started = true,
                Action::Event { kind, text } => self.events.record(kind, &text, &[]),
                Action::Line(l) => self.lines.push(l),
                Action::Stamp { prepared, error } => {
                    self.stamp.data_prepared = prepared;
                    self.stamp.data_error = error;
                    self.stamp.write(&self.profile.stamp()).unwrap();
                }
                Action::EndLocked { detail } => {
                    self.events.record("error", &detail, &[]);
                    self.locked = true;
                }
                Action::Extend(d) => self.data_until = Some(std::time::Instant::now() + d),
            }
        }
    }

    /// Wait for the answer of `data.prepare`, at most `wait`: then the
    /// timeout of the request, as the shim's tick gives it.
    fn data_answer(&mut self, wait: Duration) {
        let Some(req) = self.data_req else { return };
        let end = std::time::Instant::now() + wait;
        loop {
            let end = self.data_until.map_or(end, |u| u.max(end));
            let left = end.saturating_duration_since(std::time::Instant::now());
            match self.next(left) {
                Some(Event::Reply(r)) if r.req == req => {
                    let actions = self.data.answer(&r.outcome);
                    self.data_actions(actions);
                    return;
                }
                Some(Event::DataProgress { text, wait_secs }) => {
                    let actions = self.data.progress(&text, Duration::from_secs(wait_secs));
                    self.data_actions(actions);
                }
                Some(_) => {}
                None => {
                    let actions = self.data.timeout(wait);
                    self.data_actions(actions);
                    return;
                }
            }
        }
    }

    /// The quit of `run_quit.rs`, with `free` bytes on the volume. The
    /// guest's `shutdown` answered Ok stands for the guest that stops.
    fn quit(&mut self, free: u64) -> i32 {
        let (save, actions) = self.data.at_quit(&data_start::AtQuit {
            save: true,
            prepared: self.stamp.data_prepared,
            password: self.password.is_some(),
            free,
            needs: RAM + session::SAVE_EXTRA,
        });
        self.data_actions(actions);
        if !save && self.quit.phase() == quit::Phase::Idle {
            self.quit = Quit::new(quit::Config::new("TextEdit", false, RAM));
        }
        let actions = self.quit.step(quit::Input::Begin { agent: true });
        self.act(actions, free);
        while self.exit.is_none() {
            let ev = self
                .next(Duration::from_secs(10))
                .expect("an answer in the quit");
            if let Event::Reply(r) = ev {
                if Some(r.req) == self.quit_req {
                    if let Some(input) = quit::quit_answer(&r.outcome) {
                        let actions = self.quit.step(input);
                        self.act(actions, free);
                    }
                } else if Some(r.req) == self.shutdown_req {
                    let input = match r.outcome {
                        Outcome::Ok { .. } => quit::Input::GuestStopped,
                        Outcome::Error { text, .. } => quit::Input::ShutdownRefused(text),
                    };
                    let actions = self.quit.step(input);
                    self.act(actions, free);
                }
            }
        }
        self.exit.unwrap()
    }

    fn act(&mut self, actions: Vec<quit::Action>, free: u64) {
        use quit::Action;
        for a in actions {
            let more = match a {
                Action::SendQuit { force } => {
                    self.quit_req = Some(self.send(Request::AppQuit {
                        app: "com.apple.TextEdit".into(),
                        force,
                    }));
                    vec![]
                }
                Action::Deadline(_) | Action::CloseWindows => vec![],
                Action::CheckSpace => self.quit.step(quit::Input::Space(free)),
                Action::Pause => self.quit.step(quit::Input::Paused(Ok(()))),
                Action::Save => self.quit.step(quit::Input::Saved(Ok(RAM))),
                Action::CommitState | Action::RemovePartial | Action::Resume => vec![],
                Action::SendShutdown => match self.password.clone() {
                    Some(password) => {
                        self.shutdown_req = Some(self.send(Request::Shutdown { password }));
                        vec![]
                    }
                    None => self.quit.step(quit::Input::ShutdownRefused(
                        text::shutdown_no_session_password(),
                    )),
                },
                Action::Stop => self.quit.step(quit::Input::Stopped(Ok(()))),
                Action::Stamp { clean, saved } => {
                    session::finish(&self.profile, &mut self.stamp, clean, saved, 1_762_250_000)
                        .unwrap();
                    vec![]
                }
                Action::Line(l) => {
                    self.lines.push(l);
                    vec![]
                }
                Action::Event { kind, text } => {
                    self.events.record(&kind, &text, &[]);
                    vec![]
                }
                Action::Exit(code) => {
                    self.exit = Some(code);
                    vec![]
                }
                Action::Suspended => {
                    self.lines.push(SUSPENDED.into());
                    self.exit = Some(0);
                    vec![]
                }
                Action::SuspendRefused(l) => {
                    self.lines.push(l);
                    vec![]
                }
            };
            self.act(more, free);
        }
    }

    fn events_text(&self) -> String {
        std::fs::read_to_string(self.profile.events_log()).unwrap_or_default()
    }

    fn disk_stamp(&self) -> Stamp {
        Stamp::read(&self.profile.stamp()).unwrap()
    }
}

fn fake() -> (Fake, Arc<Mutex<FakeState>>) {
    Fake::new()
}

#[test]
fn a_new_profile_moves_its_home_to_the_data_disk_and_quits_clean() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    assert!(!h.app_started, "the app waits for the home");
    h.data_answer(Duration::from_secs(10));
    assert!(h.app_started);
    let data_id = h.stamp.data_id.clone();
    assert_eq!(st.lock().unwrap().data_prepares, vec![data_id]);
    assert_eq!(st.lock().unwrap().passwords, vec![PASSWORD]);
    let s = h.disk_stamp();
    assert!(s.data_prepared);
    assert_eq!(s.data_error, None);
    let ev = h.events_text();
    assert!(
        ev.contains("the data disk is prepared (verify: verified)"),
        "{ev}"
    );
    // The quit of the nospace check: 2 GiB free, so no save, and the
    // guest shuts down with the password. The stamp is clean.
    assert_eq!(h.quit(2 * GIB), 0);
    let s = h.disk_stamp();
    assert!(s.clean, "{s:?}");
    assert!(s.state_id.is_none());
    assert_eq!(st.lock().unwrap().shutdowns, 1);
    assert_eq!(st.lock().unwrap().passwords, vec![PASSWORD, PASSWORD]);
    let ev = h.events_text();
    assert!(ev.contains("not enough space for the saved state"), "{ev}");
    assert!(ev.contains("\"text\":\"quit without save\""), "{ev}");
    assert!(!ev.contains("stopped without save"), "{ev}");

    // The next start: the stamp says prepared, nothing goes to the guest.
    let (f, st) = fake();
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    assert!(h.app_started);
    assert_eq!(h.data_req, None);
    // A quit with space saves the state and shuts nothing down.
    assert_eq!(h.quit(100 * GIB), 0);
    assert_eq!(st.lock().unwrap().shutdowns, 0);
    let s = h.disk_stamp();
    assert!(s.clean && s.data_prepared);
    assert_eq!(s.state_id.as_deref(), Some(s.data_id.as_str()));
}

#[test]
fn a_failed_step_is_named_the_app_starts_and_the_next_start_tries_again() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    st.lock().unwrap().data_prepare_result = Some((
        ErrorCode::Io,
        text::data_step_failed(
            crate::guest::data::STEP_COPY,
            "ditto: No space left on device",
        ),
    ));
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    h.data_answer(Duration::from_secs(10));
    assert!(h.app_started, "the app runs on the system disk");
    let s = h.disk_stamp();
    assert!(!s.data_prepared);
    let error = s.data_error.clone().unwrap();
    assert!(
        error.contains("copy the home onto the volume: ditto"),
        "{error}"
    );
    assert!(h.events_text().contains("copy the home onto the volume"));
    let row = text::status_data_row(s.data_prepared, s.data_error.as_deref());
    assert!(row.starts_with("not prepared: "), "{row}");
    assert!(row.contains("copy the home onto the volume"), "{row}");
    // With space to save, the quit saves nothing: a saved state would make
    // the next start warm, and a warm start never prepares.
    assert_eq!(h.quit(100 * GIB), 0);
    let s = h.disk_stamp();
    assert!(s.clean);
    assert!(s.state_id.is_none(), "{s:?}");
    assert_eq!(st.lock().unwrap().shutdowns, 1);
    assert!(h
        .events_text()
        .contains("the data disk is not prepared yet"));

    // The next cold start sends the prepare again, and it works.
    let (f, st) = fake();
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    h.data_answer(Duration::from_secs(10));
    assert_eq!(st.lock().unwrap().data_prepares.len(), 1);
    let s = h.disk_stamp();
    assert!(s.data_prepared);
    assert_eq!(s.data_error, None, "the success clears the old failure");
}

#[test]
fn a_prepare_without_an_answer_times_out_and_the_app_starts() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    {
        let mut st = st.lock().unwrap();
        st.data_prepare_delay = Duration::from_millis(1500);
        st.data_prepare_quiet = true;
    }
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    h.data_answer(Duration::from_millis(200));
    assert!(h.app_started);
    let s = h.disk_stamp();
    assert!(!s.data_prepared);
    assert!(s
        .data_error
        .unwrap()
        .contains("did not answer data.prepare"));
    // The late answer changes nothing of this start.
    let req = h.data_req.unwrap();
    let late = h.until_reply(req);
    assert!(matches!(late, Outcome::Ok { .. }));
    assert!(h.data.answer(&late).is_empty());
    assert!(!h.disk_stamp().data_prepared);
}

#[test]
fn without_a_password_nothing_is_prepared_and_the_quit_says_why() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    let mut h = Host::start(&p, f, None, true);
    h.hello(false);
    assert!(h.app_started);
    assert_eq!(h.data_req, None);
    let s = h.disk_stamp();
    assert!(s.data_error.unwrap().contains("no guest password"));
    assert!(h.lines.iter().any(|l| l.contains("keyring")));
    assert_eq!(h.quit(2 * GIB), 0);
    // No shutdown request without a password: the VM stops, not clean,
    // and the event says why.
    assert_eq!(st.lock().unwrap().shutdowns, 0);
    assert!(!h.disk_stamp().clean);
    let ev = h.events_text();
    assert!(ev.contains("no guest password for the shutdown"), "{ev}");
    assert!(ev.contains("stopped without save"), "{ev}");
}

#[test]
fn a_disk_of_another_system_disk_ends_the_start_untouched() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    st.lock().unwrap().data_prepare_result = Some((
        ErrorCode::Locked,
        text::data_step_failed(crate::guest::data::STEP_MOUNT, "no mount"),
    ));
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    h.data_answer(Duration::from_secs(10));
    assert!(h.locked);
    assert!(!h.app_started);
    assert!(h.events_text().contains("mount the volume: no mount"));
}

#[test]
fn a_disk_dialog_of_the_boot_is_pressed_before_the_prepare_runs() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    {
        let mut st = st.lock().unwrap();
        st.windows = vec![RawWindow {
            id: 7,
            pid: 90,
            owner: "DiskArbitrationAgent".into(),
            bundle: None,
            layer: 0,
            bounds: super::proto::Frame::new(0.0, 0.0, 400.0, 200.0),
            alpha: 1.0,
        }];
        st.dialog = Seen {
            texts: vec!["The disk you attached was not readable by this computer.".into()],
            buttons: vec!["Initialize…".into(), "Ignore".into(), "Eject".into()],
            pressed: Some("Ignore".into()),
            why: None,
        };
        st.data_prepare_delay = Duration::from_millis(100);
        // The prepare is queued before the loop of the agent starts.
        st.data_check_delay = Duration::from_millis(200);
    }
    let mut h = Host::start_with(&p, f, Some(PASSWORD), true, true);
    h.data_answer(Duration::from_secs(10));
    assert!(h.disk_stamp().data_prepared);
    let dialog = h
        .seen
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::Dialog {
                    dismissed: true,
                    ..
                }
            )
        })
        .expect("a dialog event");
    let reply = h
        .seen
        .iter()
        .position(|e| matches!(e, Event::Reply(r) if Some(r.req) == h.data_req))
        .unwrap();
    assert!(dialog < reply, "{:?}", h.seen);
    assert_eq!(st.lock().unwrap().dialog_asks, vec![7]);
}

#[test]
fn a_quit_while_the_home_is_copied_ends_quickly_and_clean() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    st.lock().unwrap().data_prepare_delay = Duration::from_secs(60);
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    let begun = std::time::Instant::now();
    h.hello(false);
    // The progress of the copy comes, and the host waits longer for it.
    loop {
        match h.next(Duration::from_secs(10)).expect("a progress line") {
            Event::DataProgress { text, wait_secs } => {
                let actions = h.data.progress(&text, Duration::from_secs(wait_secs));
                assert!(actions.contains(&data_start::Action::Extend(
                    Duration::from_secs(wait_secs) + data_start::PROGRESS_MARGIN
                )));
                break;
            }
            _ => continue,
        }
    }
    assert!(!h.app_started);
    // The stop of the user: the agent answers the quit at once, stops the
    // copy at the shutdown, and the stamp is clean, long before the 60 s.
    assert_eq!(h.quit(100 * GIB), 0);
    assert!(
        begun.elapsed() < Duration::from_secs(15),
        "{:?}",
        begun.elapsed()
    );
    assert!(!h.app_started, "no app starts during the quit");
    let s = h.disk_stamp();
    assert!(s.clean, "{s:?}");
    assert!(!s.data_prepared);
    assert!(s.state_id.is_none());
    assert!(s.data_error.unwrap().contains("quit came while"));
    let ev = h.events_text();
    assert!(
        ev.contains("the quit came while the guest prepared"),
        "{ev}"
    );
    assert!(ev.contains("\"text\":\"quit without save\""), "{ev}");
    let st = st.lock().unwrap();
    assert_eq!((st.data_prepare_cancels, st.shutdowns), (1, 1));
}

#[test]
fn a_quit_after_a_failed_prepare_names_too_little_space() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    st.lock().unwrap().data_prepare_result = Some((
        ErrorCode::Timeout,
        text::data_copy_timed_out(Duration::from_secs(60)),
    ));
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    h.data_answer(Duration::from_secs(10));
    assert!(h.app_started);
    assert_eq!(h.quit(2 * GIB), 0);
    assert!(
        h.lines
            .iter()
            .any(|l| l.contains("not enough space for the saved state")),
        "{:?}",
        h.lines
    );
    assert!(h.disk_stamp().clean);
}

#[test]
fn a_suspend_saves_without_quitting_the_app_and_the_stamp_resumes_warm() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path(), false);
    let (f, st) = fake();
    let mut h = Host::start(&p, f, Some(PASSWORD), true);
    h.hello(false);
    h.data_answer(Duration::from_secs(10));
    assert!(h.app_started);
    // Too little space: refused, the session runs on, nothing stopped.
    let actions = h.quit.step(quit::Input::Suspend { agent: true });
    h.act(actions, 2 * GIB);
    assert_eq!(h.exit, None);
    assert!(
        h.lines.iter().any(|l| l.contains("not enough space")),
        "{:?}",
        h.lines
    );
    // With space: saved, suspended, the app never asked to quit.
    let actions = h.quit.step(quit::Input::Suspend { agent: true });
    h.act(actions, 100 * GIB);
    assert_eq!(h.exit, Some(0));
    assert_eq!(h.lines.last().map(String::as_str), Some(SUSPENDED));
    assert_eq!(h.quit_req, None);
    assert_eq!(st.lock().unwrap().shutdowns, 0);
    let s = h.disk_stamp();
    assert!(s.clean, "{s:?}");
    assert_eq!(s.state_id.as_deref(), Some(s.data_id.as_str()));
    let ev = h.events_text();
    assert!(ev.contains("\"kind\":\"suspend\""), "{ev}");
    assert!(!ev.contains("quit with save"), "{ev}");
}
