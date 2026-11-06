//! The stand-in: `paddock tack run PROGRAM ARGS…`, which the PATH
//! stand-ins run (`tackshims`). It asks tackd to start the real program
//! with its secrets, and behaves like that program for its caller:
//!
//! - the arguments, the working folder and the environment go with the
//!   request; the standard streams go as file descriptors (SCM_RIGHTS), so
//!   the program reads and writes the caller's terminal or pipes itself;
//! - the signals that the stand-in gets go on to the program: a signal
//!   from the terminal (Ctrl-C, Ctrl-Z, a new window size) to the
//!   program's process group, as the terminal would, a signal from `kill`
//!   to the program alone;
//! - when the program stops (Ctrl-Z), the stand-in stops too, so the
//!   shell shows the job as stopped; `fg` continues both;
//! - the program's exit code is the stand-in's, and a program that a
//!   signal ended ends the stand-in with the same signal.
//!
//! Exit codes of the stand-in itself: `DENIED` when tackd refuses (the
//! reason is printed once), `UNAVAILABLE` when tackd is installed but does
//! not answer or cannot start the program, `LOST` when the connection to
//! tackd ends while the program runs. When tackd is not installed, or in
//! a sandbox, the stand-in runs the program itself with the references,
//! and says so once per shell session.

use super::fdpass;
use super::proto::{self, Answer, Control, Decision, Event, Start, Verdict};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// tackd refused the start (or a caller rule asks, which this paddock
/// cannot yet). EX_NOPERM of sysexits.h.
pub const DENIED: i32 = 77;
/// tackd is installed but does not answer, or could not start the
/// program. EX_UNAVAILABLE.
pub const UNAVAILABLE: i32 = 69;
/// The connection to tackd ended while the program ran. EX_SOFTWARE.
pub const LOST: i32 = 70;

/// How long the stand-in waits for tackd to start the program: a store
/// can wait for the user to unlock it.
pub const START_TIMEOUT: Duration = Duration::from_secs(150);

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Exited(i32),
    Killed {
        signal: i32,
        core: bool,
    },
    /// tackd decided deny or ask.
    Denied(Decision),
    /// tackd allowed but could not start.
    Failed(String),
    /// The connection ended while the program ran.
    Lost {
        pid: u32,
    },
}

/// The bytes of one frame.
fn frame<T: serde::Serialize>(v: &T) -> io::Result<Vec<u8>> {
    let mut b = Vec::new();
    proto::write_frame(&mut b, &proto::envelope(v))?;
    Ok(b)
}

fn read_answer(s: &UnixStream) -> Result<Answer, super::client::Error> {
    use super::client::Error;
    let mut r = s;
    let body = proto::read_frame(&mut r).map_err(|e| match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Error::TimedOut,
        io::ErrorKind::UnexpectedEof => {
            Error::Protocol("tackd closed the connection without an answer".into())
        }
        _ => Error::Io(e),
    })?;
    proto::parse_answer(&body).map_err(Error::Protocol)
}

