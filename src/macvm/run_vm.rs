//! The framework and process side of the shim `paddock-vm app`, macOS
//! only: the boot and the resume of the VM, the VM delegate, the control socket guest, the signal pipe, the
//! token from the parent, the host facts, `clonefile` and the backups of
//! the data disk. `run.rs` holds the session and its loop.

use super::events::Events;
use super::proto;
use super::session::{self, Profile};
use super::text;
use super::vz_mac_agent::socket_device;
use crate::vm::bridge::Guest;
use crate::vm::vz::{describe, dup_fd, error_text};
use anyhow::{anyhow, bail, Context, Result};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_foundation::NSError;
use objc2_virtualization::*;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

/// How long a `connect` on the control socket waits for the guest.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a `pause` or `resume` line waits for the framework.
const FREEZE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a `window` or `bench windows` line waits for the loop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// What the VM and the process report to the session loop.
pub enum VmEvent {
    GuestStopped,
    Failed(String),
    /// SIGTERM or SIGINT.
    Signal,
    /// The `stop` line on the control socket.
    ControlStop,
    /// A `window` line on the control socket, with where its answer goes.
    Window(String, Sender<Result<String, String>>),
    /// A `bench` line on the control socket.
    Bench(Bench),
    /// A `pause` (true) or `resume` line on the control socket.
    Freeze(bool, Sender<Result<String, String>>),
    /// A `suspend REASON` line on the control socket (`run_suspend.rs`).
    Suspend(String, Sender<Result<String, String>>),
}

/// The bench doors of the shim, for the Mac checks.
pub enum Bench {
    /// A `windows` event as if the guest sent it.
    Windows(proto::WindowsEvent, Sender<Result<String, String>>),
    /// Send `ax.value` lines to this channel until it closes; stamped
    /// lines start with the epoch milliseconds (`bench axt`).
    AxWatch(Sender<String>, bool),
    AxStop,
    /// One request to the agent (`launch`, `agent-restart`, `op`).
    Request(proto::Request, Sender<Result<String, String>>),
    /// `agent.update` with the agent of this paddock (`bench update`).
    Update(Sender<Result<String, String>>),
}

/// The VM, shared between threads. Only closures on its queue touch it.
pub struct Vm(pub Retained<VZVirtualMachine>);
// SAFETY: every use of the inner object runs on the main queue.
unsafe impl Send for Vm {}
unsafe impl Sync for Vm {}

/// An object that moves into a closure on the queue of the VM.
pub struct OnQueue<T>(pub T);
// SAFETY: the object is used only inside that closure.
unsafe impl<T> Send for OnQueue<T> {}

impl<T> OnQueue<T> {
    pub fn get(&self) -> &T {
        &self.0
    }
}

/// Write to stderr (the shim log) and never panic on a closed stream.
pub fn say(text: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{}", text::helper_line(text));
}

/// The control socket side of the VM, for `paddock-host` and
/// `paddock app stop`.
pub struct VmGuest {
    pub vm: Arc<Vm>,
    pub queue: &'static DispatchQueue,
    pub tx: Sender<VmEvent>,
}

