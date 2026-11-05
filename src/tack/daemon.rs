//! tackd: the root service that decides which program receives which
//! secret. It starts nothing yet: it answers launch requests and writes the
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

use super::audit::{Log, Record};
use super::config::{self, Tack};
use super::ident::{self, HashCache};
use super::peer;
use super::policy::{self, Input, Peer};
use super::procs::{self, Table};
use super::proto::{self, Answer, Decision, Request, Status, Verdict};
use std::io::{self, Read};
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

/// Where tackd finds a user's configuration.
pub trait Users: Send + Sync {
    fn config(&self, uid: u32) -> Result<Tack, String>;
}

/// The configuration under the home folder of the passwd entry.
pub struct Passwd;

/// The home folder of a uid, from the passwd database (thread-safe).
pub fn home_of(uid: u32) -> Option<PathBuf> {
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
    if r != 0 || out.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir is a NUL-terminated string in buf.
    let dir = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) };
    use std::os::unix::ffi::OsStrExt;
    let p = PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes()));
    p.is_absolute().then_some(p)
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
}

pub struct Server {
    pub table: Box<dyn Table + Send + Sync>,
    pub users: Box<dyn Users>,
    pub audit: Log,
    pub cache: HashCache,
    pub agents: Vec<String>,
    pub signer: fn(&Path) -> Option<String>,
    pub started: u64,
    /// The time a client has to send its request.
    pub deadline: Duration,
    pub max_conns: usize,
    pub max_per_uid: usize,
    counts: Counters,
    conns: AtomicUsize,
    per_uid: std::sync::Mutex<std::collections::HashMap<u32, usize>>,
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
            cache: HashCache::default(),
            agents: procs::agent_names(),
            signer: ident::signer,
            started: crate::state::now(),
            deadline: FRAME_DEADLINE,
            max_conns: MAX_CONNS,
            max_per_uid: MAX_PER_UID,
            counts: Counters::default(),
            conns: AtomicUsize::new(0),
            per_uid: Default::default(),
        }
    }

    /// Decide a launch request and put it on record.
    fn launch(&self, peer: Peer, alive: &dyn Fn() -> bool, req: &proto::Launch) -> Decision {
        let cfg = self.users.config(peer.uid);
        let chain = procs::chain(self.table.as_ref(), peer.pid, &self.agents);
        let path = Path::new(&req.program);
        let ident = match &cfg {
            Ok(c) if peer.uid != 0 && policy::listed(c, path) => {
                ident::identify(path, peer.uid, &self.cache)
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
        let rec = Record::from_decision(crate::state::now(), peer, &req.program, &d);
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
        d
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
            audit: self.audit.path().display().to_string(),
            audit_error,
        }
    }

    /// The answer to one request frame from `peer`.
    pub fn answer(&self, peer: Peer, alive: &dyn Fn() -> bool, body: &[u8]) -> Answer {
        self.counts.requests.fetch_add(1, Ordering::Relaxed);
        let req = match proto::parse_request(body) {
            Ok(r) => r,
            Err(e) => {
                self.counts.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "tackd: refused a request of uid {} pid {}: {e}",
                    peer.uid, peer.pid
                );
                return Answer::error(e);
            }
        };
        match req {
            Request::Launch(l) => Answer::Decision(self.launch(peer, alive, &l)),
            Request::Status {} => Answer::Status(self.status()),
            Request::Audit { lines } => Answer::Audit {
                lines: self.audit.read((peer.uid != 0).then_some(peer.uid), lines),
            },
        }
    }

    /// Serve one connection: one request, one answer.
    fn serve_conn(&self, conn: UnixStream) -> io::Result<()> {
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
                );
            }
            *n += 1;
            UidSlot(self, cred.uid)
        };
        let mut r = Deadline::new(&conn, self.deadline);
        let answer = match proto::read_frame(&mut r) {
            Ok(body) => self.answer(cred.peer(), &|| cred.alive(), &body),
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
        proto::write_frame(&mut w, &proto::envelope(answer))
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
                    if let Err(e) = me.serve_conn(conn) {
                        log::debug!("tackd connection: {e}");
                    }
                    me.conns.fetch_sub(1, Ordering::SeqCst);
                });
            if let Err(e) = spawned {
                self.conns.fetch_sub(1, Ordering::SeqCst);
                eprintln!("tackd: cannot start a thread: {e}");
            }
        }
    }
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

impl<'a> Deadline<'a> {
    fn new(s: &'a UnixStream, d: Duration) -> Self {
        Deadline {
            s,
            end: Instant::now() + d,
        }
    }
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
    let server = Arc::new(Server::new(
        Box::new(procs::Kernel::new()),
        Box::new(Passwd),
        audit,
    ));
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

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
