//! The `serve` shape: the program runs once and answers HTTP/1.1 on
//! `$PADDOCK_DEVICE_SOCKET`, the supervisor proxies.
//!
//! The supervisor starts the program with the socket path `app.sock` next
//! to its own socket, waits up to `ready` seconds for `GET /v1/health`,
//! then passes each `POST /v1/call` through byte for byte: the headers of
//! the broker, the body, and the response as the program framed it
//! (chunked stays chunked). `GET /v1/health` and `GET /v1/catalog` reach
//! the program too, so a `Paddock-Protocol-Min` of the program reaches the
//! broker.
//!
//! When the program exits, every open request gets `503`, and the
//! supervisor restarts it with a backoff of 1, 2 and 4 seconds. Three
//! exits within a minute make the device `failed`: every answer is `503`
//! with `Paddock-Status: failed` until the supervisor is restarted.

use super::exec::{cannot_start, program_env};
use super::http::{Done, Handler, Reply, Req, HEAD_ROOM};
use super::{Log, Options};
use crate::devices::program::Program;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The socket of the program, next to the supervisor's.
pub const APP_SOCKET: &str = "app.sock";
/// The backoff before each restart, in seconds.
const BACKOFF: [u64; 3] = [1, 2, 4];
/// Exits within this window count toward `failed`.
const WINDOW: Duration = Duration::from_secs(60);
const MAX_EXITS: usize = 3;
const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Ready,
    Restarting,
    Failed,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Ready => "ready",
            State::Restarting => "restarting",
            State::Failed => "failed",
        }
    }
}

struct Inner {
    argv: Vec<String>,
    app_sock: PathBuf,
    ready: Duration,
    timeout: Duration,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
    host: bool,
    log: Arc<Log>,
    state: Mutex<State>,
    cv: Condvar,
    child: Mutex<Option<Child>>,
    stopping: AtomicBool,
}

/// The `serve` handler.
pub struct Serve(Arc<Inner>);

impl Serve {
    pub fn start(opts: &Options, log: Arc<Log>) -> Result<Self> {
        let dir = opts
            .socket
            .parent()
            .context("the socket path needs a folder")?;
        let app_sock = dir.join(APP_SOCKET);
        if let Some(c) = &opts.cwd {
            if !c.is_dir() {
                anyhow::bail!("cwd {} does not exist", c.display());
            }
        }
        let mut env = program_env(opts);
        env.push((
            "PADDOCK_DEVICE_SOCKET".into(),
            app_sock.display().to_string(),
        ));
        env.push(("PADDOCK_DEVICE_PROTOCOL".into(), "1".into()));
        let inner = Arc::new(Inner {
            argv: opts.command.clone(),
            app_sock,
            ready: Duration::from_secs(opts.ready),
            timeout: Duration::from_secs(opts.timeout),
            env,
            cwd: opts.cwd.clone(),
            host: opts.host,
            log,
            state: Mutex::new(State::Starting),
            cv: Condvar::new(),
            child: Mutex::new(None),
            stopping: AtomicBool::new(false),
        });
        inner.spawn_child()?;
        let monitor = inner.clone();
        std::thread::Builder::new()
            .name("device-serve-monitor".into())
            .spawn(move || monitor.monitor())
            .context("cannot start the monitor thread")?;
        Ok(Self(inner))
    }

    pub fn state(&self) -> State {
        *self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    fn set(&self, s: State) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
        self.cv.notify_all();
        self.log.line(&format!("program {}", s.name()));
    }

