//! The protocol between a client and tackd.
//!
//! A frame is a 4-byte big-endian length, then that many bytes of one JSON
//! object. A frame is at most `MAX_FRAME` bytes; a longer length is refused
//! before anything is read. One connection carries one request and one
//! answer, except a start: after `Answer::Started`, tackd sends `Event`s
//! (the program stopped, went on, ended) and the stand-in sends `Control`s
//! (signals for the program) on the same connection, until the program
//! ends. A start request carries the caller's standard streams as file
//! descriptors (SCM_RIGHTS) on the bytes of its frame.
//!
//! Every request and every answer names the protocol version (`v`). tackd
//! answers a version it does not speak with an error that lists the ones
//! it speaks, so an old client and a new tackd (or the reverse) fail with
//! a clear message, not a parse error.
//!
//! tackd treats every request as hostile: the sizes are bounded, unknown
//! fields are refused, and nothing that a client says about itself is
//! taken over the kernel (the caller pid must be the pid that the kernel
//! reports for the socket).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};

/// The protocol version that this paddock speaks.
pub const VERSION: u32 = 1;
/// The largest frame, in bytes. A start request carries the caller's
/// environment, so the limit is near what one exec takes on Linux.
pub const MAX_FRAME: usize = 1024 * 1024;
/// The largest frame after a start (events and controls).
pub const MAX_EVENT: usize = 4096;
/// The most environment variables that a start request can carry.
pub const MAX_ENV: usize = 4096;
/// The most arguments that a launch request can carry.
pub const MAX_ARGS: usize = 4096;
/// The longest path (program, working folder).
pub const MAX_PATH: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Request {
    /// May this program start with secrets? tackd decides and writes the
    /// audit line. It starts nothing.
    Launch(Launch),
    /// The state of tackd: version, uptime, counters.
    Status {},
    /// The audit lines of the caller's user, newest last.
    Audit { lines: usize },
    /// Decide as for Launch and, when allowed, start the program with its
    /// secrets as the caller's user. The caller's standard streams come
    /// with the frame (`stdio`).
    Start(Start),
}

/// A start request: what the stand-in knows of the program it stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    /// The program, as an absolute path. The client resolves PATH.
    pub program: String,
    /// The name that the program gets as its argument 0 (`gh`).
    pub argv0: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub caller_pid: u32,
    /// The caller's environment, `NAME=value` each. tackd drops the
    /// variables that load code into a program (`LD_PRELOAD` and others)
    /// and puts the real values of the allowed secrets in.
    pub env: Vec<String>,
    /// Which of standard input, output and error come with the request, as
    /// file descriptors in this order. A missing one becomes /dev/null.
    pub stdio: [bool; 3],
}

impl Start {
    /// The decision part of the request.
    pub fn launch(&self) -> Launch {
        Launch {
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            caller_pid: self.caller_pid,
        }
    }

    /// How many descriptors come with the request.
    pub fn fds(&self) -> usize {
        self.stdio.iter().filter(|b| **b).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    /// The program, as an absolute path. The client resolves PATH.
    pub program: String,
    /// The arguments after the program name.
    pub args: Vec<String>,
    /// The working folder of the program.
    pub cwd: String,
    /// The pid of the process that asks, as the client sees it. It must be
    /// the pid that the kernel reports for the socket.
    pub caller_pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    /// A caller rule says to ask the user. A client that cannot ask
    /// treats it as deny.
    Ask,
    Deny,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Ask => "ask",
            Verdict::Deny => "deny",
        }
    }
}

/// The answer to a launch request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub verdict: Verdict,
    /// One sentence for the user. Never a value or an argument.
    pub reason: String,
    /// The secrets that the program receives (allow), or would receive
    /// once the user says yes (ask). Names only.
    pub secrets: Vec<String>,
    /// The destination hosts of each secret in `secrets`. Empty list: no
    /// limit from tack.
    pub hosts: BTreeMap<String, Vec<String>>,
    /// The program as tackd identified it, when it could.
    pub program: Option<Program>,
    /// The callers, nearest first, as `name[pid]` with a mark for a
    /// sandbox or an agent.
    pub chain: Vec<String>,
}