impl Guest for VmGuest {
    fn connect(&self, port: u32) -> Result<OwnedFd> {
        let (rtx, rrx) = mpsc::channel::<Result<OwnedFd, String>>();
        let vm = self.vm.clone();
        self.queue.exec_async(move || {
            let Some(dev) = socket_device(&vm.0) else {
                let _ = rtx.send(Err(text::no_socket_device()));
                return;
            };
            let done = RcBlock::new(
                move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
                    let r = match unsafe { conn.as_ref() } {
                        Some(c) => dup_fd(unsafe { c.fileDescriptor() }).map_err(|e| e.to_string()),
                        None => Err(error_text(err).err().unwrap_or_else(text::no_connection)),
                    };
                    let _ = rtx.send(r);
                },
            );
            unsafe { dev.connectToPort_completionHandler(port, &done) };
        });
        match rrx.recv_timeout(CONNECT_TIMEOUT) {
            Ok(r) => r.map_err(|e| anyhow!(text::guest_port_failed(port, &e))),
            Err(_) => bail!(text::guest_port_timeout(port)),
        }
    }

    fn stop(&self) {
        let _ = self.tx.send(VmEvent::ControlStop);
    }

    fn suspend(&self, reason: &str) -> Result<()> {
        let (rtx, rrx) = mpsc::channel();
        let _ = self.tx.send(VmEvent::Suspend(reason.to_string(), rtx));
        match rrx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => bail!(e),
            Err(_) => bail!(text::control_request_timeout()),
        }
    }

    fn pause(&self) -> Result<()> {
        self.freeze(true)
    }

    fn resume(&self) -> Result<()> {
        self.freeze(false)
    }

    fn window(&self, args: &str) -> Result<String> {
        let (rtx, rrx) = mpsc::channel();
        let _ = self.tx.send(VmEvent::Window(args.to_string(), rtx));
        match rrx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(Ok(line)) => Ok(line),
            Ok(Err(e)) => bail!(e),
            Err(_) => bail!(text::control_request_timeout()),
        }
    }

    /// `bench windows JSON`: the event goes through the fold as if the
    /// guest sent it. `bench ax SECONDS`: `ax.watch` on, one line per
    /// `ax.value` event (`WINDOW TEXT`) for that long, then off. `bench
    /// launch PATH`, `bench agent-restart`, `bench lock` and `bench
    /// keychain` send that request.
    fn bench(&self, args: &str, out: &mut dyn std::io::Write) -> Result<()> {
        if let Some(json) = args.strip_prefix("windows ") {
            let ev: proto::WindowsEvent =
                serde_json::from_str(json).map_err(|e| anyhow!(text::bench_event_invalid(&e)))?;
            let (rtx, rrx) = mpsc::channel();
            let _ = self.tx.send(VmEvent::Bench(Bench::Windows(ev, rtx)));
            return match rrx.recv_timeout(REQUEST_TIMEOUT) {
                Ok(Ok(line)) => {
                    writeln!(out, "ok {line}")?; // not user text
                    Ok(())
                }
                Ok(Err(e)) => bail!(e),
                Err(_) => bail!(text::control_request_timeout()),
            };
        }
        if let Some(json) = args.strip_prefix("op ") {
            // `bench op JSON`: one request of `proto::Request`, the bench
            // ops among them, sent as it is.
            let req: proto::Request =
                serde_json::from_str(json).map_err(|e| anyhow!(text::bench_event_invalid(&e)))?;
            return self.bench_request(req, out);
        }
        let stamped = args.starts_with("axt ");
        if let Some(secs) = args
            .strip_prefix("ax ")
            .or_else(|| args.strip_prefix("axt "))
        {
            let secs: u64 = secs
                .parse()
                .map_err(|_| anyhow!(text::control_request_unknown(args)))?;
            let (ltx, lrx) = mpsc::channel::<String>();
            let _ = self.tx.send(VmEvent::Bench(Bench::AxWatch(ltx, stamped)));
            writeln!(out, "ok")?;
            let end = std::time::Instant::now() + Duration::from_secs(secs);
            loop {
                let now = std::time::Instant::now();
                if now >= end {
                    break;
                }
                match lrx.recv_timeout(end - now) {
                    Ok(line) => {
                        if writeln!(out, "{line}").is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let _ = self.tx.send(VmEvent::Bench(Bench::AxStop));
            return Ok(());
        }
        if let Some(path) = args.strip_prefix("launch ") {
            // A check script can start an app of the guest this way, by its
            // guest path.
            let req = proto::Request::AppLaunch {
                path: std::path::PathBuf::from(path),
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
            };
            return self.bench_request(req, out);
        }
        if args == "agent-restart" {
            return self.bench_request(proto::Request::AgentRestart, out);
        }
        // `bench lock` locks the guest screen (the session unlocks it);
        // `bench keychain` puts the state of the login keychain into the
        // event log as a `keychain` event. Both need a `--bench` session.
        if args == "lock" {
            return self.bench_request(proto::Request::BenchLock, out);
        }
        if args == "keychain" {
            return self.bench_request(proto::Request::BenchKeychain, out);
        }
        if args == "agent-update" {
            let (rtx, rrx) = mpsc::channel();
            let _ = self.tx.send(VmEvent::Bench(Bench::Update(rtx)));
            return match rrx.recv_timeout(REQUEST_TIMEOUT) {
                Ok(Ok(line)) => {
                    writeln!(out, "ok {line}")?; // not user text
                    Ok(())
                }
                Ok(Err(e)) => bail!(e),
                Err(_) => bail!(text::control_request_timeout()),
            };
        }
        bail!(text::control_request_unknown(args))
    }
}

impl VmGuest {
    /// A `pause` or `resume` line: the loop calls the framework, and the
    /// answer comes once it is done.
    fn freeze(&self, pause: bool) -> Result<()> {
        let (rtx, rrx) = mpsc::channel();
        let _ = self.tx.send(VmEvent::Freeze(pause, rtx));
        match rrx.recv_timeout(FREEZE_TIMEOUT) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => bail!(e),
            Err(_) => bail!(text::control_request_timeout()),
        }
    }

    /// One request to the agent for a bench line, answered with `ok`
    /// once the loop sent it.
    fn bench_request(&self, req: proto::Request, out: &mut dyn std::io::Write) -> Result<()> {
        let (rtx, rrx) = mpsc::channel();
        let _ = self.tx.send(VmEvent::Bench(Bench::Request(req, rtx)));
        match rrx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(Ok(line)) => {
                writeln!(out, "ok {line}")?; // not user text
                Ok(())
            }
            Ok(Err(e)) => bail!(e),
            Err(_) => bail!(text::control_request_timeout()),
        }
    }
}