/// Run one session on a connected socket: send the start request with
/// `fds`, then pass events and signals until the program ends.
///
/// `signals` is the read end of a pipe on which each signal arrives as two
/// bytes, the number and 1 when the terminal sent it. `on_stop` runs when
/// the program stops (the real stand-in stops itself there).
pub fn session(
    s: &UnixStream,
    req: &Start,
    fds: &[RawFd],
    signals: Option<RawFd>,
    on_stop: &mut dyn FnMut(i32),
) -> Result<Outcome, super::client::Error> {
    use super::client::Error;
    s.set_write_timeout(Some(super::client::TIMEOUT))
        .map_err(Error::Io)?;
    s.set_read_timeout(Some(START_TIMEOUT)).map_err(Error::Io)?;
    let bytes = frame(&Request::Start(req.clone())).map_err(Error::Io)?;
    fdpass::send(s, &bytes, fds).map_err(Error::Io)?;
    let pid = match read_answer(s)? {
        Answer::Started { pid, .. } => pid,
        Answer::Decision(d) => return Ok(Outcome::Denied(d)),
        Answer::Failed { message } => return Ok(Outcome::Failed(message)),
        Answer::Error { message, .. } => return Err(Error::Refused(message)),
        _ => return Err(Error::Protocol("tackd answered with something else".into())),
    };
    s.set_read_timeout(None).map_err(Error::Io)?;
    loop {
        let mut pfd = vec![libc::pollfd {
            fd: s.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        if let Some(fd) = signals {
            pfd.push(libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            });
        }
        // SAFETY: pfd is a valid array of pollfd.
        let r = unsafe { libc::poll(pfd.as_mut_ptr(), pfd.len() as libc::nfds_t, -1) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(Error::Io(e));
        }
        if pfd.len() > 1 && pfd[1].revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 64];
            // SAFETY: buf is writable for its length.
            let n =
                unsafe { libc::read(pfd[1].fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            for pair in buf[..n.max(0) as usize].as_chunks::<2>().0 {
                let c = Control::Signal {
                    signal: pair[0] as i32,
                    group: pair[1] == 1 || pair[0] as i32 == libc::SIGCONT,
                };
                if let Ok(b) = frame(&c) {
                    let mut w = s;
                    let _ = io::Write::write_all(&mut w, &b);
                }
            }
        }
        if pfd[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut r = s;
            let body = match proto::read_small_frame(&mut r, proto::MAX_EVENT) {
                Ok(b) => b,
                Err(_) => return Ok(Outcome::Lost { pid }),
            };
            match proto::parse_after::<Event>(&body) {
                Ok(Event::Exited { code }) => return Ok(Outcome::Exited(code)),
                Ok(Event::Killed { signal, core }) => return Ok(Outcome::Killed { signal, core }),
                Ok(Event::Stopped { signal }) => on_stop(signal),
                Ok(Event::Continued {}) => {}
                Err(e) => return Err(Error::Protocol(e)),
            }
        }
    }
}

use super::proto::Request;

/// The signals that the stand-in passes on.
pub fn forwarded() -> &'static [libc::c_int] {
    proto::FORWARDED
}

static SIG_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let fd = SIG_PIPE.load(std::sync::atomic::Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    // SAFETY: the kernel passes a valid siginfo for SA_SIGINFO.
    let code = if info.is_null() {
        0
    } else {
        unsafe { (*info).si_code }
    };
    let from_tty = from_terminal(sig, code);
    let b = [sig as u8, from_tty as u8];
    // SAFETY: write is async-signal-safe; a full pipe drops the signal.
    unsafe { libc::write(fd, b.as_ptr() as *const libc::c_void, 2) };
}

/// Whether a signal came from the terminal (it goes to the program's
/// process group then). Linux marks it SI_KERNEL; elsewhere the signals
/// that a terminal sends count as such.
pub fn from_terminal(sig: libc::c_int, code: libc::c_int) -> bool {
    #[cfg(target_os = "linux")]
    {
        let _ = sig;
        code == libc::SI_KERNEL
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = code;
        matches!(
            sig,
            libc::SIGINT | libc::SIGQUIT | libc::SIGTSTP | libc::SIGWINCH
        )
    }
}

/// Catch the forwarded signals into a pipe; returns its read end.
pub fn catch_signals() -> io::Result<RawFd> {
    let mut p = [0 as RawFd; 2];
    // SAFETY: p is an array of two descriptors.
    if unsafe { libc::pipe(p.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in p {
        // SAFETY: fd is one of the new pipe ends.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            let fl = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
    }
    SIG_PIPE.store(p[1], std::sync::atomic::Ordering::Relaxed);
    // A tackd that went away must not kill the stand-in on its next
    // write: the read side reports it.
    // SAFETY: setting a disposition has no preconditions.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    for &sig in forwarded() {
        // SAFETY: a zeroed sigaction with our handler and SA_SIGINFO.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(p[0])
}

/// Stop this process, as the program did (the shell sees a stopped job).
pub fn stop_self(_sig: i32) {
    // SAFETY: raise has no preconditions.
    unsafe { libc::raise(libc::SIGSTOP) };
}

/// End this process the way the program ended.
pub fn end_like(o: &Outcome) -> ! {
    match o {
        Outcome::Exited(c) => std::process::exit(*c),
        Outcome::Killed { signal, .. } => {
            // SAFETY: plain signal calls; no core file of the stand-in.
            unsafe {
                let none = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                libc::setrlimit(libc::RLIMIT_CORE, &none);
                libc::signal(*signal, libc::SIG_DFL);
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, *signal);
                libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
                libc::raise(*signal);
            }
            // A signal that does not end a process (SIGCHLD): the shell's
            // number for it.
            std::process::exit(128 + signal)
        }
        Outcome::Denied(_) => std::process::exit(DENIED),
        Outcome::Failed(_) => std::process::exit(UNAVAILABLE),
        Outcome::Lost { .. } => std::process::exit(LOST),
    }
}

/// Which of descriptors 0, 1 and 2 are open.
pub fn open_stdio() -> [bool; 3] {
    // SAFETY: F_GETFD only reads the descriptor's flags.
    [0, 1, 2].map(|fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } != -1)
}

