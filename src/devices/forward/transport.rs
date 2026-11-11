//! The transport of a remote device: a Unix socket, or the standard
//! input and output of `ssh HOST paddock device serve-remote`.
//!
//! One connection is one request, as the socket framing wants. `Conn`
//! holds the two ends and a `Closer`: closing from another thread is how
//! a cancel and the deadline reach the other side, which sees its
//! connection go and cancels as a close of the device file does.

use crate::env::HostEnv;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The arguments of ssh before the destination. `-T`: no terminal, the
/// bytes go through untouched. `BatchMode`: never a password prompt (a
/// broker has no terminal to answer it), so a key that does not log in
/// fails at once with ssh's own text instead of hanging to the deadline.
pub const SSH_ARGS: &[&str] = &["-T", "-o", "BatchMode=yes", "--"];
/// What ssh runs on the other side.
pub const REMOTE_COMMAND: &[&str] = &["paddock", "device", "serve-remote"];
/// The most of ssh's standard error that the log keeps.
const STDERR_KEEP: usize = 4096;
/// How long the end of a request waits for ssh to exit by itself.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Where the other paddock is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `ssh://[user@]host[:port]`, as ssh takes it.
    Ssh(String),
    /// `unix:/path/to/devices.sock`.
    Unix(PathBuf),
}

impl Target {
    /// Parse the `target` of a table. The error names the fix.
    pub fn parse(s: &str) -> Result<Self, String> {
        if let Some(h) = s.strip_prefix("ssh://") {
            let ok = !h.is_empty()
                && h.len() <= 253
                && !h.starts_with('-')
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':'));
            if !ok {
                return Err(format!(
                    "target {s:?}: ssh:// needs a host as ssh takes it, for example ssh://mac or ssh://me@mac.local:2222"
                ));
            }
            return Ok(Target::Ssh(h.to_string()));
        }
        if let Some(p) = s.strip_prefix("unix:") {
            if !p.starts_with('/') {
                return Err(format!(
                    "target {s:?}: unix: needs an absolute socket path, for example unix:/tmp/paddock-mac/devices.sock"
                ));
            }
            return Ok(Target::Unix(PathBuf::from(p)));
        }
        Err(format!("target {s:?} must be ssh://HOST or unix:/PATH"))
    }

    /// The table spelling.
    pub fn render(&self) -> String {
        match self {
            Target::Ssh(h) => format!("ssh://{h}"),
            Target::Unix(p) => format!("unix:{}", p.display()),
        }
    }

    /// `ssh` or `unix`, for the catalog and the audit line.
    pub fn transport(&self) -> &'static str {
        match self {
            Target::Ssh(_) => "ssh",
            Target::Unix(_) => "unix",
        }
    }

    /// The host of an ssh target.
    pub fn host(&self) -> Option<&str> {
        match self {
            Target::Ssh(h) => Some(h),
            Target::Unix(_) => None,
        }
    }

    /// True for a transport that runs a program on this host as the user
    /// (ssh with the user's configuration and keys).
    pub fn host_action(&self) -> bool {
        matches!(self, Target::Ssh(_))
    }

    /// Open one connection. The error is text for the log and the
    /// catalog.
    pub fn open(&self, env: &HostEnv) -> Result<Conn, String> {
        match self {
            Target::Unix(p) => {
                let s = UnixStream::connect(p)
                    .map_err(|e| format!("cannot connect to {}: {e}", p.display()))?;
                let r = s
                    .try_clone()
                    .map_err(|e| format!("cannot clone the socket: {e}"))?;
                Ok(Conn {
                    reader: Box::new(r),
                    writer: Box::new(s.try_clone().map_err(|e| e.to_string())?),
                    closer: Closer::Unix(Arc::new(s)),
                })
            }
            Target::Ssh(h) => open_ssh(env, h),
        }
    }
}

/// `ssh` on the PATH of the host, with the user's environment.
fn open_ssh(env: &HostEnv, host: &str) -> Result<Conn, String> {
    let ssh = env
        .which("ssh")
        .ok_or_else(|| "ssh is not on the PATH of the host".to_string())?;
    let mut c = Command::new(ssh);
    c.args(SSH_ARGS)
        .arg(format!("ssh://{host}"))
        .args(REMOTE_COMMAND)
        .env_clear()
        .envs(&env.vars)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("cannot start ssh: {e}"))?;
    let stdin = child.stdin.take().ok_or("ssh has no stdin")?;
    let stdout = child.stdout.take().ok_or("ssh has no stdout")?;
    let stderr = child.stderr.take().ok_or("ssh has no stderr")?;
    let text = Arc::new(Mutex::new(String::new()));
    let t2 = text.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.take(STDERR_KEEP as u64).read_to_end(&mut buf);
        *t2.lock().unwrap_or_else(|e| e.into_inner()) =
            String::from_utf8_lossy(&buf).trim().to_string();
    });
    Ok(Conn {
        reader: Box::new(stdout),
        writer: Box::new(stdin),
        closer: Closer::Ssh(Arc::new(Mutex::new(Some(child))), text),
    })
}

/// How to close a connection from another thread.
#[derive(Clone)]
pub enum Closer {
    Unix(Arc<UnixStream>),
    /// The ssh process and what it wrote to its standard error.
    Ssh(Arc<Mutex<Option<Child>>>, Arc<Mutex<String>>),
}

impl Closer {
    /// Close the connection: the other side sees it go. For ssh, kill the
    /// process (the session ends, the remote process sees its stdin go).
    pub fn close(&self) {
        match self {
            Closer::Unix(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Closer::Ssh(child, _) => {
                let mut g = child.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(c) = g.as_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                *g = None;
            }
        }
    }

    /// The end of a request: let ssh exit by itself for a moment (the
    /// remote process ends after its one request), then make sure.
    pub fn finish(&self) {
        if let Closer::Ssh(child, _) = self {
            let end = Instant::now() + EXIT_GRACE;
            loop {
                let done = {
                    let mut g = child.lock().unwrap_or_else(|e| e.into_inner());
                    match g.as_mut() {
                        None => true,
                        Some(c) => matches!(c.try_wait(), Ok(Some(_))),
                    }
                };
                if done || Instant::now() >= end {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.close();
    }

    /// What ssh said on its standard error, for the log. Empty for a
    /// socket.
    pub fn stderr(&self) -> String {
        match self {
            Closer::Unix(_) => String::new(),
            Closer::Ssh(_, text) => {
                // The reader thread may still be at it: a short wait.
                let end = Instant::now() + Duration::from_millis(200);
                loop {
                    let t = text.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if !t.is_empty() || Instant::now() >= end {
                        return t;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

/// One open connection: the two ends, and the closer for another thread.
pub struct Conn {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub closer: Closer,
}
