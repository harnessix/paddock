//! tackd: the root service that decides which program receives which
//! secret, starts the allowed programs with it (Linux), and writes the
//! audit log.
//!
//! - It listens on a socket that every user can reach (0666, in a
//!   root-owned 0755 folder). Who asks comes from the kernel, never from
//!   the request (`peer`).
//! - It reads the `[tack]` table of the asking uid's own configuration
//!   (`~/.config/paddock/config.toml` of that uid's home, a file that uid
//!   owns). It never reads another uid's configuration or store for a
//!   request.
//! - One thread per connection, at most `MAX_CONNS` at once; a client gets
//!   `FRAME_DEADLINE` to send its request. A client that is too slow, too
//!   large or malformed gets an error answer and is dropped; it cannot hold
//!   tackd up for the others.
//! - Every decision is written to the audit log before the answer is
//!   sent. When it cannot be written, tackd does not allow.
//! - A start request that is allowed starts the program (`Starter`; on
//!   Linux `start::Linux`) and keeps the connection until the program ends:
//!   tackd sends what happens to the program and passes on the signals
//!   that the stand-in sends. A running program does not hold one of the
//!   connection slots above; it counts against `MAX_RUNNING_PER_UID`.

use super::audit::{Log, Record};
use super::config::{self, Tack};
use super::ident::{self, HashCache};
use super::peer;
use super::policy::{self, Input, Peer};
use super::procs::{self, Table};
use super::proto::{self, Answer, Control, Decision, Event, Request, Start, Status, Verdict};
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The most connections served at once.
pub const MAX_CONNS: usize = 32;
/// The most connections of one uid at once: a user whose requests hang
/// (a program path on a FUSE mount that never answers) cannot take the
/// slots of the other users.
pub const MAX_PER_UID: usize = 6;
/// The time a client has to send its whole request.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(5);
/// The largest user configuration that tackd reads.
const MAX_CONFIG: u64 = 1 << 20;
/// The most programs that tackd runs at once, and for one uid.
pub const MAX_RUNNING: usize = 1024;
pub const MAX_RUNNING_PER_UID: usize = 128;

/// Starts an allowed program. Linux: `start::Linux`.
pub trait Starter: Send + Sync {
    fn start(
        &self,
        peer: Peer,
        req: &Start,
        d: &Decision,
        cfg: &Tack,
        fds: Vec<OwnedFd>,
    ) -> Result<Box<dyn Program>, String>;
}

/// A program that tackd started.
pub trait Program: Send + Sync {
    fn pid(&self) -> u32;
    /// For the audit: the protection, the cgroup, dropped variables.
    fn note(&self) -> String;
    /// Wait for what happens next. `Exited` and `Killed` are the last.
    fn next(&self) -> Event;
    fn signal(&self, sig: i32, group: bool) -> Result<(), String>;
    /// Its level of protection and the uid it runs as.
    fn runs_as(&self) -> Option<super::protect::RunsAs> {
        None
    }
}

/// Where tackd finds a user's configuration.
pub trait Users: Send + Sync {
    fn config(&self, uid: u32) -> Result<Tack, String>;
}

/// The configuration under the home folder of the passwd entry.
pub struct Passwd;

/// The home folder of a uid, from the passwd database (thread-safe).
pub fn home_of(uid: u32) -> Option<PathBuf> {
    passwd(uid).map(|(_, home)| home)
}

/// The name and home folder of a uid, from the passwd database.
pub fn passwd(uid: u32) -> Option<(String, PathBuf)> {
    let mut buf = vec![0u8; 16 * 1024];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: pw, buf and out are valid for the call; the strings point
    // into buf, which lives until we copy them.
    let r = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pw,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut out,
        )
    };
    if r != 0 || out.is_null() || pw.pw_dir.is_null() || pw.pw_name.is_null() {
        return None;
    }
    // SAFETY: pw_dir and pw_name are NUL-terminated strings in buf.
    let (dir, name) = unsafe {
        (
            std::ffi::CStr::from_ptr(pw.pw_dir),
            std::ffi::CStr::from_ptr(pw.pw_name),
        )
    };
    use std::os::unix::ffi::OsStrExt;
    let p = PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes()));
    let name = name.to_string_lossy().into_owned();
    p.is_absolute().then_some((name, p))
}

/// The configuration file that tackd reads for a home folder.
pub fn config_file(home: &Path) -> PathBuf {
    home.join(".config/paddock/config.toml")
}