    fn state(&self) -> State {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spawn_child(&self) -> Result<()> {
        let _ = std::fs::remove_file(&self.app_sock);
        let mut cmd = Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..])
            .env_clear()
            .envs(self.env.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(c) = &self.cwd {
            // PWD as named, so that `pwd` prints it (see exec.rs).
            cmd.current_dir(c).env("PWD", c);
        }
        // SAFETY: the parent-death arming returns before exec.
        unsafe {
            cmd.pre_exec(|| crate::platform::arm_parent_death_signal());
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("{}", cannot_start(&self.argv[0], self.host, &e)))?;
        if let Some(se) = child.stderr.take() {
            let log = self.log.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(se);
                let mut line = String::new();
                while let Ok(n) = r.read_line(&mut line) {
                    if n == 0 {
                        break;
                    }
                    log.line(&format!("[serve] {}", line.trim_end_matches(['\r', '\n'])));
                    line.clear();
                }
            });
        }
        self.log
            .line(&format!("program started, pid {}", child.id()));
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        Ok(())
    }

    fn kill_child(&self) {
        let mut g = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut c) = g.take() {
            // SAFETY: a signal to the group that the child leads.
            unsafe {
                libc::kill(-(c.id() as i32), libc::SIGKILL);
            }
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&self.app_sock);
    }

    /// Wait for the health of the child, then for its exit; restart with
    /// the backoff; give up after `MAX_EXITS` exits within `WINDOW`.
    fn monitor(self: Arc<Self>) {
        let mut exits: VecDeque<Instant> = VecDeque::new();
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            match self.wait_health() {
                Ok(()) => self.set(State::Ready),
                Err(e) => {
                    self.log.line(&format!("program not healthy: {e}"));
                    self.kill_child();
                }
            }
            let status = self.wait_exit();
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            self.log.line(&format!("program exited: {status}"));
            let _ = std::fs::remove_file(&self.app_sock);
            let now = Instant::now();
            exits.push_back(now);
            while exits
                .front()
                .is_some_and(|t| now.duration_since(*t) > WINDOW)
            {
                exits.pop_front();
            }
            if exits.len() >= MAX_EXITS {
                self.log.line(&format!(
                    "program exited {} times within a minute: failed until the device is restarted",
                    exits.len()
                ));
                self.set(State::Failed);
                return;
            }
            self.set(State::Restarting);
            let back = BACKOFF[exits.len().min(BACKOFF.len()) - 1];
            let until = Instant::now() + Duration::from_secs(back);
            while Instant::now() < until {
                if self.stopping.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(TICK);
            }
            if let Err(e) = self.spawn_child() {
                self.log.line(&format!("restart failed: {e:#}"));
                self.set(State::Failed);
                return;
            }
        }
    }

    /// `GET /v1/health` of the child until it answers 200, within `ready`.
    fn wait_health(&self) -> Result<(), String> {
        let end = Instant::now() + self.ready;
        let p = Program::connect(&self.app_sock).map_err(|e| e.to_string())?;
        let mut last;
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return Err("stopping".into());
            }
            if !self.child_alive() {
                return Err("the program exited before it answered /v1/health".into());
            }
            match p.get("/v1/health", Duration::from_millis(500)) {
                Ok((200, _, _)) => return Ok(()),
                Ok((code, _, _)) => last = format!("health answered {code}"),
                Err(e) => last = e,
            }
            if Instant::now() >= end {
                return Err(format!(
                    "did not answer /v1/health within {} s: {last}",
                    self.ready.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn child_alive(&self) -> bool {
        let mut g = self.child.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Wait until the child exits: its status as text.
    fn wait_exit(&self) -> String {
        loop {
            {
                let mut g = self.child.lock().unwrap_or_else(|e| e.into_inner());
                match g.as_mut() {
                    None => return "gone".into(),
                    Some(c) => match c.try_wait() {
                        Ok(Some(s)) => {
                            *g = None;
                            return s.to_string();
                        }
                        Ok(None) => {}
                        Err(e) => {
                            *g = None;
                            return format!("wait failed: {e}");
                        }
                    },
                }
            }
            if self.stopping.load(Ordering::SeqCst) {
                return "stopping".into();
            }
            std::thread::sleep(TICK);
        }
    }

    /// Wait until the program is ready, at most `wait`. The state after.
    fn await_ready(&self, wait: Duration) -> State {
        let end = Instant::now() + wait;
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *g {
                State::Ready | State::Failed => return *g,
                _ => {}
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return *g;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn unavailable(state: State) -> Reply {
        match state {
            State::Failed => Reply::paddock(
                503,
                "failed",
                "the device program exited three times within a minute; read its log and restart the device",
            ),
            s => Reply::paddock(503, s.name(), &format!("the device program is {}", s.name())),
        }
    }

    /// Pass one `GET` to the child: its status, its `Paddock-*` headers
    /// and its body.
    fn pass_get(&self, path: &str) -> Reply {
        let Ok(p) = Program::connect(&self.app_sock) else {
            return Self::unavailable(State::Restarting);
        };
        match p.get(path, Duration::from_secs(2)) {
            Ok((code, headers, body)) => {
                let mut r = Reply::body(code, body);
                for (n, v) in headers {
                    let l = n.to_ascii_lowercase();
                    if l.starts_with("paddock-") || l == "content-type" {
                        r = r.header(&n, &v);
                    }
                }
                r
            }
            Err(e) => Reply::paddock(
                503,
                "restarting",
                &format!("the program did not answer: {e}"),
            ),
        }
    }

    /// Proxy one call. None: the connection to the broker is gone.
    fn proxy(&self, conn: &mut UnixStream, req: &Req) -> Done {
        let deadline = req
            .header("paddock-deadline")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(self.timeout)
            .min(self.timeout);
        // The monitor sees an exit within 100 ms; a connect that fails
        // while the state still says ready is that window.
        let end = Instant::now() + deadline.min(self.ready);
        let mut up = loop {
            let state = self.await_ready(end.saturating_duration_since(Instant::now()));
            if state != State::Ready {
                let r = Self::unavailable(state);
                let _ = r.write(conn);
                return Done {
                    status: r.status,
                    bytes_out: r.body.len(),
                };
            }
            match UnixStream::connect(&self.app_sock) {
                Ok(u) => break u,
                Err(e) if Instant::now() < end => {
                    self.log
                        .line(&format!("cannot reach the program, waiting: {e}"));
                    std::thread::sleep(TICK);
                }
                Err(e) => {
                    let r = Reply::paddock(
                        503,
                        "restarting",
                        &format!("cannot reach the program: {e}"),
                    );
                    let _ = r.write(conn);
                    return Done {
                        status: 503,
                        bytes_out: r.body.len(),
                    };
                }
            }
        };
        let mut head = String::from("POST /v1/call HTTP/1.1\r\n");
        for (n, v) in &req.headers {
            let l = n.to_ascii_lowercase();
            if l == "connection" || l == "content-length" || l == "transfer-encoding" {
                continue;
            }
            head.push_str(&format!("{n}: {v}\r\n"));
        }
        head.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            req.body.len()
        ));
        let _ = up.set_write_timeout(Some(self.timeout));
        let _ = up
            .write_all(head.as_bytes())
            .and_then(|()| up.write_all(&req.body));
        let _ = up.set_read_timeout(Some(TICK));
        let fd = conn.as_raw_fd();
        let start = Instant::now();
        let mut buf: Vec<u8> = Vec::new();
        let mut head_done = false;
        let mut out = 0usize;
        let mut status = 0u16;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match up.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if head_done {
                        if conn.write_all(&chunk[..n]).is_err() {
                            return Done {
                                status,
                                bytes_out: out,
                            };
                        }
                        out += n;
                        continue;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let mut headers = [httparse::EMPTY_HEADER; 64];
                    let mut resp = httparse::Response::new(&mut headers);
                    match resp.parse(&buf) {
                        Ok(httparse::Status::Complete(at)) => {
                            status = resp.code.unwrap_or(0);
                            head_done = true;
                            if conn.write_all(&buf).is_err() {
                                return Done {
                                    status,
                                    bytes_out: 0,
                                };
                            }
                            out = buf.len() - at;
                            buf.clear();
                        }
                        Ok(httparse::Status::Partial) if buf.len() > HEAD_ROOM => {
                            let r = Reply::paddock(
                                502,
                                "bad response",
                                "the program sent no HTTP head",
                            );
                            let _ = r.write(conn);
                            return Done {
                                status: 502,
                                bytes_out: r.body.len(),
                            };
                        }
                        Ok(httparse::Status::Partial) => {}
                        Err(_) => {
                            let r = Reply::paddock(
                                502,
                                "bad response",
                                "the program sent no HTTP response",
                            );
                            let _ = r.write(conn);
                            return Done {
                                status: 502,
                                bytes_out: r.body.len(),
                            };
                        }
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break,
            }
            if crate::platform::peer_gone(fd) {
                // The broker cancelled: close the program's connection.
                return Done {
                    status,
                    bytes_out: out,
                };
            }
            if start.elapsed() >= self.timeout {
                if !head_done {
                    let r = Reply::paddock(
                        503,
                        "timeout",
                        &format!(
                            "the program did not answer in {} seconds",
                            self.timeout.as_secs()
                        ),
                    );
                    let _ = r.write(conn);
                    return Done {
                        status: 503,
                        bytes_out: r.body.len(),
                    };
                }
                return Done {
                    status,
                    bytes_out: out,
                };
            }
        }
        if !head_done {
            // The program closed before a response: it exited, or it broke.
            let r = Reply::paddock(
                503,
                "503",
                "the program closed the connection before it answered",
            );
            let _ = r.write(conn);
            return Done {
                status: 503,
                bytes_out: r.body.len(),
            };
        }
        let _ = conn.flush();
        Done {
            status,
            bytes_out: out,
        }
    }
}

impl Handler for Serve {
    fn health(&self) -> Reply {
        match self.0.state() {
            State::Ready => self.0.pass_get("/v1/health"),
            s => Inner::unavailable(s),
        }
    }

    fn catalog(&self) -> Reply {
        match self.0.state() {
            State::Ready => self.0.pass_get("/v1/catalog"),
            s => Inner::unavailable(s),
        }
    }

    fn call(&self, conn: &mut UnixStream, req: &Req, _id: u64) -> Done {
        self.0.proxy(conn, req)
    }

    fn shutdown(&self) {
        self.0.stopping.store(true, Ordering::SeqCst);
        self.0.kill_child();
    }
}
