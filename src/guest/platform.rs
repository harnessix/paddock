//! What the main loop needs from the guest system. The macOS agent
//! implements it with AppKit, Accessibility and the guest commands
//! (`macos.rs`); the Linux tests implement it with a fake, so the loop,
//! the dispatch and the reconnect are tested on every platform.

use super::windows::{AxInfo, ProcTable, RawWindow};
use crate::macvm::axmirror::Node;
use crate::macvm::clipboard::Item;
use crate::macvm::proto::{Display, ErrorCode, Frame, MenuItem, Window};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The guest's monotonic clock in milliseconds: since the agent started.
pub fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// A failed operation: the code for the host and the text for the log.
pub type Fail = (ErrorCode, String);

/// Wakes the agent thread out of [`Platform::wait`] from another thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The work of one `data.prepare`, run in a thread of its own.
pub type PrepareJob = Box<dyn FnOnce(&super::data_copy::Watch) -> Result<String, Fail> + Send>;

/// What the agent says about itself in `hello`.
#[derive(Debug, Clone, PartialEq)]
pub struct HelloInfo {
    pub agent: String,
    pub macos: String,
    pub build: String,
    pub scale: f64,
    pub display: Display,
    pub can_update: bool,
}

/// The numbers of a `health` event.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HealthInfo {
    pub uptime: u64,
    pub load: f64,
    pub mem_free: u64,
    pub data_disk_free: Option<u64>,
    pub input_source: Option<String>,
    /// The agent has the Accessibility permission now.
    pub accessibility: bool,
}

/// A request on one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowOp {
    Move(Frame),
    Raise,
    Minimize,
    Close,
    Focus,
}