/// Read a user's configuration as root, safely: the file must be a regular
/// file that `uid` owns, so a link to a file of root or of another user is
/// refused before a byte of it is read (and before a parse error could
/// show a line of it).
pub fn read_config(file: &Path, uid: u32) -> Result<Tack, String> {
    let real = match std::fs::canonicalize(file) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Tack::default()),
        Err(e) => return Err(format!("cannot read {}: {e}", file.display())),
    };
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&real)
        .map_err(|e| format!("cannot open {}: {e}", file.display()))?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err(format!("{} is not a file", file.display()));
    }
    if m.uid() != uid {
        return Err(format!(
            "{} belongs to uid {}, not to you: tackd reads only a file that you own",
            file.display(),
            m.uid()
        ));
    }
    if m.len() > MAX_CONFIG {
        return Err(format!(
            "{} is larger than {MAX_CONFIG} bytes",
            file.display()
        ));
    }
    let mut text = String::new();
    f.take(MAX_CONFIG)
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    config::parse(&text).map_err(|e| format!("[tack] in {} is invalid: {e:#}", file.display()))
}

impl Users for Passwd {
    fn config(&self, uid: u32) -> Result<Tack, String> {
        let home = home_of(uid).ok_or_else(|| format!("uid {uid} has no home folder"))?;
        read_config(&config_file(&home), uid)
    }
}

#[derive(Default)]
struct Counters {
    requests: AtomicU64,
    allowed: AtomicU64,
    denied: AtomicU64,
    errors: AtomicU64,
    started: AtomicU64,
}

pub struct Server {
    pub table: Box<dyn Table + Send + Sync>,
    pub users: Box<dyn Users>,
    pub audit: Log,
    pub cache: Arc<HashCache>,
    /// None: this tackd decides, and starts nothing (macOS for now).
    pub starter: Option<Box<dyn Starter>>,
    pub agents: Vec<String>,
    pub signer: fn(&Path) -> Option<String>,
    /// The user that a helper account of `protect = "uid"` belongs to: a
    /// request from a program at `uid` is decided with its owner's
    /// configuration.
    pub owner_of: Box<dyn Fn(u32) -> Option<u32> + Send + Sync>,
    pub started: u64,
    /// The time a client has to send its request.
    pub deadline: Duration,
    pub max_conns: usize,
    pub max_per_uid: usize,
    counts: Counters,
    conns: AtomicUsize,
    per_uid: std::sync::Mutex<std::collections::HashMap<u32, usize>>,
    running: std::sync::Mutex<std::collections::HashMap<u32, usize>>,
    pub max_running: usize,
    pub max_running_per_uid: usize,
}

/// What a request comes to.
enum Handled {
    Answer(Answer),
    /// A start request, decided; the configuration it was decided with.
    Start(Box<Start>, Decision, Option<Tack>),
}

/// A program's place in the count of running programs, given back on drop.
struct RunSlot<'a>(&'a Server, u32);

impl Drop for RunSlot<'_> {
    fn drop(&mut self) {
        let mut m = self.0.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.1);
            }
        }
    }
}

/// One connection's place in the count of its uid, given back on drop.
struct UidSlot<'a>(&'a Server, u32);

impl Drop for UidSlot<'_> {
    fn drop(&mut self) {
        let mut m = self.0.per_uid.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.1);
            }
        }
    }
}

impl Server {
    pub fn new(table: Box<dyn Table + Send + Sync>, users: Box<dyn Users>, audit: Log) -> Self {
        Server {
            table,
            users,
            audit,
            cache: Arc::new(HashCache::default()),
            starter: None,
            agents: procs::agent_names(),
            signer: ident::signer,
            owner_of: Box::new(|_| None),
            started: crate::state::now(),
            deadline: FRAME_DEADLINE,
            max_conns: MAX_CONNS,
            max_per_uid: MAX_PER_UID,
            counts: Counters::default(),
            conns: AtomicUsize::new(0),
            per_uid: Default::default(),
            running: Default::default(),
            max_running: MAX_RUNNING,
            max_running_per_uid: MAX_RUNNING_PER_UID,
        }
    }

    /// Decide a launch request and put it on record.
    fn launch(&self, peer: Peer, alive: &dyn Fn() -> bool, req: &proto::Launch) -> Decision {
        self.decide(peer, alive, req).0
    }

