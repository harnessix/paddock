//! The `exec` shape: the fixed command runs once per request.
//!
//! argv is the command list, nothing else, ever. The request is the
//! standard input (a thread writes it, so a program that does not read
//! cannot block), the response is the standard output, capped at
//! `max_response + 1` bytes. stderr goes to the log with the request id as
//! a prefix, at most 64 KiB per request. The working folder is a fresh
//! empty folder under `$TMPDIR/req-N`, removed after. The process group is
//! killed at `timeout` and when the broker closes the connection.
//!
//! The exit code mapping of the device protocol: 65 is `400`, 75 is `202`,
//! 77 is `403`; every other exit is a `200` with the envelope of 0.2.1
//! (`status:` line, `message:`, `stderr:` lines, an empty line, the
//! standard output). With `--stream` the response is the raw standard
//! output as a chunked body, and the exit status is the trailer
//! `Paddock-Exit`.

use super::http::{Done, Handler, Reply, Req};
use super::{Log, Options};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Lines of stderr in the envelope, and the length of each.
pub const STDERR_LINES: usize = 5;
pub const STDERR_LINE: usize = 200;
/// How much stderr the supervisor reads at all, per request.
pub const STDERR_READ: u64 = 64 * 1024;
/// How often a running request checks for a closed connection.
const PEER_CHECK: Duration = Duration::from_millis(100);
/// How often the loop looks at the child.
const POLL: Duration = Duration::from_millis(10);
/// Supervisors started in this process, for the name of the work folder.
static INSTANCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The request headers that become variables of the command, and nothing
/// else of the request.
pub const HEADER_ENV: &[(&str, &str)] = &[
    ("paddock-device", "PADDOCK_DEVICE"),
    ("paddock-protocol", "PADDOCK_DEVICE_PROTOCOL"),
    ("paddock-caller", "PADDOCK_CALLER"),
    ("paddock-caller-harness", "PADDOCK_CALLER_HARNESS"),
    ("paddock-caller-project", "PADDOCK_CALLER_PROJECT"),
    ("paddock-caller-mount", "PADDOCK_CALLER_MOUNT"),
    ("paddock-caller-chain", "PADDOCK_CALLER_CHAIN"),
    ("paddock-request", "PADDOCK_REQUEST"),
    ("paddock-deadline", "PADDOCK_DEADLINE"),
    ("paddock-max-response", "PADDOCK_MAX_RESPONSE"),
];

/// The variables of the supervisor's own environment that a program gets:
/// the same set that `[exec]` gives.
const BASE_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "TMPDIR",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_COLLATE",
    "LC_NUMERIC",
    "LC_TIME",
    "TZ",
];

/// The base environment of a program: `BASE_ENV` and the names of
/// `env_pass` from this process, and `TERM=dumb`.
pub fn base_env(env_pass: &[String]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = BASE_ENV
        .iter()
        .map(|k| k.to_string())
        .chain(env_pass.iter().cloned())
        .filter_map(|k| std::env::var(&k).ok().map(|val| (k, val)))
        .collect();
    v.push(("TERM".into(), "dumb".into()));
    v
}

/// The environment of a program: the whole environment of the supervisor
/// for a host device (`--host`), else `base_env` with the `--env-pass`
/// names.
pub fn program_env(opts: &Options) -> Vec<(String, String)> {
    let mut v = if opts.host {
        std::env::vars().collect()
    } else {
        base_env(&opts.env_pass)
    };
    for (k, val) in &opts.env {
        v.retain(|(n, _)| n != k);
        v.push((k.clone(), val.clone()));
    }
    v
}

/// Where a program's working folder must exist: the envelope of a request
/// whose `--cwd` is gone. `~` stands for the home of the supervisor.
pub fn cwd_missing(cwd: &Path) -> Envelope {
    let shown = match std::env::var_os("HOME").map(PathBuf::from) {
        Some(h) if !h.as_os_str().is_empty() => crate::devices::grant::tilde(cwd, &h),
        _ => cwd.display().to_string(),
    };
    Envelope::error(format!("cwd {shown} does not exist"))
}

/// The message of a program that cannot start: a name without a slash
/// that is not on the PATH, or the error of the system.
pub fn cannot_start(argv0: &str, host: bool, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound && !argv0.contains('/') {
        return format!(
            "{argv0} is not on the PATH of {}",
            if host {
                "the host"
            } else {
                "the device sandbox"
            }
        );
    }
    format!("cannot start the command: {e}")
}

/// The `PADDOCK_*` variables of one request, from its headers.
pub fn request_env(req: &Req) -> Vec<(String, String)> {
    HEADER_ENV
        .iter()
        .filter_map(|(h, var)| {
            req.header(h)
                .map(|v| (var.to_string(), crate::broker::clean_text(v)))
        })
        .collect()
}