/// The guest system, as the main loop sees it.
pub trait Platform {
    /// Wait up to `d`, serving the run loop meanwhile. Returns early
    /// after a call of the waker.
    fn wait(&mut self, d: Duration);
    fn waker(&self) -> Waker;
    /// True once after an Accessibility notification: look now.
    fn take_dirty(&mut self) -> bool;
    fn hello(&mut self) -> HelloInfo;
    fn health(&mut self) -> HealthInfo;
    /// A configure step of the agent itself (`@agent OP ARGS`, see
    /// `macvm::configure::AGENT_OP`), on the main thread: the success and
    /// the output.
    fn agent_step(&mut self, argv: &[String]) -> (bool, String) {
        (false, crate::macvm::text::agent_step_unknown(argv))
    }
    /// The guest's monotonic clock in milliseconds, for `pong`.
    fn monotonic_ms(&self) -> u64;
    /// The window server list, front first.
    fn raw_windows(&mut self) -> Vec<RawWindow>;
    fn procs(&mut self) -> ProcTable;
    /// The pid of the front application.
    fn front_pid(&mut self) -> Option<u32>;
    /// The Accessibility facts of one window, with the 1 s timeout.
    fn ax_info(&mut self, w: &RawWindow) -> AxInfo;
    /// The corner radius of a window in points, measured on its image
    /// (`capture.rs`). None without the image: no Screen Recording, a
    /// window not drawn yet, a platform without windows.
    fn window_radius(&mut self, _w: &RawWindow) -> Option<f64> {
        None
    }
    /// Keep one Accessibility observer per followed pid.
    fn observe(&mut self, pids: &BTreeSet<u32>);
    fn window_op(&mut self, w: &Window, op: WindowOp) -> Result<(), Fail>;
    /// Launch the app and give its pid.
    fn launch(
        &mut self,
        path: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<u32, Fail>;
    /// Ask the app to quit. The exit shows in the process table later.
    fn quit(&mut self, pid: u32, force: bool) -> Result<(), Fail>;
    fn crash_report(&mut self, pid: u32, since: SystemTime) -> Option<PathBuf>;
    /// `data.prepare`: the job that gives the verify output after the
    /// move. It runs in a thread of its own, so the agent answers the quit
    /// while the home is copied; the watch cancels it and takes its
    /// progress.
    fn data_prepare(&mut self, password: &str, data_id: &str) -> PrepareJob;
    /// `input.set`: the ids the guest does not have.
    fn input_set(
        &mut self,
        sources: &[String],
        current: &str,
        reduce_motion: bool,
        increase_contrast: bool,
    ) -> Result<Vec<String>, Fail>;
    /// `agent.update`: the new binary is in place after Ok.
    fn update(&mut self, path: &Path, sha256: &str) -> Result<(), Fail>;
    /// `log.fetch`: send the log over `guest-files` with the token.
    fn send_log(&mut self, token: &str) -> Result<(), Fail>;
    fn disk_low_alert(&mut self);
    /// `shutdown`: `sudo -S shutdown -h now` with the guest password
    /// (`shutdown.rs`). Ok when the halt started.
    fn shutdown(&mut self, password: &str) -> Result<(), Fail>;
    /// The focused text field of the app and its text, when it changed
    /// since the last call. Only asked while the bench watches.
    fn ax_value(&mut self) -> Option<(u32, String)>;
    /// The result of the data disk check of an unclean session, once.
    fn data_check(&mut self) -> Option<String>;
    /// The error of a data disk that does not mount, once.
    fn data_unmountable(&mut self) -> Option<String>;
    /// Read a disk dialog window and press its Ignore button
    /// (`dialogs.rs`). The default reads nothing.
    fn dialog_dismiss(&mut self, _w: &RawWindow) -> super::dialogs::Seen {
        super::dialogs::Seen {
            why: Some("this platform cannot read dialogs".into()),
            ..Default::default()
        }
    }
    /// The guest screen is locked now (`CGSSessionScreenIsLocked` of the
    /// session dictionary). The default sees no lock.
    fn screen_locked(&mut self) -> bool {
        false
    }
    /// The seconds since the last input in the guest, when known.
    fn idle_secs(&mut self) -> Option<u64> {
        None
    }
    /// `bench.lock`: lock the guest screen now.
    fn lock_screen(&mut self) -> Result<(), Fail> {
        Err((
            ErrorCode::Unsupported,
            crate::macvm::text::screen_lock_unsupported(),
        ))
    }
    /// `keychain.unlock`: unlock the login keychain with the password
    /// (never on a command line) and point the search list and the
    /// default keychain at the home; the summary of what it did.
    fn keychain_unlock(&mut self, _password: &str) -> Result<String, Fail> {
        Err((
            ErrorCode::Unsupported,
            crate::macvm::text::keychain_unsupported(),
        ))
    }
    /// `bench.keychain`: the state of the login keychain as text.
    fn keychain_info(&mut self) -> Result<String, Fail> {
        Err((
            ErrorCode::Unsupported,
            crate::macvm::text::keychain_unsupported(),
        ))
    }
    /// The file that remembers the app of the session across a restart
    /// of the agent (a crash, an update): the pid and the path. None
    /// means no memory.
    fn state_file(&self) -> Option<PathBuf>;
    /// The change count of the guest pasteboard.
    fn clipboard_count(&mut self) -> u64;
    /// The items of the guest pasteboard, with their types and the three
    /// markers.
    fn clipboard_read(&mut self) -> Vec<Item>;
    /// `clipboard.set`: replace the guest pasteboard; the new change
    /// count, which the poll skips.
    fn clipboard_write(&mut self, items: &[Item]) -> Result<u64, Fail>;
    /// The PNG of a TIFF of the guest pasteboard, when
    /// the platform reads it.
    fn tiff_to_png(&mut self, _tiff: &[u8]) -> Option<Vec<u8>> {
        None
    }
    /// A clipboard set too big for a line: the JSON over `guest-files`;
    /// the transfer name.
    fn send_clipboard(&mut self, token: &str, count: u64, json: &[u8]) -> Result<String, Fail>;
    /// `drop`: the file URLs on the pasteboard, the files opened with the
    /// app of `pid`; the new pasteboard change count.
    fn open_with_app(&mut self, paths: &[PathBuf], pid: Option<u32>) -> Result<u64, Fail>;
    /// Make sure the Downloads folder of the home is a link to the
    /// downloads share. True once it is.
    fn downloads_link(&mut self) -> bool;
    /// True once after a menu notification of the app (a menu opened, an
    /// item selected): read the menu bar again.
    fn take_menu_dirty(&mut self) -> bool;
    /// The menu bar of the app, without the Apple menu and the app menu,
    /// two levels deep, not yet capped. Each Accessibility call has the
    /// 1 s timeout, and the first one that times out ends the read.
    fn menu_read(&mut self, pid: u32) -> Result<Vec<MenuItem>, Fail>;
    /// `menu.press`: `kAXPressAction` on the item with this path.
    fn menu_press(&mut self, pid: u32, path: &[usize]) -> Result<(), Fail>;
    /// True once after a notification that changes the tree of a window
    /// (the focused element, a value, the layout): read it again.
    fn take_ax_tree_dirty(&mut self) -> bool;
    /// The tree of a window four levels deep, not capped, and whether
    /// the read stopped at the node limit.
    fn ax_tree(&mut self, w: &Window) -> Result<(Vec<Node>, bool), Fail>;
    /// `ax.press`: `kAXPressAction` on the element at this path, when
    /// its role is still `role`.
    fn ax_tree_press(&mut self, w: &Window, path: &[usize], role: &str) -> Result<(), Fail>;
    /// `ax.set`: the value of the element at this path.
    fn ax_tree_set(
        &mut self,
        w: &Window,
        path: &[usize],
        role: &str,
        value: &str,
    ) -> Result<(), Fail>;
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// What the test controls and reads.
    #[derive(Default)]
    pub struct FakeState {
        pub windows: Vec<RawWindow>,
        pub procs: ProcTable,
        pub front: Option<u32>,
        pub titles: BTreeMap<u32, String>,
        pub observed: BTreeSet<u32>,
        pub ops: Vec<(u32, WindowOp)>,
        pub op_result: Option<Fail>,
        pub launched: Vec<PathBuf>,
        pub launch_pid: u32,
        pub quits: Vec<(u32, bool)>,
        pub crash: Option<PathBuf>,
        pub inputs: Vec<String>,
        pub missing: Vec<String>,
        pub updates: Vec<(PathBuf, String)>,
        pub update_result: Option<Fail>,
        pub logs_sent: Vec<String>,
        pub alerts: u32,
        pub shutdowns: u32,
        /// The answer of the next `shutdown`: `None` is Ok.
        pub shutdown_result: Option<Fail>,
        pub ax_values: Vec<(u32, String)>,
        pub data_prepares: Vec<String>,
        /// The passwords of `data.prepare` and `shutdown`, in order.
        pub passwords: Vec<String>,
        /// The answer of the next `data.prepare`: `None` is Ok, and the
        /// time it takes.
        pub data_prepare_result: Option<Fail>,
        pub data_prepare_delay: Duration,
        /// The prepares that a cancel stopped.
        pub data_prepare_cancels: u32,
        /// The prepare sends no progress line, as a guest that hangs.
        pub data_prepare_quiet: bool,
        /// A wait in the data check after the hello: the requests that
        /// follow the token are queued by then.
        pub data_check_delay: Duration,
        pub data_check: Option<String>,
        /// The dialog that the fake reads, and the windows it was asked
        /// about.
        pub dialog: crate::guest::dialogs::Seen,
        pub dialog_asks: Vec<u32>,
        pub health: HealthInfo,
        pub can_update: bool,
        pub state_file: Option<PathBuf>,
        pub clip_count: u64,
        pub clip_items: Vec<Item>,
        pub clip_written: Vec<Vec<Item>>,
        pub clip_sent: Vec<(u64, Vec<u8>)>,
        pub opened: Vec<(Vec<PathBuf>, Option<u32>)>,
        pub downloads_checks: u32,
        /// The menu bar the fake reports, and the error instead of it.
        pub menu: Vec<MenuItem>,
        pub menu_result: Option<Fail>,
        pub menu_reads: u32,
        pub menu_dirty: bool,
        pub presses: Vec<Vec<usize>>,
        pub press_result: Option<Fail>,
        /// The tree the fake reports for any window, and the error
        /// instead of it.
        pub ax_tree: Vec<Node>,
        pub ax_tree_result: Option<Fail>,
        pub ax_tree_reads: u32,
        pub ax_tree_dirty: bool,
        pub ax_presses: Vec<(u32, Vec<usize>, String)>,
        pub ax_sets: Vec<(u32, Vec<usize>, String, String)>,
        pub ax_action_result: Option<Fail>,
        /// The screen lock that the fake reports, and the reads of it.
        pub locked: bool,
        pub lock_reads: u32,
        pub locks: u32,
        /// The passwords of `keychain.unlock`, and its error instead.
        pub keychain_unlocks: Vec<String>,
        pub keychain_result: Option<Fail>,
        /// The chrome the fake reports: title bars, full screen windows,
        /// measured radii, and the windows whose radius was asked for.
        pub title_bars: BTreeMap<u32, f64>,
        pub full_screen: BTreeSet<u32>,
        pub radii: BTreeMap<u32, f64>,
        pub radius_asks: Vec<u32>,
    }

    pub struct Fake {
        pub st: Arc<Mutex<FakeState>>,
        pub dirty: Arc<AtomicBool>,
        pub woken: Arc<AtomicBool>,
    }

    impl Fake {
        pub fn new() -> (Self, Arc<Mutex<FakeState>>) {
            let st = Arc::new(Mutex::new(FakeState {
                launch_pid: 100,
                can_update: true,
                ..FakeState::default()
            }));
            let f = Fake {
                st: st.clone(),
                dirty: Arc::new(AtomicBool::new(false)),
                woken: Arc::new(AtomicBool::new(false)),
            };
            (f, st)
        }

        fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.st.lock().unwrap()
        }
    }