    /// Decide, put it on record, and keep the configuration for a start.
    fn decide(
        &self,
        peer: Peer,
        alive: &dyn Fn() -> bool,
        req: &proto::Launch,
    ) -> (Decision, Option<Tack>) {
        let owner = self.owner(peer.uid);
        let cfg = self.users.config(owner);
        let chain = procs::chain(self.table.as_ref(), peer.pid, &self.agents);
        let path = Path::new(&req.program);
        let ident = match &cfg {
            Ok(c) if peer.uid != 0 && policy::listed(c, path) => {
                ident::identify(path, owner, &self.cache)
            }
            _ => Err(ident::IdentError::NotFound),
        };
        let signer = |p: &Path| (self.signer)(p);
        let mut d = policy::decide(&Input {
            peer,
            req,
            cfg: cfg.as_ref().map_err(Clone::clone),
            chain: chain.as_ref(),
            ident: ident.as_ref().map_err(Clone::clone),
            signer: &signer,
        });
        // A pid names the same process only while it lives.
        if d.verdict != Verdict::Deny && !alive() {
            d = refuse(d, "the calling process ended while tackd read it".into());
        }
        let rec = self.record(peer, &req.program, &d);
        if let Err(e) = self.audit.write(&rec) {
            eprintln!(
                "tackd: cannot write the audit log {}: {e}",
                self.audit.path().display()
            );
            if d.verdict != Verdict::Deny {
                d = refuse(
                    d,
                    format!("tackd cannot write its audit log ({e}), so it allows nothing"),
                );
            }
        }
        match d.verdict {
            Verdict::Allow => self.counts.allowed.fetch_add(1, Ordering::Relaxed),
            _ => self.counts.denied.fetch_add(1, Ordering::Relaxed),
        };
        (d, cfg.ok())
    }

    /// The uid whose configuration decides for `uid`: its owner when it is
    /// a helper account, itself otherwise.
    fn owner(&self, uid: u32) -> u32 {
        (self.owner_of)(uid).filter(|o| *o != 0).unwrap_or(uid)
    }

    /// The audit record of a decision: under the owner's uid, so that the
    /// owner reads it, with the helper as the caller.
    fn record(&self, peer: Peer, program: &str, d: &Decision) -> Record {
        let mut rec = Record::from_decision(crate::state::now(), peer, program, d);
        let owner = self.owner(peer.uid);
        if owner != peer.uid {
            rec.uid = owner;
            rec.caller_uid = Some(peer.uid);
        }
        rec
    }

    /// An audit line about a start: `event` is started, ended or failed.
    fn audit_start(
        &self,
        peer: Peer,
        program: &str,
        d: &Decision,
        event: &str,
        reason: String,
        runs_as: Option<&super::protect::RunsAs>,
    ) {
        let mut rec = self.record(peer, program, d);
        rec.decision = event.to_string();
        rec.reason = reason;
        if let Some(r) = runs_as {
            rec.protect = Some(r.level.name().to_string());
            rec.run_uid = Some(r.uid);
        }
        if let Err(e) = self.audit.write(&rec) {
            eprintln!(
                "tackd: cannot write the audit log {}: {e}",
                self.audit.path().display()
            );
        }
    }

    pub fn status(&self) -> Status {
        let audit_error = self.audit.check().err().map(|e| e.to_string());
        Status {
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocols: vec![proto::VERSION],
            pid: std::process::id(),
            started: self.started,
            requests: self.counts.requests.load(Ordering::Relaxed),
            allowed: self.counts.allowed.load(Ordering::Relaxed),
            denied: self.counts.denied.load(Ordering::Relaxed),
            errors: self.counts.errors.load(Ordering::Relaxed),
            started_programs: self.counts.started.load(Ordering::Relaxed),
            running: self
                .running
                .lock()
                .map(|m| m.values().sum::<usize>() as u64)
                .unwrap_or(0),
            audit: self.audit.path().display().to_string(),
            audit_error,
        }
    }

    /// The answer to one request frame from `peer`. A start request gets
    /// its decision here; only a connection can start a program.
    pub fn answer(&self, peer: Peer, alive: &dyn Fn() -> bool, body: &[u8]) -> Answer {
        match self.handle(peer, alive, body) {
            Handled::Answer(a) => a,
            Handled::Start(_, d, _) => Answer::Decision(d),
        }
    }

