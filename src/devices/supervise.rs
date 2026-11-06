//! `paddock device-serve`: the supervisor that puts a device program
//! behind a Unix socket that speaks HTTP/1.1 (program.rs is the client).
//!
//! For `shape = "exec"` it forks the fixed command per `POST /v1/call`,
//! with the request on stdin and the response from stdout (exec.rs). For
//! `shape = "serve"` it starts the program once, waits for its health and
//! proxies each request to it (serve.rs). `--connect PORT:SOCKET` listens
//! on `127.0.0.1:PORT` and copies each connection to a socket that the
//! broker serves (relay.rs). The HTTP server is http.rs.
//!
//! A device with run = "sandbox" runs this inside a device sandbox, one with
//! run = "host" on the host. Here it runs with no isolation, as the tests do.
//! It exits when its parent dies (`PR_SET_PDEATHSIG` on Linux, a parent pid
//! check every second everywhere) and on SIGTERM, after it killed its
//! children.
//!
//! The log has one line per request (the id, the caller, the status, the
//! bytes, the duration) and the program's stderr lines with the request id
//! as a prefix. Never a request or a response. The file is capped at 10
//! MiB and rotated once (`.1`).

use super::config::Shape;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub mod exec;
pub mod http;
pub mod relay;
pub mod serve;

pub use exec::Envelope;

/// The size at which the log file rotates.
pub const LOG_CAP: u64 = 10 << 20;
/// How often the supervisor checks its parent and the stop flag.
const TICK: Duration = Duration::from_millis(200);

/// What `paddock device-serve` got on its command line.
#[derive(Debug, Clone)]
pub struct Options {
    /// The socket to serve on. Its folder must exist.
    pub socket: PathBuf,
    pub shape: Shape,
    /// The program and its fixed arguments.
    pub command: Vec<String>,
    /// Seconds per request.
    pub timeout: u64,
    pub max_request: usize,
    pub max_response: usize,
    /// Requests at work at the same time.
    pub concurrency: u32,
    /// `exec`: the response is the raw stdout as a chunked body, with the
    /// exit status in a trailer.
    pub stream: bool,
    /// `serve`: seconds to wait for the program's health.
    pub ready: u64,
    /// `--connect PORT:SOCKET` entries.
    pub connect: Vec<(u16, PathBuf)>,
    /// The log file. None: stderr.
    pub log: Option<PathBuf>,
    /// Names of variables of the supervisor's own environment that the
    /// program gets on top of the base set (`--env-pass`).
    pub env_pass: Vec<String>,
    /// The program runs on the host as the user (host.rs): it gets the
    /// whole environment of the supervisor, not the base set, and the
    /// messages name the host.
    pub host: bool,
    /// The working folder of the program. None: a fresh empty folder per
    /// `exec` request, the supervisor's own folder for `serve`.
    pub cwd: Option<PathBuf>,
    /// Fixed variables of the program, on top of the base set and
    /// `env_pass`. The thread launcher of the tests sets the `env` of the
    /// profile here; the bwrap and Seatbelt launchers set them on the
    /// sandbox instead.
    pub env: Vec<(String, String)>,
}

impl Options {
    /// Defaults for `socket` and `command`; the default limits.
    pub fn new(socket: &Path, shape: Shape, command: Vec<String>) -> Self {
        Self {
            socket: socket.to_path_buf(),
            shape,
            command,
            timeout: super::config::DEFAULT_TIMEOUT,
            max_request: super::config::DEFAULT_MAX_BODY,
            max_response: super::config::DEFAULT_MAX_BODY,
            concurrency: if shape == Shape::Serve { 4 } else { 1 },
            stream: false,
            ready: super::config::DEFAULT_READY,
            connect: Vec::new(),
            log: None,
            env_pass: Vec::new(),
            host: false,
            cwd: None,
            env: Vec::new(),
        }
    }
}

/// `--connect LOCALPORT:SOCKETPATH`.
pub fn parse_connect(s: &str) -> Result<(u16, PathBuf)> {
    let (port, path) = s
        .split_once(':')
        .with_context(|| format!("--connect {s:?}: use PORT:SOCKET_PATH"))?;
    let port: u16 = port
        .parse()
        .ok()
        .filter(|p| *p > 0)
        .with_context(|| format!("--connect {s:?}: {port:?} is not a port"))?;
    if path.is_empty() {
        anyhow::bail!("--connect {s:?}: the socket path is empty");
    }
    Ok((port, PathBuf::from(path)))
}

/// The log of the supervisor: a file with a cap and one rotation, or
/// stderr.
pub struct Log {
    inner: Mutex<LogInner>,
}

struct LogInner {
    path: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    cap: u64,
}

impl Log {
    /// A log in `path` with the default cap, or on stderr.
    pub fn open(path: Option<&Path>) -> Result<Self> {
        Self::open_with_cap(path, LOG_CAP)
    }