/// The identity of the program file that the decision is about. A starter
/// checks that it runs this same file (device and inode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub path: String,
    pub real: String,
    pub sha256: String,
    pub dev: u64,
    pub ino: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    /// The paddock version of the tackd binary.
    pub version: String,
    /// The protocol versions that tackd speaks.
    pub protocols: Vec<u32>,
    pub pid: u32,
    pub started: u64,
    pub requests: u64,
    pub allowed: u64,
    pub denied: u64,
    pub errors: u64,
    /// Programs started since tackd started, and running now.
    #[serde(default)]
    pub started_programs: u64,
    #[serde(default)]
    pub running: u64,
    /// The audit log, and why it cannot be written, if it cannot.
    pub audit: String,
    pub audit_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Answer {
    Decision(Decision),
    Status(Status),
    Audit {
        lines: Vec<String>,
    },
    /// The request could not be handled: a bad frame, an unknown version,
    /// tackd is too busy. Nothing was decided.
    Error {
        message: String,
        protocols: Vec<u32>,
    },
    /// The program runs: `Event`s follow.
    Started {
        pid: u32,
        decision: Decision,
        /// How the program is protected from other processes of the user.
        protection: String,
    },
    /// tackd allowed the start, but could not start the program (a store
    /// that is locked, a program that changed). Nothing runs.
    Failed {
        message: String,
    },
}

/// What happens to a started program, from tackd to the stand-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Event {
    /// The program stopped on this signal (SIGTSTP, SIGSTOP).
    Stopped { signal: i32 },
    /// The program goes on after a stop.
    Continued {},
    /// The program ended with this exit code.
    Exited { code: i32 },
    /// A signal ended the program.
    Killed { signal: i32, core: bool },
}

/// From the stand-in to tackd, after the start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Control {
    /// Send `signal` to the program, or to its process group when `group`
    /// is set (a signal from the terminal: Ctrl-C, Ctrl-Z, a new size).
    Signal { signal: i32, group: bool },
}

/// The signals that a stand-in may pass on to its program. Other signals
/// are refused: a client cannot use tackd to send SIGKILL or SIGSTOP on
/// its own (a stand-in that ends makes tackd end the program).
pub const FORWARDED: &[libc::c_int] = &[
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGUSR1,
    libc::SIGUSR2,
    libc::SIGWINCH,
    libc::SIGTSTP,
    libc::SIGCONT,
    libc::SIGALRM,
];

/// The name of a signal, for messages.
pub fn signal_name(n: i32) -> String {
    let known = [
        (libc::SIGHUP, "SIGHUP"),
        (libc::SIGINT, "SIGINT"),
        (libc::SIGQUIT, "SIGQUIT"),
        (libc::SIGILL, "SIGILL"),
        (libc::SIGTRAP, "SIGTRAP"),
        (libc::SIGABRT, "SIGABRT"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGKILL, "SIGKILL"),
        (libc::SIGUSR1, "SIGUSR1"),
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGUSR2, "SIGUSR2"),
        (libc::SIGPIPE, "SIGPIPE"),
        (libc::SIGALRM, "SIGALRM"),
        (libc::SIGTERM, "SIGTERM"),
        (libc::SIGCHLD, "SIGCHLD"),
        (libc::SIGCONT, "SIGCONT"),
        (libc::SIGSTOP, "SIGSTOP"),
        (libc::SIGTSTP, "SIGTSTP"),
        (libc::SIGTTIN, "SIGTTIN"),
        (libc::SIGTTOU, "SIGTTOU"),
        (libc::SIGXCPU, "SIGXCPU"),
        (libc::SIGXFSZ, "SIGXFSZ"),
        (libc::SIGVTALRM, "SIGVTALRM"),
        (libc::SIGPROF, "SIGPROF"),
        (libc::SIGWINCH, "SIGWINCH"),
        (libc::SIGSYS, "SIGSYS"),
    ];
    known
        .iter()
        .find(|(k, _)| *k == n)
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| format!("signal {n}"))
}

impl Answer {
    pub fn error(message: impl Into<String>) -> Self {
        Answer::Error {
            message: message.into(),
            protocols: vec![VERSION],
        }
    }
}

/// Write one frame.
pub fn write_frame<T: Serialize>(w: &mut impl Write, v: &T) -> io::Result<()> {
    let body = serde_json::to_vec(v).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the message is {} bytes; the limit is {MAX_FRAME}",
                body.len()
            ),
        ));
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)?;
    w.flush()
}

/// Read one frame's bytes. A length over `MAX_FRAME` is refused before the
/// body is read; a frame that ends early is an error.
pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    read_small_frame(r, MAX_FRAME)
}

