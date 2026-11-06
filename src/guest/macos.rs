//! The macOS implementation of [`Platform`]: AppKit, Accessibility, the
//! window server list, the guest commands and the vsock transfers.

use super::apps::{crash_reports_dir, find_crash_report};
use super::ax::{self, Ax};
use super::platform::{Fail, HealthInfo, HelloInfo, Platform, Waker, WindowOp};
use super::windows::{AxInfo, ProcTable, RawWindow};
use super::{
    apps_macos, clipboard, data, downloads, drop, files, input, update, vsock, HOST_CID,
    SUPPORT_DIR,
};
use crate::macvm::axmirror::Node;
use crate::macvm::clipboard::Item;
use crate::macvm::pasteboard;
use crate::macvm::proto::{Display, ErrorCode, FileKind, Frame, MenuItem, Window};
use crate::macvm::text;
use crate::vm::ports::GUEST_FILES;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{NSRunningApplication, NSScreen, NSWorkspace};
use objc2_core_foundation::{kCFRunLoopDefaultMode, CFArray, CFRetained, CFRunLoop};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{ns_string, NSArray, NSDictionary, NSString};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The guest, from the agent thread, which is the main thread.
pub struct Mac {
    ax: Ax,
    mtm: MainThreadMarker,
    main_loop: CFRetained<CFRunLoop>,
    /// The bundle id of each pid, read once.
    bundles: HashMap<u32, Option<String>>,
    /// The app pid of the session, for the bench's `ax.value`.
    app_pid: Option<u32>,
    data_check: Option<String>,
    unmountable: Option<String>,
    /// Opens a `guest-files` connection: vsock, or a test socket.
    files_dial: Box<dyn FnMut() -> std::io::Result<std::os::unix::net::UnixStream>>,
    /// The agent folder: [`SUPPORT_DIR`], or `PADDOCK_GUEST_SUPPORT` for
    /// the checks on a Mac.
    support: PathBuf,
}

impl Mac {
    /// The agent of the real guest: vsock transfers, the data disk checks.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let mut mac = Self::with_files(mtm, || {
            vsock::connect(HOST_CID, GUEST_FILES, Duration::from_secs(5))
        });
        mac.data_check = data::start_check();
        mac.unmountable = data::unmountable_check();
        mac
    }

    /// A platform whose `guest-files` transfers go through `files_dial`,
    /// without the data disk checks: the local check on a Mac.
    pub fn with_files(
        mtm: MainThreadMarker,
        files_dial: impl FnMut() -> std::io::Result<std::os::unix::net::UnixStream> + 'static,
    ) -> Self {
        let support = std::env::var_os("PADDOCK_GUEST_SUPPORT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(SUPPORT_DIR));
        Mac {
            ax: Ax::new(),
            mtm,
            main_loop: CFRunLoop::main().expect("the main run loop"),
            bundles: HashMap::new(),
            app_pid: None,
            data_check: None,
            unmountable: None,
            files_dial: Box::new(files_dial),
            support,
        }
    }

    fn bundle(&mut self, pid: u32) -> Option<String> {
        self.bundles
            .entry(pid)
            .or_insert_with(|| {
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
                    .and_then(|a| a.bundleIdentifier())
                    .map(|s| s.to_string())
            })
            .clone()
    }
}

/// The output of `sw_vers -KEY`, or an empty text.
fn sw_vers(key: &str) -> String {
    Command::new("/usr/bin/sw_vers")
        .arg(key)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Run a short AppleScript without waiting for it.
fn osascript(script: &str) -> Result<(), Fail> {
    Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(crate::reap::in_background)
        .map_err(|e| {
            (
                ErrorCode::Io,
                text::cannot_run_command("osascript", &e.to_string()),
            )
        })
}

fn uptime_secs() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid out struct.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec.max(0) as u64
}

fn load_1m() -> f64 {
    let mut avg = [0f64; 3];
    // SAFETY: a buffer of three doubles.
    let n = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
    if n >= 1 {
        avg[0]
    } else {
        0.0
    }
}

fn mem_free() -> u64 {
    let mut pages: u32 = 0;
    let mut len = std::mem::size_of::<u32>();
    let name = c"vm.page_free_count";
    // SAFETY: the out buffer and its length match.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut pages as *mut u32).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return 0;
    }
    // SAFETY: a plain query.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
    pages as u64 * page
}

impl Platform for Mac {
    fn monotonic_ms(&self) -> u64 {
        super::platform::monotonic_ms()
    }

    fn wait(&mut self, d: Duration) {
        // SAFETY: a constant of Core Foundation.
        let mode = unsafe { kCFRunLoopDefaultMode };
        // Return after the first handled source: an Accessibility
        // notification makes the agent look at once.
        CFRunLoop::run_in_mode(mode, d.as_secs_f64(), true);
    }

    fn waker(&self) -> Waker {
        let rl = CFRetained::as_ptr(&self.main_loop).as_ptr() as usize;
        Arc::new(move || {
            // SAFETY: the main run loop lives for the process; CFRunLoopStop
            // is safe from any thread.
            let rl = unsafe { &*(rl as *const CFRunLoop) };
            rl.stop();
        })
    }