/// What one run gave, as the envelope of 0.2.1 shows it.
pub struct Envelope {
    pub status: String,
    pub message: Option<String>,
    pub stderr: String,
    pub stdout: Vec<u8>,
}

impl Envelope {
    pub fn error(message: String) -> Self {
        Self {
            status: "error".into(),
            message: Some(message),
            stderr: String::new(),
            stdout: Vec::new(),
        }
    }

    /// The response: header lines, an empty line, the standard output.
    pub fn response(&self) -> Vec<u8> {
        let mut head = format!("status: {}\n", self.status);
        if let Some(m) = &self.message {
            head.push_str(&format!("message: {}\n", one_line(m, 500)));
        }
        let lines: Vec<&str> = self
            .stderr
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        for l in lines.iter().take(STDERR_LINES) {
            head.push_str(&format!("stderr: {}\n", one_line(l, STDERR_LINE)));
        }
        if lines.len() > STDERR_LINES {
            head.push_str(&format!(
                "stderr: ({} more lines)\n",
                lines.len() - STDERR_LINES
            ));
        }
        head.push('\n');
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.stdout);
        out
    }
}

/// One line without control characters, at most `max` characters.
pub fn one_line(s: &str, max: usize) -> String {
    let clean: String = crate::broker::clean_text(s)
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect();
    if clean.chars().count() > max {
        let mut t: String = clean.chars().take(max).collect();
        t.push_str("...");
        t
    } else {
        clean
    }
}

/// The `exec` handler.
pub struct Exec {
    argv: Vec<String>,
    timeout: Duration,
    max_response: usize,
    stream: bool,
    env: Vec<(String, String)>,
    tmp: PathBuf,
    /// `--cwd`: the fixed working folder, in place of a fresh one.
    cwd: Option<PathBuf>,
    host: bool,
    log: Arc<Log>,
    /// The process groups at work, for the shutdown.
    groups: Mutex<HashSet<i32>>,
}

/// How a run ended.
enum End {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
}

struct Run {
    end: End,
    stdout: Vec<u8>,
    stderr: String,
    too_large: bool,
    /// Stream: the head went out, the bytes are on the wire.
    streamed: Option<usize>,
}

/// Where the standard output goes while the program runs.
enum Out<'a> {
    Collect(Vec<u8>),
    Stream {
        conn: &'a mut UnixStream,
        head_sent: bool,
        sent: usize,
        broken: bool,
    },
}

impl Out<'_> {
    /// False when the output passed the limit.
    fn push(&mut self, piece: &[u8], max: usize) -> bool {
        match self {
            Out::Collect(v) => {
                v.extend_from_slice(piece);
                v.len() <= max
            }
            Out::Stream {
                conn,
                head_sent,
                sent,
                broken,
            } => {
                if *sent + piece.len() > max {
                    return false;
                }
                if !*head_sent {
                    *head_sent = true;
                    if conn.write_all(stream_head().as_bytes()).is_err() {
                        *broken = true;
                    }
                }
                *sent += piece.len();
                let frame = format!("{:x}\r\n", piece.len());
                if conn
                    .write_all(frame.as_bytes())
                    .and_then(|()| conn.write_all(piece))
                    .and_then(|()| conn.write_all(b"\r\n"))
                    .is_err()
                {
                    *broken = true;
                }
                true
            }
        }
    }
}

fn stream_head() -> String {
    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: Paddock-Exit\r\nConnection: close\r\n\r\n".to_string()
}

impl Exec {
    pub fn new(opts: &Options, log: Arc<Log>) -> Self {
        // One private parent per supervisor: several supervisors can share
        // a TMPDIR (the host, the tests), and their request ids repeat.
        let n = INSTANCES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(format!("paddock-device-{}-{n}", std::process::id()));
        if let Err(e) = crate::privfs::dir(&tmp) {
            log.line(&format!("cannot create {}: {e}", tmp.display()));
        }
        Self {
            argv: opts.command.clone(),
            timeout: Duration::from_secs(opts.timeout),
            max_response: opts.max_response,
            stream: opts.stream,
            env: program_env(opts),
            tmp,
            cwd: opts.cwd.clone(),
            host: opts.host,
            log,
            groups: Mutex::new(HashSet::new()),
        }
    }