/// Parse a request frame. The version is read first, so that a request
/// of another version gets the version error, not a parse error.
pub fn parse_request(body: &[u8]) -> Result<Request, String> {
    let raw: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("the request is not JSON: {e}"))?;
    match raw.get("v").and_then(|v| v.as_u64()) {
        Some(v) if v == u64::from(VERSION) => {}
        Some(v) => {
            return Err(format!(
                "protocol version {v} is not spoken here (this tackd speaks {VERSION}): update paddock and run sudo paddock tack setup"
            ))
        }
        None => return Err("the request names no protocol version (v)".into()),
    }
    let req: Request = serde_json::from_value(without_version(raw))
        .map_err(|e| format!("the request is invalid: {e}"))?;
    match &req {
        Request::Launch(l) => check_launch(l)?,
        Request::Start(s) => check_start(s)?,
        _ => {}
    }
    Ok(req)
}

/// Bounds on a start request: those of a launch, the name and the
/// environment.
pub fn check_start(s: &Start) -> Result<(), String> {
    check_launch(&s.launch())?;
    if s.argv0.is_empty() || s.argv0.len() > MAX_PATH || s.argv0.contains('\0') {
        return Err("argv0 must be 1 to 4096 bytes without a NUL byte".into());
    }
    if s.env.len() > MAX_ENV {
        return Err(format!("more than {MAX_ENV} environment variables"));
    }
    for e in &s.env {
        let Some((name, value)) = e.split_once('=') else {
            return Err("an environment entry has no '='".into());
        };
        if name.is_empty() || name.contains('\0') || value.contains('\0') {
            return Err("an environment entry has an empty name or a NUL byte".into());
        }
    }
    Ok(())
}

/// Parse an event (the stand-in side) or a control (the tackd side): the
/// frames after a start.
pub fn parse_after<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    let raw: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("the message is not JSON: {e}"))?;
    if raw.get("v").and_then(|v| v.as_u64()) != Some(u64::from(VERSION)) {
        return Err(format!("the message is not of protocol version {VERSION}"));
    }
    serde_json::from_value(without_version(raw)).map_err(|e| format!("the message is invalid: {e}"))
}

/// Read one frame of at most `max` bytes.
pub fn read_small_frame(r: &mut impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a message of {n} bytes; the limit is {max}"),
        ));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// Bounds on a launch request, before anything looks at the file system.
pub fn check_launch(l: &Launch) -> Result<(), String> {
    for (what, p) in [("program", &l.program), ("cwd", &l.cwd)] {
        if p.is_empty() || !p.starts_with('/') {
            return Err(format!("{what} must be an absolute path"));
        }
        if p.len() > MAX_PATH {
            return Err(format!("{what} is longer than {MAX_PATH} bytes"));
        }
        if p.contains('\0') {
            return Err(format!("{what} holds a NUL byte"));
        }
    }
    if l.args.len() > MAX_ARGS {
        return Err(format!("more than {MAX_ARGS} arguments"));
    }
    if l.args.iter().any(|a| a.contains('\0')) {
        return Err("an argument holds a NUL byte".into());
    }
    if l.caller_pid == 0 {
        return Err("caller_pid is 0".into());
    }
    Ok(())
}

/// Parse an answer frame (the client side).
pub fn parse_answer(body: &[u8]) -> Result<Answer, String> {
    let raw: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("tackd's answer is not JSON: {e}"))?;
    match raw.get("v").and_then(|v| v.as_u64()) {
        Some(v) if v == u64::from(VERSION) => {}
        Some(v) => {
            return Err(format!(
                "tackd speaks protocol version {v} and this paddock speaks {VERSION}: run sudo paddock tack setup with this paddock"
            ))
        }
        None => return Err("tackd's answer names no protocol version".into()),
    }
    serde_json::from_value(without_version(raw))
        .map_err(|e| format!("tackd's answer is invalid: {e}"))
}

fn without_version(mut raw: serde_json::Value) -> serde_json::Value {
    if let Some(m) = raw.as_object_mut() {
        m.remove("v");
    }
    raw
}