    pub fn open_with_cap(path: Option<&Path>, cap: u64) -> Result<Self> {
        let (file, size) = match path {
            Some(p) => {
                let f = crate::privfs::append(p)
                    .with_context(|| format!("cannot open the log {}", p.display()))?;
                let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                (Some(f), size)
            }
            None => (None, 0),
        };
        Ok(Self {
            inner: Mutex::new(LogInner {
                path: path.map(Path::to_path_buf),
                file,
                size,
                cap,
            }),
        })
    }

    /// One line, with the time first. Control characters are removed.
    pub fn line(&self, text: &str) {
        let clean: String = text
            .chars()
            .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
            .collect();
        let line = format!("{} {clean}\n", crate::state::now());
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(path) = g.path.clone() else {
            eprint!("{line}");
            return;
        };
        if g.size + line.len() as u64 > g.cap {
            g.file = None;
            let old = path.with_extension(match path.extension() {
                Some(e) => format!("{}.1", e.to_string_lossy()),
                None => "1".to_string(),
            });
            let _ = std::fs::rename(&path, &old);
            g.file = crate::privfs::append(&path).ok();
            g.size = 0;
        }
        if let Some(f) = g.file.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                g.size += line.len() as u64;
            }
        }
    }
}

/// The running supervisor: the server, the shape handler and the relays.
pub struct Supervisor {
    server: Arc<http::Server>,
    handler: Arc<dyn http::Handler>,
    relays: Vec<relay::Relay>,
    stop: Arc<AtomicBool>,
    log: Arc<Log>,
    socket: PathBuf,
}

impl Supervisor {
    /// Bind the socket, start the program of a `serve` shape and the
    /// relays. The server does not accept before `run`.
    pub fn start(opts: Options) -> Result<Self> {
        let log = Arc::new(Log::open(opts.log.as_deref())?);
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<dyn http::Handler> = match opts.shape {
            Shape::Exec => Arc::new(exec::Exec::new(&opts, log.clone())),
            Shape::Serve => Arc::new(serve::Serve::start(&opts, log.clone())?),
            Shape::HttpProxy => anyhow::bail!("device-serve serves exec or serve programs"),
        };
        let server = Arc::new(http::Server::bind(&opts, handler.clone(), log.clone())?);
        let mut relays = Vec::new();
        for (port, path) in &opts.connect {
            relays.push(relay::Relay::start(*port, path, log.clone(), stop.clone())?);
        }
        log.line(&format!(
            "start shape {} socket {} concurrency {} timeout {} s{}{}",
            opts.shape.name(),
            opts.socket.display(),
            opts.concurrency,
            opts.timeout,
            if opts.host { " on the host" } else { "" },
            opts.cwd
                .as_ref()
                .map(|c| format!(" cwd {}", c.display()))
                .unwrap_or_default()
        ));
        Ok(Self {
            server,
            handler,
            relays,
            stop,
            log,
            socket: opts.socket,
        })
    }

    /// The stop flag: set it to end `run`.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Accept connections until the stop flag is set, then kill the
    /// children and remove the socket.
    pub fn run(&self) {
        self.server.run(&self.stop);
        self.shutdown();
    }

    /// Kill the children, end the relays, remove the socket.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.handler.shutdown();
        for r in &self.relays {
            r.stop();
        }
        let _ = std::fs::remove_file(&self.socket);
        self.log.line("stop");
    }

    /// Run in a thread of this process, for tests. Dropping the handle
    /// stops the supervisor.
    pub fn spawn(self) -> Running {
        let sup = Arc::new(self);
        let stop = sup.stop_flag();
        let s = sup.clone();
        let thread = std::thread::Builder::new()
            .name("device-serve".into())
            .spawn(move || s.run())
            .expect("thread");
        Running {
            sup,
            stop,
            thread: Some(thread),
        }
    }
}

/// A supervisor that runs in a thread (tests).
pub struct Running {
    sup: Arc<Supervisor>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    pub fn socket(&self) -> &Path {
        &self.sup.socket
    }

    /// Set the stop flag and wait for the thread.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

static TERM: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    TERM.store(true, Ordering::SeqCst);
}

/// The main of `paddock device-serve`: run until SIGTERM, SIGINT or the
/// death of the parent.
pub fn main(opts: Options) -> Result<i32> {
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a plain sigaction(2) with a handler that stores a flag.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    // SAFETY: a closed connection must not kill the supervisor.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    arm_parent_death();
    let sup = Arc::new(Supervisor::start(opts)?);
    let stop = sup.stop_flag();
    let parent = unsafe { libc::getppid() };
    std::thread::Builder::new()
        .name("device-serve-watch".into())
        .spawn(move || {
            let mut n = 0u32;
            loop {
                std::thread::sleep(TICK);
                n += 1;
                let parent_gone = n.is_multiple_of(5) && unsafe { libc::getppid() } != parent;
                if TERM.load(Ordering::SeqCst) || parent_gone {
                    stop.store(true, Ordering::SeqCst);
                    return;
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
            }
        })
        .context("cannot start the watch thread")?;
    sup.run();
    Ok(0)
}

/// Linux: SIGTERM when the parent thread dies. The parent pid check of
/// `main` covers every platform.
fn arm_parent_death() {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl on this process.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
}

#[cfg(test)]
#[path = "supervise_tests.rs"]
pub(crate) mod tests;