    fn take_dirty(&mut self) -> bool {
        ax::take_dirty()
    }

    fn hello(&mut self) -> HelloInfo {
        let screen = NSScreen::mainScreen(self.mtm);
        let (scale, display) = match screen {
            Some(s) => {
                let f = s.frame();
                (
                    s.backingScaleFactor(),
                    Display {
                        w: f.size.width as u32,
                        h: f.size.height as u32,
                    },
                )
            }
            None => (1.0, Display { w: 0, h: 0 }),
        };
        HelloInfo {
            agent: crate::macvm::version::paddock_version().to_string(),
            macos: sw_vers("-productVersion"),
            build: sw_vers("-buildVersion"),
            scale,
            display,
            can_update: update::can_update(&self.support),
        }
    }

    fn health(&mut self) -> HealthInfo {
        HealthInfo {
            uptime: uptime_secs(),
            load: load_1m(),
            mem_free: mem_free(),
            data_disk_free: data::free_bytes(),
            input_source: input::current(),
            accessibility: ax::trusted_now(),
        }
    }

    fn agent_step(&mut self, argv: &[String]) -> (bool, String) {
        // No step of the list is the agent's own now (the desktop
        // picture went, `configure::AGENT_OP` says why).
        (false, text::agent_step_unknown(argv))
    }

    fn raw_windows(&mut self) -> Vec<RawWindow> {
        let opts =
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
        let Some(list) = CGWindowListCopyWindowInfo(opts, 0) else {
            return Vec::new();
        };
        // SAFETY: CFArray and NSArray are toll-free bridged. The list
        // holds dictionaries with the documented keys.
        let arr: &NSArray<NSDictionary<NSString, AnyObject>> =
            unsafe { &*(&*list as *const CFArray).cast() };
        let num = |d: &NSDictionary<NSString, AnyObject>, k: &NSString| -> Option<f64> {
            let v = d.objectForKey(k)?;
            Some(unsafe { msg_send![&*v, doubleValue] })
        };
        let text = |d: &NSDictionary<NSString, AnyObject>, k: &NSString| -> Option<String> {
            let v = d.objectForKey(k)?;
            // SAFETY: the owner name is an NSString.
            let s: &NSString = unsafe { &*(&*v as *const AnyObject).cast() };
            Some(s.to_string())
        };
        let mut out = Vec::new();
        for d in arr.iter() {
            let (Some(id), Some(pid)) = (
                num(&d, ns_string!("kCGWindowNumber")),
                num(&d, ns_string!("kCGWindowOwnerPID")),
            ) else {
                continue;
            };
            let Some(b) = d.objectForKey(ns_string!("kCGWindowBounds")) else {
                continue;
            };
            let field = |k: &NSString| -> f64 {
                let v: Option<Retained<AnyObject>> = unsafe { msg_send![&*b, objectForKey: k] };
                v.map(|v| unsafe { msg_send![&*v, doubleValue] })
                    .unwrap_or(0.0)
            };
            let pid = pid as u32;
            out.push(RawWindow {
                id: id as u32,
                pid,
                owner: text(&d, ns_string!("kCGWindowOwnerName")).unwrap_or_default(),
                bundle: self.bundle(pid),
                layer: num(&d, ns_string!("kCGWindowLayer")).unwrap_or(0.0) as i64,
                bounds: Frame::new(
                    field(ns_string!("X")),
                    field(ns_string!("Y")),
                    field(ns_string!("Width")),
                    field(ns_string!("Height")),
                ),
                alpha: num(&d, ns_string!("kCGWindowAlpha")).unwrap_or(1.0),
            });
        }
        out
    }

    fn procs(&mut self) -> ProcTable {
        let table = crate::mark::procs::proc_table();
        self.bundles.retain(|pid, _| table.contains_key(pid));
        table
    }