    impl Platform for Fake {
        fn wait(&mut self, d: Duration) {
            let start = std::time::Instant::now();
            while start.elapsed() < d {
                if self.woken.swap(false, Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        fn waker(&self) -> Waker {
            let w = self.woken.clone();
            Arc::new(move || w.store(true, Ordering::SeqCst))
        }

        fn take_dirty(&mut self) -> bool {
            self.dirty.swap(false, Ordering::SeqCst)
        }

        fn hello(&mut self) -> HelloInfo {
            HelloInfo {
                agent: "0.2.1".into(),
                macos: "15.2".into(),
                build: "24C101".into(),
                scale: 2.0,
                display: Display { w: 1728, h: 1117 },
                can_update: self.st().can_update,
            }
        }

        fn health(&mut self) -> HealthInfo {
            self.st().health.clone()
        }

        fn monotonic_ms(&self) -> u64 {
            42
        }

        fn raw_windows(&mut self) -> Vec<RawWindow> {
            self.st().windows.clone()
        }

        fn procs(&mut self) -> ProcTable {
            self.st().procs.clone()
        }

        fn front_pid(&mut self) -> Option<u32> {
            self.st().front
        }

        fn ax_info(&mut self, w: &RawWindow) -> AxInfo {
            let st = self.st();
            AxInfo {
                title: st.titles.get(&w.id).cloned().unwrap_or_default(),
                hint: None,
                minimized: false,
                focused: st.front == Some(w.pid)
                    && st.windows.iter().find(|x| x.pid == w.pid).map(|x| x.id) == Some(w.id),
                resizable: true,
                movable: true,
                title_bar: st.title_bars.get(&w.id).copied(),
                full_screen: st.full_screen.contains(&w.id),
                ..AxInfo::default()
            }
        }

        fn window_radius(&mut self, w: &RawWindow) -> Option<f64> {
            let mut st = self.st();
            st.radius_asks.push(w.id);
            st.radii.get(&w.id).copied()
        }

        fn observe(&mut self, pids: &BTreeSet<u32>) {
            self.st().observed = pids.clone();
        }

        fn window_op(&mut self, w: &Window, op: WindowOp) -> Result<(), Fail> {
            let mut st = self.st();
            st.ops.push((w.id, op));
            if let Some(e) = &st.op_result {
                return Err(e.clone());
            }
            if let WindowOp::Move(f) = op {
                if let Some(raw) = st.windows.iter_mut().find(|x| x.id == w.id) {
                    raw.bounds = f;
                }
            }
            Ok(())
        }

        fn launch(
            &mut self,
            path: &Path,
            _args: &[String],
            _env: &BTreeMap<String, String>,
        ) -> Result<u32, Fail> {
            let mut st = self.st();
            st.launched.push(path.to_path_buf());
            let pid = st.launch_pid;
            st.procs.insert(pid, 1);
            Ok(pid)
        }

        fn quit(&mut self, pid: u32, force: bool) -> Result<(), Fail> {
            self.st().quits.push((pid, force));
            Ok(())
        }

        fn crash_report(&mut self, _pid: u32, _since: SystemTime) -> Option<PathBuf> {
            self.st().crash.clone()
        }

        fn data_prepare(&mut self, password: &str, data_id: &str) -> PrepareJob {
            {
                let mut st = self.st();
                st.data_prepares.push(data_id.to_string());
                st.passwords.push(password.to_string());
            }
            let st = self.st.clone();
            Box::new(move |watch| {
                let (delay, quiet) = {
                    let st = st.lock().unwrap();
                    (st.data_prepare_delay, st.data_prepare_quiet)
                };
                if !quiet {
                    (watch.progress)(
                        crate::macvm::text::data_step_now("fake"),
                        delay + Duration::from_secs(1),
                    );
                }
                let start = std::time::Instant::now();
                while start.elapsed() < delay {
                    if watch.cancelled() {
                        st.lock().unwrap().data_prepare_cancels += 1;
                        return watch.check().map(|_| String::new());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                st.lock()
                    .unwrap()
                    .data_prepare_result
                    .take()
                    .map_or(Ok("verified".into()), Err)
            })
        }

        fn input_set(
            &mut self,
            _sources: &[String],
            current: &str,
            _rm: bool,
            _ic: bool,
        ) -> Result<Vec<String>, Fail> {
            let mut st = self.st();
            st.inputs.push(current.to_string());
            Ok(st.missing.clone())
        }

        fn update(&mut self, path: &Path, sha256: &str) -> Result<(), Fail> {
            let mut st = self.st();
            st.updates.push((path.to_path_buf(), sha256.to_string()));
            match &st.update_result {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        fn send_log(&mut self, token: &str) -> Result<(), Fail> {
            self.st().logs_sent.push(token.to_string());
            Ok(())
        }

        fn disk_low_alert(&mut self) {
            self.st().alerts += 1;
        }

        fn shutdown(&mut self, password: &str) -> Result<(), Fail> {
            let mut st = self.st();
            st.shutdowns += 1;
            st.passwords.push(password.to_string());
            if password.is_empty() {
                return Err((ErrorCode::Io, crate::macvm::text::shutdown_no_password()));
            }
            st.shutdown_result.take().map_or(Ok(()), Err)
        }

        fn ax_value(&mut self) -> Option<(u32, String)> {
            let mut st = self.st();
            if st.ax_values.is_empty() {
                None
            } else {
                Some(st.ax_values.remove(0))
            }
        }

        fn data_check(&mut self) -> Option<String> {
            let delay = self.st().data_check_delay;
            std::thread::sleep(delay);
            self.st().data_check.take()
        }

        fn data_unmountable(&mut self) -> Option<String> {
            None
        }

        fn dialog_dismiss(&mut self, w: &RawWindow) -> crate::guest::dialogs::Seen {
            let mut st = self.st();
            st.dialog_asks.push(w.id);
            st.dialog.clone()
        }

        fn state_file(&self) -> Option<PathBuf> {
            self.st().state_file.clone()
        }

        fn screen_locked(&mut self) -> bool {
            let mut st = self.st();
            st.lock_reads += 1;
            st.locked
        }

        fn idle_secs(&mut self) -> Option<u64> {
            Some(7)
        }

        fn lock_screen(&mut self) -> Result<(), Fail> {
            let mut st = self.st();
            st.locks += 1;
            st.locked = true;
            Ok(())
        }

        fn keychain_unlock(&mut self, password: &str) -> Result<String, Fail> {
            let mut st = self.st();
            st.keychain_unlocks.push(password.to_string());
            st.keychain_result.take().map_or(Ok("unlocked".into()), Err)
        }

        fn keychain_info(&mut self) -> Result<String, Fail> {
            Ok("status unlocked".into())
        }

        fn clipboard_count(&mut self) -> u64 {
            self.st().clip_count
        }

        fn clipboard_read(&mut self) -> Vec<Item> {
            self.st().clip_items.clone()
        }

        fn clipboard_write(&mut self, items: &[Item]) -> Result<u64, Fail> {
            let mut st = self.st();
            st.clip_written.push(items.to_vec());
            st.clip_count += 1;
            st.clip_items = items.to_vec();
            Ok(st.clip_count)
        }

        fn send_clipboard(
            &mut self,
            _token: &str,
            count: u64,
            json: &[u8],
        ) -> Result<String, Fail> {
            self.st().clip_sent.push((count, json.to_vec()));
            Ok(format!("clip-{count}.json")) // not user text
        }

        fn open_with_app(&mut self, paths: &[PathBuf], pid: Option<u32>) -> Result<u64, Fail> {
            let mut st = self.st();
            st.opened.push((paths.to_vec(), pid));
            st.clip_count += 1;
            Ok(st.clip_count)
        }

        fn downloads_link(&mut self) -> bool {
            self.st().downloads_checks += 1;
            true
        }

        fn take_menu_dirty(&mut self) -> bool {
            std::mem::take(&mut self.st().menu_dirty)
        }

        fn menu_read(&mut self, _pid: u32) -> Result<Vec<MenuItem>, Fail> {
            let mut st = self.st();
            st.menu_reads += 1;
            match &st.menu_result {
                Some(e) => Err(e.clone()),
                None => Ok(st.menu.clone()),
            }
        }

        fn menu_press(&mut self, _pid: u32, path: &[usize]) -> Result<(), Fail> {
            let mut st = self.st();
            st.presses.push(path.to_vec());
            match &st.press_result {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        fn take_ax_tree_dirty(&mut self) -> bool {
            std::mem::take(&mut self.st().ax_tree_dirty)
        }

        fn ax_tree(&mut self, _w: &Window) -> Result<(Vec<Node>, bool), Fail> {
            let mut st = self.st();
            st.ax_tree_reads += 1;
            match &st.ax_tree_result {
                Some(e) => Err(e.clone()),
                None => Ok((st.ax_tree.clone(), false)),
            }
        }

        fn ax_tree_press(&mut self, w: &Window, path: &[usize], role: &str) -> Result<(), Fail> {
            let mut st = self.st();
            st.ax_presses.push((w.id, path.to_vec(), role.into()));
            match &st.ax_action_result {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        fn ax_tree_set(
            &mut self,
            w: &Window,
            path: &[usize],
            role: &str,
            value: &str,
        ) -> Result<(), Fail> {
            let mut st = self.st();
            st.ax_sets
                .push((w.id, path.to_vec(), role.into(), value.into()));
            match &st.ax_action_result {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }
}
