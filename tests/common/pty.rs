//! A real terminal for a test, as a tmux pane: a pty whose master the test
//! holds, a command that runs as the session leader on its slave, and a
//! close of the master that hangs the slave up.
//!
//! The kernel hangs up the slave only when the last descriptor of the
//! master closes. `openpty(3)` makes descriptors without close-on-exec, so
//! every child that a test spawned after it (paddock, its sandbox, or the
//! child of another test in the same binary) kept a copy of the master, and
//! the close of the test did nothing. Here both ends have `O_CLOEXEC` from
//! the start, and the test keeps no copy of the slave after the spawn.

use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// `ptsname(3)` returns a static buffer: one caller at a time.
static PTSNAME: Mutex<()> = Mutex::new(());

/// A pty pair, the master and the slave, both close-on-exec.
pub fn pty() -> (OwnedFd, OwnedFd) {
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: open(2) of a constant path.
    let m = unsafe { libc::open(c"/dev/ptmx".as_ptr(), flags) };
    assert!(m >= 0, "/dev/ptmx: {}", std::io::Error::last_os_error());
    // SAFETY: the descriptor is new and ours.
    let master = unsafe { OwnedFd::from_raw_fd(m) };
    // SAFETY: grantpt and unlockpt on our master.
    let r = unsafe { libc::grantpt(m) | libc::unlockpt(m) };
    assert_eq!(r, 0, "grantpt: {}", std::io::Error::last_os_error());
    let name = {
        let _one = PTSNAME.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: ptsname on our master. The lock keeps its buffer ours
        // until the copy.
        let p = unsafe { libc::ptsname(m) };
        assert!(!p.is_null(), "ptsname: {}", std::io::Error::last_os_error());
        // SAFETY: a NUL-terminated string from ptsname.
        unsafe { std::ffi::CStr::from_ptr(p) }.to_owned()
    };
    // SAFETY: open(2) of the slave name.
    let s = unsafe { libc::open(name.as_ptr(), flags) };
    assert!(s >= 0, "{name:?}: {}", std::io::Error::last_os_error());
    // SAFETY: the descriptor is new and ours.
    (master, unsafe { OwnedFd::from_raw_fd(s) })
}

/// The terminal side of a pty: a thread reads what the session writes
/// until [`Terminal::close`].
pub struct Terminal {
    close: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Vec<u8>>,
}

impl Terminal {
    fn start(master: OwnedFd) -> Self {
        let close = Arc::new(AtomicBool::new(false));
        let stop = close.clone();
        let thread = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while !stop.load(Ordering::SeqCst) {
                let mut p = libc::pollfd {
                    fd: master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: poll on one descriptor that this thread owns.
                if unsafe { libc::poll(&mut p, 1, 100) } > 0 {
                    // SAFETY: read into our buffer from our descriptor.
                    let n = unsafe { libc::read(p.fd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n <= 0 {
                        break;
                    }
                    seen.extend_from_slice(&buf[..n as usize]);
                }
            }
            drop(master);
            seen
        });
        Terminal { close, thread }
    }

    /// Close the master, the last descriptor of it: the slave hangs up, as
    /// when tmux closes a pane. The result is what the session wrote.
    pub fn close(self) -> Vec<u8> {
        self.close.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

/// Run `cmd` as the session leader of a new pty, with the pty as its
/// controlling terminal and as its stdin, stdout and stderr, as in a tmux
/// pane. The test keeps only the master, in the returned terminal.
pub fn spawn_on_pty(mut cmd: Command) -> (Child, Terminal) {
    use std::os::unix::process::CommandExt as _;
    let (master, slave) = pty();
    cmd.stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: setsid and ioctl are async-signal-safe: the pty becomes the
    // controlling terminal of a new session.
    unsafe {
        cmd.pre_exec(|| {
            // A terminal starts its session with the default action of
            // SIGHUP. A test runner that ignores SIGHUP (nohup, a queue in
            // the background) would pass the ignore on through exec.
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            #[allow(clippy::useless_conversion)]
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().unwrap();
    // The command holds the slave descriptors: they go with it.
    drop(cmd);
    (child, Terminal::start(master))
}