    fn front_pid(&mut self) -> Option<u32> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier() as u32)
    }

    fn ax_info(&mut self, w: &RawWindow) -> AxInfo {
        let front = self.front_pid();
        self.ax.info(w, front)
    }

    fn observe(&mut self, pids: &BTreeSet<u32>) {
        self.ax.observe(pids);
    }

    fn window_radius(&mut self, w: &RawWindow) -> Option<f64> {
        super::capture::radius(w)
    }

    fn window_op(&mut self, w: &Window, op: WindowOp) -> Result<(), Fail> {
        self.ax.op(w, op)
    }

    fn launch(
        &mut self,
        path: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<u32, Fail> {
        let pid = apps_macos::launch(path, args, env)?;
        self.app_pid = Some(pid);
        Ok(pid)
    }

    fn quit(&mut self, pid: u32, force: bool) -> Result<(), Fail> {
        apps_macos::quit(pid, force)
    }

    fn crash_report(&mut self, pid: u32, since: SystemTime) -> Option<PathBuf> {
        find_crash_report(&crash_reports_dir(), pid, since)
    }

    fn data_prepare(&mut self, password: &str, data_id: &str) -> super::platform::PrepareJob {
        let (password, data_id) = (password.to_string(), data_id.to_string());
        Box::new(move |watch| data::prepare(&password, &data_id, watch))
    }

    fn input_set(
        &mut self,
        sources: &[String],
        current: &str,
        reduce_motion: bool,
        increase_contrast: bool,
    ) -> Result<Vec<String>, Fail> {
        input::set(sources, current, reduce_motion, increase_contrast)
    }

    fn update(&mut self, path: &Path, sha256: &str) -> Result<(), Fail> {
        update::apply(&self.support, path, sha256, update::copy_executable).map(|_| ())
    }

    fn send_log(&mut self, token: &str) -> Result<(), Fail> {
        let path = std::env::var_os("PADDOCK_GUEST_LOG")
            .map(PathBuf::from)
            .unwrap_or_else(super::log::default_path);
        files::send(token, FileKind::Log, &path, &mut self.files_dial)
    }

    fn disk_low_alert(&mut self) {
        let _ = osascript(
            "display alert \"The disk of your Mac is almost full\" message \"paddock cannot save this session. Free some space on your Mac, then quit the app and start it again.\" as warning",
        );
    }

    fn shutdown(&mut self, password: &str) -> Result<(), Fail> {
        data::end_session();
        super::shutdown::run(password, super::configure::run_plan).map_err(|t| (ErrorCode::Io, t))
    }

    fn ax_value(&mut self) -> Option<(u32, String)> {
        let pid = self.app_pid?;
        self.ax.value(pid)
    }

    fn data_check(&mut self) -> Option<String> {
        self.data_check.take()
    }

    fn data_unmountable(&mut self) -> Option<String> {
        self.unmountable.take()
    }

    fn dialog_dismiss(&mut self, w: &RawWindow) -> super::dialogs::Seen {
        super::dialog_ax::dismiss(&mut self.ax, w)
    }

    fn screen_locked(&mut self) -> bool {
        super::screen::locked()
    }

    fn idle_secs(&mut self) -> Option<u64> {
        super::screen::idle_secs()
    }

    fn lock_screen(&mut self) -> Result<(), Fail> {
        super::screen::lock_now()
    }

    fn keychain_unlock(&mut self, password: &str) -> Result<String, Fail> {
        super::keychain::unlock(password)
    }

    fn keychain_info(&mut self) -> Result<String, Fail> {
        super::keychain::info()
    }

    fn state_file(&self) -> Option<PathBuf> {
        // The temporary folder of the login: gone at a reboot, which ends
        // the app too.
        Some(std::env::temp_dir().join("paddock-guest-app"))
    }

    fn clipboard_count(&mut self) -> u64 {
        clipboard::count()
    }

    fn clipboard_read(&mut self) -> Vec<Item> {
        clipboard::read()
    }

    fn tiff_to_png(&mut self, tiff: &[u8]) -> Option<Vec<u8>> {
        pasteboard::tiff_to_png(tiff)
    }

    fn clipboard_write(&mut self, items: &[Item]) -> Result<u64, Fail> {
        clipboard::write(items)
    }

    fn send_clipboard(&mut self, token: &str, count: u64, json: &[u8]) -> Result<String, Fail> {
        clipboard::send(token, count, json, &mut self.files_dial)
    }

    fn open_with_app(&mut self, paths: &[PathBuf], pid: Option<u32>) -> Result<u64, Fail> {
        drop::open(paths, pid)
    }

    fn take_menu_dirty(&mut self) -> bool {
        ax::take_menu_dirty()
    }

    fn menu_read(&mut self, pid: u32) -> Result<Vec<MenuItem>, Fail> {
        self.ax.menu(pid)
    }

    fn menu_press(&mut self, pid: u32, path: &[usize]) -> Result<(), Fail> {
        self.ax.menu_press(pid, path)
    }

    fn take_ax_tree_dirty(&mut self) -> bool {
        ax::take_tree_dirty()
    }

    fn ax_tree(&mut self, w: &Window) -> Result<(Vec<Node>, bool), Fail> {
        self.ax.tree(w)
    }

    fn ax_tree_press(&mut self, w: &Window, path: &[usize], role: &str) -> Result<(), Fail> {
        self.ax.tree_press(w, path, role)
    }

    fn ax_tree_set(
        &mut self,
        w: &Window,
        path: &[usize],
        role: &str,
        value: &str,
    ) -> Result<(), Fail> {
        self.ax.tree_set(w, path, role, value)
    }

    fn downloads_link(&mut self) -> bool {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return true;
        };
        match downloads::ensure_link(&home, &downloads::share_path()) {
            Ok(downloads::Outcome::Linked) => true,
            Ok(downloads::Outcome::Relinked) => {
                log::info!(
                    "the Downloads folder of the guest is a link to the downloads share again"
                );
                true
            }
            Ok(downloads::Outcome::Waiting) => false,
            Err(e) => {
                log::warn!("the Downloads folder of the guest could not be linked: {e}");
                true
            }
        }
    }
}

/// True when the agent has the Accessibility permission, for the log.
pub fn accessibility_granted(mac: &Mac) -> bool {
    mac.ax.trusted()
}