/// A request or an answer with the protocol version: `{"v": 1, ...}`.
pub fn envelope<T: Serialize>(body: T) -> serde_json::Value {
    let mut v = serde_json::to_value(body).unwrap_or_default();
    if let Some(m) = v.as_object_mut() {
        m.insert("v".into(), VERSION.into());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch {
            program: "/usr/bin/gh".into(),
            args: vec!["pr".into(), "list".into()],
            cwd: "/home/demo/p".into(),
            caller_pid: 42,
        }
    }

    fn frame<T: Serialize>(v: &T) -> Vec<u8> {
        let mut b = Vec::new();
        write_frame(&mut b, v).unwrap();
        b
    }

    #[test]
    fn a_request_round_trips_through_a_frame() {
        let req = Request::Launch(launch());
        let b = frame(&envelope(req.clone()));
        assert_eq!(&b[..4], &((b.len() - 4) as u32).to_be_bytes());
        let body = read_frame(&mut &b[..]).unwrap();
        assert_eq!(parse_request(&body).unwrap(), req);
        for r in [Request::Status {}, Request::Audit { lines: 5 }] {
            let body = read_frame(&mut &frame(&envelope(r.clone()))[..]).unwrap();
            assert_eq!(parse_request(&body).unwrap(), r);
        }
    }

    #[test]
    fn an_answer_round_trips_and_names_its_version() {
        let a = Answer::Decision(Decision {
            verdict: Verdict::Deny,
            reason: "no".into(),
            secrets: vec![],
            hosts: BTreeMap::new(),
            program: None,
            chain: vec!["bash[1]".into()],
        });
        let b = frame(&envelope(a.clone()));
        let text = String::from_utf8_lossy(&b[4..]).to_string();
        assert!(
            text.contains("\"v\":1") && text.contains("\"type\":\"decision\""),
            "{text}"
        );
        assert_eq!(parse_answer(&read_frame(&mut &b[..]).unwrap()).unwrap(), a);
    }

    #[test]
    fn an_oversized_length_is_refused_before_the_body_is_read() {
        let mut b = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(b"{}");
        let e = read_frame(&mut &b[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("limit"), "{e}");
        let big = Launch {
            args: vec!["x".repeat(MAX_FRAME)],
            ..launch()
        };
        assert!(write_frame(&mut Vec::new(), &envelope(Request::Launch(big))).is_err());
    }

    #[test]
    fn a_frame_that_ends_early_is_an_error() {
        let b = frame(&envelope(Request::Status {}));
        for cut in [0, 2, 4, b.len() - 1] {
            assert!(read_frame(&mut &b[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn another_version_gets_a_version_error_not_a_parse_error() {
        let e = parse_request(br#"{"v":2,"type":"launch","whatever":1}"#).unwrap_err();
        assert!(e.contains("version 2") && e.contains("speaks 1"), "{e}");
        let e = parse_request(br#"{"type":"status"}"#).unwrap_err();
        assert!(e.contains("no protocol version"), "{e}");
        let e = parse_answer(br#"{"v":9,"type":"status"}"#).unwrap_err();
        assert!(e.contains("sudo paddock tack setup"), "{e}");
    }

    #[test]
    fn hostile_requests_are_refused_with_a_reason() {
        for (body, want) in [
            (&b"not json"[..], "not JSON"),
            (br#"{"v":1,"type":"launch"}"#, "invalid"),
            (br#"{"v":1,"type":"status","extra":1}"#, "invalid"),
            (br#"{"v":1,"type":"rm"}"#, "invalid"),
            (br#"{"v":1,"type":"audit","lines":-1}"#, "invalid"),
        ] {
            let e = parse_request(body).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
        let cases: Vec<(Launch, &str)> = vec![
            (
                Launch {
                    program: "gh".into(),
                    ..launch()
                },
                "absolute",
            ),
            (
                Launch {
                    cwd: "".into(),
                    ..launch()
                },
                "absolute",
            ),
            (
                Launch {
                    program: format!("/{}", "a".repeat(MAX_PATH)),
                    ..launch()
                },
                "longer",
            ),
            (
                Launch {
                    program: "/bin/a\0b".into(),
                    ..launch()
                },
                "NUL",
            ),
            (
                Launch {
                    args: vec!["a\0".into()],
                    ..launch()
                },
                "NUL",
            ),
            (
                Launch {
                    args: vec![String::new(); MAX_ARGS + 1],
                    ..launch()
                },
                "arguments",
            ),
            (
                Launch {
                    caller_pid: 0,
                    ..launch()
                },
                "caller_pid",
            ),
        ];
        for (l, want) in cases {
            let body = serde_json::to_vec(&envelope(Request::Launch(l))).unwrap();
            let e = parse_request(&body).unwrap_err();
            assert!(e.contains(want), "{want}: {e}");
        }
    }
}