    fn kill_group(pid: i32) {
        // SAFETY: a signal to the group that the child leads.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }

    fn run(
        &self,
        conn: &mut UnixStream,
        req: &Req,
        id: u64,
        work: &std::path::Path,
    ) -> Result<Run, Envelope> {
        let mut env = self.env.clone();
        env.extend(request_env(req));
        let defaults = [
            ("PADDOCK_REQUEST", id.to_string()),
            ("PADDOCK_DEADLINE", self.timeout.as_secs().to_string()),
            ("PADDOCK_MAX_RESPONSE", self.max_response.to_string()),
            ("PADDOCK_DEVICE_PROTOCOL", "1".to_string()),
        ];
        for (k, v) in defaults {
            if !env.iter().any(|(n, _)| n == k) {
                env.push((k.to_string(), v));
            }
        }
        let mut cmd = Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..])
            .env_clear()
            .envs(env)
            .current_dir(work)
            // The folder as named, so that `pwd` prints it: on macOS /tmp
            // is a link, and a shell without PWD prints /private/tmp.
            .env("PWD", work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: the parent-death arming returns before exec.
        unsafe {
            cmd.pre_exec(|| crate::platform::arm_parent_death_signal());
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Envelope::error(cannot_start(&self.argv[0], self.host, &e)))?;
        let pid = child.id() as i32;
        self.groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid);
        let r = self.drive(conn, &mut child, req, id);
        Self::kill_group(pid);
        let _ = child.wait();
        self.groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&pid);
        Ok(r)
    }

    fn drive(&self, conn: &mut UnixStream, child: &mut Child, req: &Req, id: u64) -> Run {
        if let Some(mut stdin) = child.stdin.take() {
            let data = req.body.clone();
            std::thread::spawn(move || {
                let _ = stdin.write_all(&data);
            });
        }
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let mut so = child.stdout.take().expect("piped");
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match so.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let se = child.stderr.take().expect("piped");
        let log = self.log.clone();
        let errs = std::thread::spawn(move || {
            let mut text = String::new();
            let mut r = BufReader::new(se.take(STDERR_READ));
            let mut line = String::new();
            while let Ok(n) = r.read_line(&mut line) {
                if n == 0 {
                    break;
                }
                let l = line.trim_end_matches(['\r', '\n']);
                log.line(&format!("[{id}] {l}"));
                text.push_str(l);
                text.push('\n');
                line.clear();
            }
            text
        });
        let fd = conn.as_raw_fd();
        let max = self.max_response;
        let mut out = if self.stream {
            Out::Stream {
                conn,
                head_sent: false,
                sent: 0,
                broken: false,
            }
        } else {
            Out::Collect(Vec::new())
        };
        let pid = child.id() as i32;
        let start = Instant::now();
        let mut last_peer = Instant::now();
        let mut too_large = false;
        let end = loop {
            while let Ok(piece) = rx.try_recv() {
                if !out.push(&piece, max) {
                    too_large = true;
                }
            }
            if too_large {
                Self::kill_group(pid);
                break child.wait().map(End::Exited).unwrap_or(End::TimedOut);
            }
            if let Out::Stream { broken: true, .. } = out {
                Self::kill_group(pid);
                break End::Cancelled;
            }
            match child.try_wait() {
                Ok(Some(s)) => break End::Exited(s),
                Ok(None) => {}
                Err(_) => break End::TimedOut,
            }
            if last_peer.elapsed() >= PEER_CHECK {
                last_peer = Instant::now();
                if crate::platform::peer_gone(fd) {
                    Self::kill_group(pid);
                    break End::Cancelled;
                }
            }
            if start.elapsed() >= self.timeout {
                Self::kill_group(pid);
                let _ = child.wait();
                break End::TimedOut;
            }
            std::thread::sleep(POLL);
        };
        // The output ends when the last process that holds it ends.
        let drain_until = Instant::now() + Duration::from_secs(2);
        while !too_large {
            match rx.recv_timeout(drain_until.saturating_duration_since(Instant::now())) {
                Ok(piece) => {
                    if !out.push(&piece, max) {
                        too_large = true;
                    }
                }
                Err(_) => break,
            }
        }
        let stderr = errs.join().unwrap_or_default();
        let (stdout, streamed) = match out {
            Out::Collect(v) => (v, None),
            Out::Stream {
                head_sent, sent, ..
            } => (Vec::new(), head_sent.then_some(sent)),
        };
        Run {
            end,
            stdout,
            stderr,
            too_large,
            streamed,
        }
    }

    /// The status text of an end, as the envelope and the trailer say it.
    fn status_of(end: &End) -> String {
        match end {
            End::TimedOut => "timeout".into(),
            End::Cancelled => "cancelled".into(),
            End::Exited(s) => match (s.code(), s.signal()) {
                (Some(c), _) => format!("exit {c}"),
                (None, Some(sig)) => format!("signal {sig}"),
                _ => "error".into(),
            },
        }
    }

    /// The reply of a collected run.
    fn reply(&self, run: Run) -> Reply {
        if run.too_large {
            return Reply::paddock(
                500,
                "too large",
                &format!("the program wrote more than {} bytes", self.max_response),
            );
        }
        let status = Self::status_of(&run.end);
        let code = match &run.end {
            End::Exited(s) => s.code(),
            _ => None,
        };
        let message = matches!(run.end, End::TimedOut).then(|| {
            format!(
                "the command did not finish in {} seconds and was stopped",
                self.timeout.as_secs()
            )
        });
        let env = Envelope {
            status,
            message,
            stderr: run.stderr,
            stdout: run.stdout,
        };
        let body = env.response();
        match code {
            Some(65) => Reply::body(400, body).header("Paddock-Status", "rejected"),
            Some(75) => Reply::body(202, body).header("Paddock-Status", "waiting"),
            Some(77) => Reply::body(403, body).header("Paddock-Status", "declined"),
            _ => Reply::body(200, body),
        }
    }
}