/// The token line and the password line from the pipe of the parent.
pub fn read_token(fd: i32) -> Result<(String, Option<String>)> {
    // SAFETY: the parent passed this fd for the token; it is owned here.
    let f = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut text_lines = String::new();
    f.take(super::vz_mac_agent::TOKEN_LINE_MAX as u64 * 2)
        .read_to_string(&mut text_lines)
        .with_context(text::token_unreadable)?;
    let mut lines = text_lines.lines();
    let token = lines.next().unwrap_or("").to_string();
    if !crate::vm::proto::valid_token(&token) {
        bail!(text::token_invalid());
    }
    let password = lines.next().filter(|p| !p.is_empty()).map(str::to_string);
    Ok((token, password))
}

/// `sw_vers -buildVersion`, or `unknown`.
pub fn host_build() -> String {
    std::process::Command::new("/usr/bin/sw_vers")
        .arg("-buildVersion")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// The version of the Virtualization framework: its bundle version, or
/// the host build when that cannot be read.
pub fn framework_version() -> String {
    let plist = "/System/Library/Frameworks/Virtualization.framework/Resources/Info.plist";
    std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleVersion", "raw", "-o", "-", plist])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(host_build)
}

/// An APFS clone of `src` as the new file `dst` (`clonefile(2)`).
pub fn clonefile(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let s = CString::new(src.as_os_str().as_bytes())?;
    let d = CString::new(dst.as_os_str().as_bytes())?;
    // CLONE_NOFOLLOW | CLONE_NOOWNERCOPY of <sys/clonefile.h>.
    // SAFETY: two NUL-terminated paths.
    let r = unsafe {
        libc::clonefileat(
            libc::AT_FDCWD,
            s.as_ptr(),
            libc::AT_FDCWD,
            d.as_ptr(),
            0x0001 | 0x0002,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // A clone keeps the file flags and the mode of its source, and the
    // sealed golden is uchg and 0444: the profile's copy must be writable
    // by its owner only. The owner may clear uchg.
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::chflags(d.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o600))
}

/// A clone backup of the data disk into `backups/<reason>-<time>.img`, mode
/// 0600, skipped with one line when the volume lacks the used size of the
/// disk. The retention and `paddock app backup` are in `backup.rs`.
pub fn backup_data(profile: &Profile, reason: &str, events: &Events) -> Result<()> {
    let data = profile.data();
    let Ok(meta) = std::fs::metadata(&data) else {
        return Ok(());
    };
    use std::os::unix::fs::MetadataExt;
    let used = meta.blocks() * 512;
    let free = session::free_bytes(&profile.dir);
    if free < used {
        let line = text::backup_skipped_used(used, free);
        say(&line);
        events.record("backup", &line, &[]);
        return Ok(());
    }
    std::fs::create_dir_all(profile.backups())?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = format!(
        "{}-{}.img", // not user text
        session::safe_name(reason),
        crate::waiting::format_rfc3339(secs).replace(':', "-")
    );
    let target = profile.backups().join(name);
    let partial = session::partial(&target);
    let _ = std::fs::remove_file(&partial);
    clonefile(&data, &partial).with_context(|| text::cannot_clone(&data, &partial))?;
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600))?;
    session::commit(&partial)?;
    let line = text::backup_made(&target);
    say(&line);
    events.record("backup", &line, &[("bytes", used as f64)]);
    Ok(())
}