    fn handle(&self, peer: Peer, alive: &dyn Fn() -> bool, body: &[u8]) -> Handled {
        self.counts.requests.fetch_add(1, Ordering::Relaxed);
        let req = match proto::parse_request(body) {
            Ok(r) => r,
            Err(e) => {
                self.counts.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "tackd: refused a request of uid {} pid {}: {e}",
                    peer.uid, peer.pid
                );
                return Handled::Answer(Answer::error(e));
            }
        };
        Handled::Answer(match req {
            Request::Launch(l) => Answer::Decision(self.launch(peer, alive, &l)),
            Request::Status {} => Answer::Status(self.status()),
            Request::Audit { lines } => Answer::Audit {
                lines: self
                    .audit
                    .read((peer.uid != 0).then(|| self.owner(peer.uid)), lines),
            },
            Request::Start(s) => {
                let (d, cfg) = self.decide(peer, alive, &s.launch());
                return Handled::Start(Box::new(s), d, cfg);
            }
        })
    }

    /// Serve one connection: one request, one answer; for a start that
    /// runs, the program to stay with.
    fn serve_conn(&self, conn: UnixStream) -> io::Result<Option<Session>> {
        conn.set_write_timeout(Some(self.deadline))?;
        let cred = match peer::of(&conn) {
            Ok(c) => c,
            Err(e) => {
                self.counts.errors.fetch_add(1, Ordering::Relaxed);
                let mut w = &conn;
                let _ = proto::write_frame(
                    &mut w,
                    &proto::envelope(Answer::error(format!("tackd cannot tell who you are: {e}"))),
                );
                return Err(e);
            }
        };
        let _slot = {
            let mut m = self.per_uid.lock().unwrap_or_else(|e| e.into_inner());
            let n = m.entry(cred.uid).or_insert(0);
            if *n >= self.max_per_uid {
                drop(m);
                self.counts.errors.fetch_add(1, Ordering::Relaxed);
                let mut w = &conn;
                return proto::write_frame(
                    &mut w,
                    &proto::envelope(Answer::error(format!(
                        "tackd is busy with {} requests of your user: try again",
                        self.max_per_uid
                    ))),
                )
                .map(|_| None);
            }
            *n += 1;
            UidSlot(self, cred.uid)
        };
        let answer = match read_request(&conn, self.deadline) {
            Ok((body, fds)) => match self.handle(cred.peer(), &|| cred.alive(), &body) {
                Handled::Answer(a) => a,
                Handled::Start(req, d, _) if d.verdict != Verdict::Allow => {
                    drop(fds);
                    let _ = req;
                    Answer::Decision(d)
                }
                Handled::Start(req, d, cfg) => {
                    let peer = cred.peer();
                    let p = self.start(&conn, peer, &req, d.clone(), cfg, fds)?;
                    return Ok(p.map(|p| Session {
                        conn,
                        peer,
                        req,
                        decision: d,
                        program: p,
                    }));
                }
            },
            Err(e) => {
                self.counts.errors.fetch_add(1, Ordering::Relaxed);
                let msg = match e.kind() {
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                        format!("the request did not arrive within {:?}", self.deadline)
                    }
                    _ => format!("bad request frame: {e}"),
                };
                Answer::error(msg)
            }
        };
        let mut w = &conn;
        proto::write_frame(&mut w, &proto::envelope(answer)).map(|_| None)
    }

    /// Accept connections until the listener fails.
    pub fn serve(self: &Arc<Self>, l: UnixListener) -> io::Result<()> {
        loop {
            let (conn, _) = match l.accept() {
                Ok(c) => c,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // Out of descriptors: wait instead of spinning.
                Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE)) => {
                    eprintln!("tackd: accept: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                Err(e) => return Err(e),
            };
            if self.conns.fetch_add(1, Ordering::SeqCst) >= self.max_conns {
                self.conns.fetch_sub(1, Ordering::SeqCst);
                self.counts.errors.fetch_add(1, Ordering::Relaxed);
                let _ = conn.set_write_timeout(Some(Duration::from_millis(200)));
                let mut w = &conn;
                let _ = proto::write_frame(
                    &mut w,
                    &proto::envelope(Answer::error("tackd is busy: try again")),
                );
                continue;
            }
            let me = self.clone();
            let spawned = std::thread::Builder::new()
                .name("tackd-conn".into())
                .spawn(move || {
                    let r = me.serve_conn(conn);
                    me.conns.fetch_sub(1, Ordering::SeqCst);
                    match r {
                        Ok(Some(s)) => me.supervise(s.conn, s.peer, &s.req, &s.decision, s.program),
                        Ok(None) => {}
                        Err(e) => log::debug!("tackd connection: {e}"),
                    }
                });
            if let Err(e) = spawned {
                self.conns.fetch_sub(1, Ordering::SeqCst);
                eprintln!("tackd: cannot start a thread: {e}");
            }
        }
    }
}