/// The environment of this process as `NAME=value`, without entries that
/// are not UTF-8 (the protocol carries text). Returns the names dropped.
pub fn environment() -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for (k, v) in std::env::vars_os() {
        match (k.into_string(), v.into_string()) {
            (Ok(k), Ok(v)) => out.push(format!("{k}={v}")),
            _ => skipped += 1,
        }
    }
    (out, skipped)
}

/// The secrets whose allowlist names this program, from the user's own
/// configuration: the stand-in names them in its messages.
pub fn secrets_for(cfg: &super::config::Tack, program: &Path) -> Vec<String> {
    let real = std::fs::canonicalize(program).ok();
    cfg.secrets
        .iter()
        .filter(|(_, s)| {
            s.allow.iter().any(|a| {
                a.program == program
                    || real
                        .as_ref()
                        .is_some_and(|r| super::program::real_path(&a.program) == *r)
            })
        })
        .map(|(n, _)| n.clone())
        .collect()
}

/// The key of this shell session for the once-per-session notes: the
/// session id and the start time of its leader, so that a new session that
/// reuses the id is a new key.
pub fn session_key() -> String {
    // SAFETY: getsid(0) reads this process's session.
    let sid = unsafe { libc::getsid(0) };
    let start = std::fs::read_to_string(format!("/proc/{sid}/stat"))
        .ok()
        .and_then(|s| start_time(&s))
        .unwrap_or(0);
    format!("{sid}-{start}")
}

/// Field 22 of `/proc/PID/stat`, the start time, after the name in
/// parentheses (which can hold spaces).
pub fn start_time(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Print `msg` once per session and `kind`: the first stand-in of a shell
/// session says it, the next ones are quiet. Old marks are cleaned up.
pub fn once(dir: &Path, kind: &str, msg: &str) {
    once_in(
        dir,
        &session_key(),
        kind,
        msg,
        &leader_alive,
        &mut io::stderr(),
    );
}

/// [`once`] with the session key, the check of a session's leader and the
/// output given. Returns whether it printed. The mark of `key` itself is
/// never cleaned up: a session whose leader is gone (a daemon that forked
/// twice, a closed terminal) still has this process in it, and its note
/// must stay said.
pub fn once_in(
    dir: &Path,
    key: &str,
    kind: &str,
    msg: &str,
    alive: &dyn Fn(i32, u64) -> bool,
    out: &mut dyn io::Write,
) -> bool {
    if crate::privfs::dir(dir).is_err() {
        let _ = writeln!(out, "{msg}");
        return true;
    }
    let mark = dir.join(format!("{kind}-{key}"));
    use std::os::unix::fs::OpenOptionsExt;
    // create_new: of two stand-ins that start at once, one says the note.
    let new = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&mark)
        .is_ok();
    if !new {
        return false;
    }
    let _ = writeln!(out, "{msg}");
    // Marks of sessions whose leader is gone.
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let Some((sid, start)) = mark_session(&n) else {
                continue;
            };
            if format!("{sid}-{start}") != key && !alive(sid, start) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    true
}

/// The session id and leader start time in a mark name `KIND-SID-START`.
pub fn mark_session(name: &str) -> Option<(i32, u64)> {
    let mut it = name.rsplitn(3, '-');
    let start = it.next()?.parse().ok()?;
    let sid = it.next()?.parse().ok()?;
    it.next().filter(|k| !k.is_empty())?;
    Some((sid, start))
}