impl Handler for Exec {
    fn health(&self) -> Reply {
        Reply::new(200).header("Paddock-Protocol", "1")
    }

    fn catalog(&self) -> Reply {
        Reply::paddock(404, "not found", "an exec program has no catalog")
    }

    fn call(&self, conn: &mut UnixStream, req: &Req, id: u64) -> Done {
        let (work, fresh) = match &self.cwd {
            Some(c) => (c.clone(), false),
            None => (self.tmp.join(format!("req-{id}")), true),
        };
        if fresh {
            let _ = std::fs::remove_dir_all(&work);
            if let Err(e) = crate::privfs::dir(&work) {
                let r = Reply::paddock(
                    500,
                    "failed",
                    &format!("cannot create the working folder {}: {e}", work.display()),
                );
                let _ = r.write(conn);
                return Done {
                    status: 500,
                    bytes_out: r.body.len(),
                };
            }
        } else if !work.is_dir() {
            let r = Reply::body(200, cwd_missing(&work).response());
            let _ = r.write(conn);
            return Done {
                status: 200,
                bytes_out: r.body.len(),
            };
        }
        let run = self.run(conn, req, id, &work);
        if fresh {
            let _ = std::fs::remove_dir_all(&work);
        }
        let run = match run {
            Ok(r) => r,
            Err(env) => {
                let r = Reply::body(200, env.response());
                let _ = r.write(conn);
                return Done {
                    status: 200,
                    bytes_out: r.body.len(),
                };
            }
        };
        if matches!(run.end, End::Cancelled) {
            return Done {
                status: 0,
                bytes_out: run.streamed.unwrap_or(0),
            };
        }
        if let Some(sent) = run.streamed {
            // The head and the bytes are out: the trailer ends the stream.
            let mut trailer = format!("0\r\nPaddock-Exit: {}\r\n", stream_exit(&run.end));
            if run.too_large {
                trailer.push_str("Paddock-Status: too large\r\n");
            }
            trailer.push_str("\r\n");
            let _ = conn.write_all(trailer.as_bytes());
            let _ = conn.flush();
            return Done {
                status: 200,
                bytes_out: sent,
            };
        }
        if self.stream && !run.too_large {
            // No byte came before the end: a status reply, or an empty
            // stream with its trailer.
            let code = match &run.end {
                End::Exited(s) => s.code(),
                _ => None,
            };
            if !matches!(code, Some(65) | Some(75) | Some(77)) {
                let text = format!(
                    "{}0\r\nPaddock-Exit: {}\r\n\r\n",
                    stream_head(),
                    stream_exit(&run.end)
                );
                let _ = conn.write_all(text.as_bytes());
                let _ = conn.flush();
                return Done {
                    status: 200,
                    bytes_out: 0,
                };
            }
        }
        let r = self.reply(run);
        let _ = r.write(conn);
        Done {
            status: r.status,
            bytes_out: r.body.len(),
        }
    }

    fn shutdown(&self) {
        let groups: Vec<i32> = self
            .groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();
        for g in groups {
            Self::kill_group(g);
        }
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

impl Drop for Exec {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

/// The `Paddock-Exit` trailer: the exit code, `signal N` or `timeout`.
fn stream_exit(end: &End) -> String {
    match end {
        End::Exited(s) => match (s.code(), s.signal()) {
            (Some(c), _) => c.to_string(),
            (None, Some(sig)) => format!("signal {sig}"),
            _ => "error".into(),
        },
        End::TimedOut => "timeout".into(),
        End::Cancelled => "cancelled".into(),
    }
}