static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    let fd = SIGNAL_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        // Only write(2) here: it is safe in a signal handler.
        unsafe { libc::write(fd, b"s".as_ptr().cast(), 1) };
    }
}

/// SIGTERM and SIGINT become `Signal` events.
pub fn watch_signals(tx: Sender<VmEvent>) -> Result<()> {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(text::signal_pipe_failed);
    }
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    SIGNAL_FD.store(fds[1], Ordering::Relaxed);
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for sig in [libc::SIGTERM, libc::SIGINT] {
        unsafe { libc::signal(sig, handler) };
    }
    std::thread::spawn(move || {
        let mut read = read;
        let mut byte = [0u8; 1];
        while matches!(read.read(&mut byte), Ok(1)) {
            if tx.send(VmEvent::Signal).is_err() {
                break;
            }
        }
    });
    Ok(())
}

/// The end of the terminal must never end the session.
pub fn ignore_hup() {
    unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
}

impl super::run::Session<'_> {
    /// `start`, or `restore` then `resume`.
    pub(super) fn boot(&mut self) {
        self.begun = std::time::Instant::now();
        let vm = self.vm.clone();
        let tx = self.tx.clone();
        if self.start == super::session::Start::Warm {
            let state = match crate::vm::vz::url(&self.plan.profile.state()) {
                Ok(u) => u,
                Err(e) => {
                    let _ = tx.send(super::run::Event::Restored(Err(e.to_string())));
                    return;
                }
            };
            let state = OnQueue(state);
            self.queue.exec_async(move || {
                let done = RcBlock::new(move |err: *mut NSError| {
                    let _ = tx.send(super::run::Event::Restored(error_text(err)));
                });
                unsafe {
                    vm.0.restoreMachineStateFromURL_completionHandler(state.get(), &done)
                };
            });
        } else {
            self.queue.exec_async(move || {
                let done = RcBlock::new(move |err: *mut NSError| {
                    let _ = tx.send(super::run::Event::Started(error_text(err)));
                });
                unsafe { vm.0.startWithCompletionHandler(&done) };
            });
        }
    }

    pub(super) fn resume(&self, warm_start: bool) {
        let vm = self.vm.clone();
        let tx = self.tx.clone();
        self.queue.exec_async(move || {
            let done = RcBlock::new(move |err: *mut NSError| {
                let r = error_text(err);
                let _ = tx.send(if warm_start {
                    super::run::Event::Started(r)
                } else {
                    super::run::Event::Resumed(r)
                });
            });
            unsafe { vm.0.resumeWithCompletionHandler(&done) };
        });
    }
}

pub struct VmIvars {
    tx: Sender<VmEvent>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AllocAnyThread]
    #[name = "PaddockAppVmDelegate"]
    #[ivars = VmIvars]
    pub struct VmDelegate;

    unsafe impl NSObjectProtocol for VmDelegate {}

    unsafe impl VZVirtualMachineDelegate for VmDelegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop(&self, _vm: &VZVirtualMachine) {
            let _ = self.ivars().tx.send(VmEvent::GuestStopped);
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn did_stop_with_error(&self, _vm: &VZVirtualMachine, error: &NSError) {
            let _ = self.ivars().tx.send(VmEvent::Failed(describe(error)));
        }
    }
);

impl VmDelegate {
    pub fn new(tx: Sender<VmEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(VmIvars { tx });
        unsafe { msg_send![super(this), init] }
    }
}