/// A started program and its connection, for `supervise`.
struct Session {
    conn: UnixStream,
    peer: Peer,
    req: Box<Start>,
    decision: Decision,
    program: Box<dyn Program>,
}

/// Read one request frame within `deadline`, with the descriptors that
/// came on its first bytes.
fn read_request(conn: &UnixStream, deadline: Duration) -> io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    let end = Instant::now() + deadline;
    conn.set_read_timeout(Some(deadline))?;
    let mut len = [0u8; 4];
    let (mut got, fds) = super::fdpass::recv(conn, &mut len, super::fdpass::MAX_FDS)?;
    if got == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    let mut r = Deadline { s: conn, end };
    while got < 4 {
        let n = r.read(&mut len[got..])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        got += n;
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > proto::MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a message of {n} bytes; the limit is {}", proto::MAX_FRAME),
        ));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    Ok((body, fds))
}

fn refuse(mut d: Decision, reason: String) -> Decision {
    d.verdict = Verdict::Deny;
    d.reason = reason;
    d.secrets.clear();
    d.hosts.clear();
    d
}

/// A reader with one deadline for all its reads: a client that sends one
/// byte at a time cannot stretch the time it holds a thread.
struct Deadline<'a> {
    s: &'a UnixStream,
    end: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.s.set_read_timeout(Some(left))?;
        let mut s = self.s;
        s.read(buf)
    }
}

/// The socket folder and socket, made safe: the folder is root's (or the
/// owner's in tests) and only it writes there; a stale socket of a tackd
/// that ended is removed; a running tackd is not displaced.
pub fn bind(path: &Path, owner: u32) -> io::Result<UnixListener> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let m = std::fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.uid() != owner {
        return Err(io::Error::other(format!(
            "{} is not a folder of uid {owner}: remove it and start tackd again",
            dir.display()
        )));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    if let Ok(m) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::FileTypeExt;
        if !m.file_type().is_socket() {
            return Err(io::Error::other(format!(
                "{} exists and is not a socket",
                path.display()
            )));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another tackd answers on {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    // Bind under a private name, open it up, then rename into place: the
    // socket is never reachable with the wrong mode.
    let tmp = dir.join(format!(".tackd-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let l = UnixListener::bind(&tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o666))?;
    std::fs::rename(&tmp, path)?;
    Ok(l)
}

/// `paddock tack serve`: the service's main loop.
pub fn main(paths: &super::service::Paths) -> anyhow::Result<i32> {
    // SAFETY: geteuid and umask cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("tackd runs as root, started by the system service (sudo paddock tack setup installs it)");
    }
    unsafe { libc::umask(0o077) };
    let audit = Log::new(paths.audit.clone(), 0);
    if let Err(e) = audit.check() {
        eprintln!(
            "tackd: the audit log {} cannot be written ({e}): every request is denied until it can",
            paths.audit.display()
        );
    }
    let mut server = Server::new(Box::new(procs::Kernel::new()), Box::new(Passwd), audit);
    let state = PathBuf::from(super::STATE_DIR);
    server.owner_of = Box::new(move |uid| {
        let r = super::protect::Registry::load(&state, 0).ok()?;
        let h = r.of_helper(uid)?;
        super::accounts::check(h).ok().map(|_| h.owner)
    });
    #[cfg(target_os = "linux")]
    let server = {
        let mut server = server;
        let mut l = super::start::Linux::new(Box::new(super::start::Stores));
        l.cache = server.cache.clone();
        server.starter = Some(Box::new(l));
        server
    };
    let server = Arc::new(server);
    let l = bind(&paths.socket, 0)?;
    eprintln!(
        "tackd {} started: pid {}, socket {}, audit {}, protocol {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        paths.socket.display(),
        paths.audit.display(),
        proto::VERSION
    );
    server.serve(l)?;
    Ok(1)
}

#[path = "daemon_run.rs"]
mod run;

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