/// Whether the leader of session `sid`, started at `start` (0: unknown),
/// still runs. A leader that cannot be checked counts as alive, so that a
/// mark is never removed by mistake; a reused id with another start time
/// is a new process.
pub fn leader_alive(sid: i32, start: u64) -> bool {
    if sid <= 0 {
        return true;
    }
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{sid}/stat")) {
        return start == 0 || start_time(&stat).is_none_or(|s| s == start);
    }
    // SAFETY: kill with signal 0 only checks.
    let r = unsafe { libc::kill(sid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Where the once-per-session marks live.
pub fn marks_dir(env: &crate::env::HostEnv) -> PathBuf {
    match &env.runtime_dir {
        Some(r) => r.join("paddock/tack-notes"),
        None => super::state_dir(env).join("notes"),
    }
}

/// Connect to tackd. An installed tackd that is restarting gets a few
/// seconds.
fn connect(sock: &Path, installed: bool) -> Result<UnixStream, super::client::Error> {
    use super::client::Error;
    let end = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let err = match UnixStream::connect(sock) {
            Ok(s) => return Ok(s),
            Err(e) => match e.kind() {
                io::ErrorKind::NotFound => Error::NoSocket,
                io::ErrorKind::ConnectionRefused => Error::NotRunning,
                _ => Error::Io(e),
            },
        };
        let retry = installed && matches!(err, Error::NoSocket | Error::NotRunning);
        if !retry || std::time::Instant::now() > end {
            return Err(err);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Where a start goes.
#[derive(Debug)]
pub enum Route {
    /// No such program on PATH.
    NotFound(String),
    /// The stand-in runs the program itself, with the references. The note
    /// (its kind and text) is said once per shell session.
    Direct {
        program: PathBuf,
        note: Option<(&'static str, String)>,
    },
    /// tackd starts it.
    Tackd {
        program: PathBuf,
        socket: UnixStream,
    },
    /// tackd is installed and does not answer.
    Unavailable(String),
}

/// Decide where `command` goes, with tackd's files at `paths`.
/// `hint`: the secret names that a sandbox stand-in carries (a sandbox
/// does not see the user configuration).
pub fn route(
    env: &crate::env::HostEnv,
    shim: bool,
    hint: Option<&str>,
    command: &[String],
    paths: &super::service::Paths,
) -> Route {
    let Some(name) = command.first() else {
        return Route::NotFound(String::new());
    };
    let own = super::tackshims::dir(env);
    let Some(program) = super::tackshims::resolve(name, &env.path, &[own]) else {
        return Route::NotFound(name.clone());
    };
    let argv0 = name.as_str();
    if env.var("PADDOCK") == Some("1") {
        let names = hint
            .filter(|h| !h.is_empty())
            .map(|h| h.replace(',', ", "))
            .unwrap_or_else(|| "its secrets".into());
        return Route::Direct {
            program,
            note: Some((
                "sandbox",
                format!("tack: in this sandbox {argv0} gets the references of {names}, not the values: tackd starts programs with their secrets on the host"),
            )),
        };
    }
    let cfg = super::config::load(env);
    let secrets = cfg
        .as_ref()
        .map(|c| secrets_for(c, &program))
        .unwrap_or_default();
    if let Ok(c) = &cfg {
        if secrets.is_empty() {
            let listed = super::tackshims::names(c).iter().any(|n| n == argv0);
            let note = if shim && listed {
                Some((
                    "unlisted",
                    format!(
                        "tack: {} is not the {argv0} of your allowlist: it starts with the references only (paddock doctor)",
                        program.display()
                    ),
                ))
            } else if !shim {
                Some((
                    "nolist",
                    format!(
                        "tack: {} is on no allowlist of [tack]: it starts without secrets",
                        program.display()
                    ),
                ))
            } else {
                None
            };
            return Route::Direct { program, note };
        }
    }
    let names = if secrets.is_empty() {
        "its secrets".to_string()
    } else {
        secrets.join(", ")
    };
    let direct = |kind: &'static str, msg: String| Route::Direct {
        program: program.clone(),
        note: Some((kind, msg)),
    };
    if !cfg!(target_os = "linux") {
        return direct(
            "platform",
            format!("tack: on this system {argv0} gets the references of {names}: this paddock starts programs with their secrets on Linux only"),
        );
    }
    let installed = super::service::ours(&paths.unit);
    match connect(&paths.socket, installed) {
        Ok(socket) => Route::Tackd { program, socket },
        Err(super::client::Error::NoSocket) if !installed => direct(
            "rootless",
            format!("tack: tackd is not installed, so {argv0} gets the references of {names}, not the values (sudo paddock tack setup installs it)"),
        ),
        Err(e) => Route::Unavailable(format!("tack: {argv0} was not started: {e}")),
    }
}

/// The start request for `program`, from this process.
pub fn request(program: &Path, command: &[String]) -> anyhow::Result<Start> {
    let (argv0, args) = command
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("no program"))?;
    let (vars, skipped) = environment();
    if skipped > 0 {
        log::debug!("{skipped} environment variables are not UTF-8: the program does not get them");
    }
    let cwd =
        std::env::current_dir().map_err(|e| anyhow::anyhow!("the working folder is gone: {e}"))?;
    let Some(cwd) = cwd.to_str() else {
        anyhow::bail!(
            "the working folder {} is not UTF-8: tack cannot pass it",
            cwd.display()
        );
    };
    Ok(Start {
        program: program.display().to_string(),
        argv0: argv0.clone(),
        args: args.to_vec(),
        cwd: cwd.to_string(),
        caller_pid: std::process::id(),
        env: vars,
        stdio: open_stdio(),
    })
}

/// `paddock tack run [--shim] PROGRAM ARGS…`. Returns only when the
/// program could not start at all; otherwise it ends this process like the
/// program ended.
pub fn run(
    env: &crate::env::HostEnv,
    shim: bool,
    hint: Option<&str>,
    command: &[String],
) -> anyhow::Result<i32> {
    use std::os::unix::process::CommandExt;
    let paths = super::service::Paths::for_standin(env);
    let argv0 = command.first().cloned().unwrap_or_default();
    match route(env, shim, hint, command, &paths) {
        Route::NotFound(n) => {
            eprintln!("tack: {n}: not found on PATH (after the stand-ins)");
            Ok(127)
        }
        Route::Unavailable(msg) => {
            eprintln!("{msg}");
            Ok(UNAVAILABLE)
        }
        Route::Direct { program, note } => {
            if let Some((kind, msg)) = note {
                once(&marks_dir(env), kind, &msg);
            }
            let e = std::process::Command::new(&program)
                .arg0(&argv0)
                .args(&command[1..])
                .exec();
            eprintln!("tack: cannot run {}: {e}", program.display());
            Ok(if e.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            })
        }
        Route::Tackd { program, socket } => {
            let req = request(&program, command)?;
            let fds: Vec<RawFd> = (0..3).filter(|i| req.stdio[*i as usize]).collect();
            let sigs = catch_signals()?;
            let outcome = match session(&socket, &req, &fds, Some(sigs), &mut stop_self) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("tack: {argv0} was not started: {e}");
                    return Ok(UNAVAILABLE);
                }
            };
            if let Some(m) = message(&argv0, &outcome) {
                eprintln!("{m}");
            }
            end_like(&outcome)
        }
    }
}

/// What the stand-in says about an outcome, once.
pub fn message(argv0: &str, o: &Outcome) -> Option<String> {
    match o {
        Outcome::Denied(d) => Some(format!(
            "tack: {argv0} was not started: {}{}",
            d.reason,
            if d.verdict == Verdict::Ask {
                " (a caller rule asks, and this paddock cannot ask yet)"
            } else {
                ""
            }
        )),
        Outcome::Failed(m) => Some(format!("tack: tackd could not start {argv0}: {m}")),
        Outcome::Lost { pid } => Some(format!(
            "tack: the connection to tackd ended while {argv0} ran (pid {pid}); its exit status is unknown"
        )),
        _ => None,
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "standin_tests.rs"]
mod tests;
