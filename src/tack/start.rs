//! tackd starts an allowed program (Linux).
//!
//! The program runs as the calling user: that user's id, the caller's own
//! groups (from the kernel, never more than the caller has), the caller's
//! umask, resource limits, nice value and cgroup, in the caller's working
//! folder, with the caller's standard streams (the descriptors that came
//! with the request) and the caller's environment plus the real values.
//! It is a process group of its own in tackd's session, which has no
//! controlling terminal: it reads and writes the caller's terminal through
//! the descriptors, and the stand-in passes on the signals that the
//! terminal sends.
//!
//! tackd runs the file that it identified, not a path that could change
//! in between: it opens the real file, checks that it is the same file
//! (device, inode) with the same content (SHA-256), and starts that open
//! file (`execveat`). A script (`#!`) needs its path for its interpreter,
//! so a script starts by its real path.
//!
//! Everything that the new process does before it becomes the program is
//! prepared in tackd first: the child only makes system calls that are
//! safe after `fork` in a process with threads.
//!
//! At `protect = "uid"` the program runs as the user's helper account
//! instead, with only the helper's group, the helper's home, the exec
//! wrapper first on PATH and an ACL on the working folder
//! (`start_uid`, `protect`). Everything else above stays the caller's.

use super::caller::{self, Caller};
use super::config::Tack;
use super::daemon::{Program, Starter};
use super::ident::HashCache;
use super::policy::Peer;
use super::proto::{Decision, Event, Start};
use crate::credentials::secret::Secret;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// Where tackd gets the values: the user's store, run as that user.
pub trait Values: Send + Sync {
    fn fetch(
        &self,
        who: &Who,
        cfg: &Tack,
        names: &[String],
    ) -> Result<Vec<(String, Secret)>, String>;
}

/// The caller, for the store tools.
pub struct Who<'a> {
    pub caller: &'a Caller,
    pub home: PathBuf,
    pub user: String,
    /// The caller's environment, without the variables that load code.
    pub env: Vec<(String, String)>,
    pub drop_user: bool,
}

/// The starter of tackd.
pub struct Linux {
    pub values: Box<dyn Values>,
    pub cache: Arc<HashCache>,
    /// `/proc`, or a fake one in tests.
    pub proc: PathBuf,
    /// `/sys/fs/cgroup`.
    pub cgroups: PathBuf,
    /// `protect = "uid"`: the helpers, the wrapper and the ACLs.
    pub protect: super::start_uid::Protection,
}

impl Linux {
    pub fn new(values: Box<dyn Values>) -> Self {
        Linux {
            values,
            cache: Arc::new(HashCache::default()),
            proc: PathBuf::from("/proc"),
            cgroups: PathBuf::from("/sys/fs/cgroup"),
            protect: super::start_uid::Protection::system(PathBuf::from(super::STATE_DIR)),
        }
    }
}

/// Everything that the child needs, made before fork.
struct Exec {
    fd: OwnedFd,
    script: bool,
    path: CString,
    _argv: Vec<CString>,
    argv: Vec<*const libc::c_char>,
    _env: Vec<Secret>,
    envp: Vec<*const libc::c_char>,
    cwd: CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
    groups: Vec<libc::gid_t>,
    umask: libc::mode_t,
    limits: Vec<(i32, libc::rlimit)>,
    nice: Option<i32>,
    cgroup: Option<OwnedFd>,
    drop_user: bool,
}

// SAFETY: the raw pointers point into the CStrings and Secrets of the same
// struct, which never change after it is built.
unsafe impl Send for Exec {}
unsafe impl Sync for Exec {}

/// The child side, after fork: async-signal-safe calls only.
///
/// # Safety
/// Only in the child of a fork, before exec.
unsafe fn become_program(x: &Exec) -> io::Result<()> {
    let fail = || Err(io::Error::last_os_error());
    // A process group of its own, as a shell job has: the stand-in sends
    // the terminal's signals to it. Not a session of its own: a session
    // leader takes the first terminal it opens as its controlling
    // terminal, and gets SIGHUP when that terminal closes.
    if libc::setpgid(0, 0) < 0 {
        return fail();
    }
    if let Some(fd) = &x.cgroup {
        // "0": move the writing process. A failure leaves it in tackd's
        // cgroup; tackd checks and records where it ended up.
        let _ = libc::write(fd.as_raw_fd(), b"0".as_ptr() as *const libc::c_void, 1);
    }
    for (r, l) in &x.limits {
        let _ = libc::setrlimit(*r as _, l);
    }
    if let Some(n) = x.nice {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, n);
    }
    if x.drop_user {
        if libc::setgroups(x.groups.len() as _, x.groups.as_ptr()) != 0 {
            return fail();
        }
        if libc::setresgid(x.gid, x.gid, x.gid) != 0 {
            return fail();
        }
        if libc::setresuid(x.uid, x.uid, x.uid) != 0 {
            return fail();
        }
        if libc::getuid() != x.uid || libc::geteuid() != x.uid || libc::getegid() != x.gid {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
    }
    // The user must reach the program by its path: tackd opened it as
    // root, and a file in a folder that the user cannot enter is not one
    // that the user may run (or read through /proc/self/exe).
    if libc::access(x.path.as_ptr(), libc::X_OK) != 0 {
        return fail();
    }
    if libc::chdir(x.cwd.as_ptr()) != 0 {
        return fail();
    }
    libc::umask(x.umask);
    for sig in 1..65 {
        if sig != libc::SIGKILL && sig != libc::SIGSTOP {
            libc::signal(sig, libc::SIG_DFL);
        }
    }
    // Every descriptor of tackd closes at exec (they are close-on-exec
    // already; this is the second lock). 4: CLOSE_RANGE_CLOEXEC.
    let _ = libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32);
    if x.script {
        libc::execve(x.path.as_ptr(), x.argv.as_ptr(), x.envp.as_ptr());
    } else {
        libc::syscall(
            libc::SYS_execveat,
            x.fd.as_raw_fd(),
            c"".as_ptr(),
            x.argv.as_ptr(),
            x.envp.as_ptr(),
            libc::AT_EMPTY_PATH,
        );
    }
    fail()
}

fn cstr(s: &str, what: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("{what} holds a NUL byte"))
}

/// Open the program that the decision is about and check that it is still
/// that file, with that content.
pub fn open_program(d: &Decision, cache: &HashCache) -> Result<(OwnedFd, bool), String> {
    use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
    let p = d
        .program
        .as_ref()
        .ok_or("the decision names no program file")?;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&p.real)
        .map_err(|e| format!("cannot open {}: {e}", p.real))?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() || m.dev() != p.dev || m.ino() != p.ino {
        return Err(format!(
            "{} was replaced after tackd checked it: start it again",
            p.real
        ));
    }
    let id = super::ident::identify_open(&f, &m, cache).map_err(|e| e.to_string())?;
    if id != p.sha256 {
        return Err(format!(
            "{} changed after tackd checked it: start it again",
            p.real
        ));
    }
    let mut head = [0u8; 2];
    let script = f.read_at(&mut head, 0).map(|n| n == 2).unwrap_or(false) && &head == b"#!";
    Ok((OwnedFd::from(f), script))
}

/// The cgroup.procs file of the caller's cgroup, opened by tackd.
fn cgroup_file(root: &Path, c: &Caller) -> Option<OwnedFd> {
    let rel = c.cgroup.as_deref()?.trim_start_matches('/');
    let p = root.join(rel).join("cgroup.procs");
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(p)
        .ok()
        .map(OwnedFd::from)
}

/// The caller's limits, clipped to what tackd may set.
fn limits(c: &Caller) -> Vec<(i32, libc::rlimit)> {
    let conv = |v: Option<u64>| v.unwrap_or(libc::RLIM_INFINITY);
    c.limits
        .iter()
        .filter_map(|l| {
            let mut own: libc::rlimit = unsafe { std::mem::zeroed() };
            // SAFETY: own is an rlimit buffer.
            if unsafe { libc::getrlimit(l.resource as _, &mut own) } != 0 {
                return None;
            }
            let hard = conv(l.hard).min(own.rlim_max);
            let soft = conv(l.soft).min(hard);
            Some((
                l.resource,
                libc::rlimit {
                    rlim_cur: soft,
                    rlim_max: hard,
                },
            ))
        })
        .collect()
}

/// The user name and home of a uid.
fn passwd(uid: u32) -> (String, PathBuf) {
    super::daemon::passwd(uid).unwrap_or_else(|| (uid.to_string(), PathBuf::from("/")))
}

/// The owner of a helper, as the store tools run: the owner's uid and
/// group from the passwd database, with the caller's umask and limits.
fn owner_caller(owner: u32, helper: &Caller) -> Caller {
    let gid = super::accounts::by_uid(owner)
        .map(|p| p.gid)
        .unwrap_or(owner);
    Caller {
        uid: owner,
        gid,
        groups: vec![gid],
        cgroup: None,
        ..helper.clone()
    }
}

/// Whether the child must change its user: always as root; a test that
/// runs as the caller's own user cannot and need not.
fn must_drop(uid: u32) -> bool {
    // SAFETY: geteuid cannot fail.
    let me = unsafe { libc::geteuid() };
    me == 0 || me != uid
}

/// The caller's environment as pairs, without the variables that load code.
fn pairs(env: &[String]) -> Vec<(String, String)> {
    env.iter()
        .filter_map(|e| e.split_once('='))
        .filter(|(k, _)| !k.is_empty() && !super::launchenv::dropped(k))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

impl Starter for Linux {
    fn start(
        &self,
        peer: Peer,
        req: &Start,
        d: &Decision,
        cfg: &Tack,
        fds: Vec<OwnedFd>,
    ) -> Result<Box<dyn Program>, String> {
        if peer.uid == 0 {
            return Err("tackd starts nothing as root".into());
        }
        let c = caller::read(&self.proc, peer.pid, peer.uid)?;
        let reg = self.protect.registry();
        // A program at protect = uid that starts an allowlisted program:
        // the store and the configuration are its owner's.
        let owner = reg
            .as_ref()
            .ok()
            .and_then(|r| r.of_helper(peer.uid))
            .map(|h| h.owner);
        let store_caller = match owner {
            Some(o) => owner_caller(o, &c),
            None => c.clone(),
        };
        let (plan, notes) = self.protect.prepare(peer.uid, &c, req, d, cfg, &reg)?;
        let (user, home) = passwd(store_caller.uid);
        let who = Who {
            caller: &store_caller,
            home,
            user,
            env: pairs(&super::protect::unwrap_env(&req.env)),
            drop_user: must_drop(store_caller.uid),
        };
        let values = self.values.fetch(&who, cfg, &d.secrets)?;
        let (fd, script) = open_program(d, &self.cache)?;
        let real = &d.program.as_ref().expect("checked by open_program").real;
        let (env, dropped) = super::launchenv::build(&plan.env, &values);
        drop(values);
        let mut argv = vec![cstr(&req.argv0, "argv0")?];
        for a in &req.args {
            argv.push(cstr(a, "an argument")?);
        }
        let mut argp: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
        argp.push(std::ptr::null());
        let mut envp: Vec<*const libc::c_char> = env
            .iter()
            .map(|e| e.as_bytes().as_ptr() as *const libc::c_char)
            .collect();
        envp.push(std::ptr::null());
        let x = Arc::new(Exec {
            fd,
            script,
            path: cstr(real, "the program path")?,
            _argv: argv,
            argv: argp,
            _env: env,
            envp,
            cwd: cstr(&req.cwd, "the working folder")?,
            uid: plan.uid,
            gid: plan.gid,
            groups: plan.groups.clone(),
            umask: c.umask as libc::mode_t,
            limits: limits(&c),
            nice: c.nice,
            cgroup: cgroup_file(&self.cgroups, &c),
            drop_user: must_drop(plan.uid),
        });
        let mut fds = fds.into_iter();
        let mut stdio = |present: bool| -> Stdio {
            match present.then(|| fds.next()).flatten() {
                Some(fd) => Stdio::from(fd),
                None => Stdio::null(),
            }
        };
        let mut cmd = Command::new(real);
        cmd.env_clear()
            .stdin(stdio(req.stdio[0]))
            .stdout(stdio(req.stdio[1]))
            .stderr(stdio(req.stdio[2]));
        let xc = x.clone();
        // SAFETY: become_program makes async-signal-safe calls only, on
        // memory that was prepared before fork.
        unsafe {
            cmd.pre_exec(move || become_program(&xc));
        }
        let at_uid = plan.level == super::protect::Level::Uid;
        let child = crate::env::spawn_retrying(&mut cmd).map_err(|e| {
            format!(
                "cannot start {real}{}: {e}",
                match e.raw_os_error() {
                    Some(libc::ENOENT) => " (the working folder or the program's interpreter is missing)",
                    Some(libc::EACCES) if at_uid => " as your helper account (protect = \"uid\"): the helper must be able to run the program and enter the working folder. Put the program where every user can run it, such as /usr/local/bin, or set protect = \"refs\" in its entry",
                    _ => "",
                }
            )
        })?;
        // The values in tackd's memory are wiped now (the child has its
        // own copy, in the program).
        drop(cmd);
        drop(x);
        let pid = child.id();
        let cgroup = std::fs::read_to_string(self.proc.join(pid.to_string()).join("cgroup"))
            .ok()
            .and_then(|t| caller::parse_cgroup(&t));
        let runs_as = super::protect::RunsAs {
            level: plan.level,
            uid: plan.uid,
            text: super::protect::describe(&plan),
        };
        let mut note = vec![runs_as.text.clone()];
        note.extend(notes);
        note.push(match (&c.cgroup, &cgroup) {
            (Some(want), Some(got)) if want == got => "in the caller's cgroup".into(),
            (Some(_), _) => "not in the caller's cgroup (tackd's instead)".into(),
            (None, _) => "no cgroup v2".into(),
        });
        if script {
            note.push("a script, started by its path".into());
        }
        if !dropped.is_empty() {
            note.push(format!("dropped {}", dropped.join(" ")));
        }
        Ok(Box::new(Child {
            pid: pid as libc::pid_t,
            reaped: Mutex::new(false),
            note: note.join("; "),
            runs_as: Some(runs_as),
        }))
    }
}

/// A program that tackd started and has not reaped yet.
pub struct Child {
    pid: libc::pid_t,
    /// Set, under the lock, when tackd reaps the program: until then its
    /// pid (and process group) cannot name another process, so a signal
    /// sent under the lock reaches the program or nothing.
    reaped: Mutex<bool>,
    note: String,
    runs_as: Option<super::protect::RunsAs>,
}

impl Child {
    /// A child that tackd did not start itself (tests).
    pub fn adopt(pid: u32) -> Self {
        Child {
            pid: pid as libc::pid_t,
            reaped: Mutex::new(false),
            note: String::new(),
            runs_as: None,
        }
    }

    fn wait_raw(&self, flags: libc::c_int) -> io::Result<libc::siginfo_t> {
        loop {
            // SAFETY: a zeroed siginfo_t is a valid out-parameter.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let r = unsafe { libc::waitid(libc::P_PID, self.pid as libc::id_t, &mut info, flags) };
            if r == 0 {
                return Ok(info);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

impl Program for Child {
    fn pid(&self) -> u32 {
        self.pid as u32
    }

    fn note(&self) -> String {
        self.note.clone()
    }

    fn runs_as(&self) -> Option<super::protect::RunsAs> {
        self.runs_as.clone()
    }

    fn next(&self) -> Event {
        loop {
            let flags = libc::WEXITED | libc::WSTOPPED | libc::WCONTINUED | libc::WNOWAIT;
            let info = match self.wait_raw(flags) {
                Ok(i) => i,
                Err(e) => {
                    // ECHILD: someone else reaped it; nothing more to know.
                    log::debug!("waitid {}: {e}", self.pid);
                    *self.reaped.lock().unwrap_or_else(|e| e.into_inner()) = true;
                    return Event::Exited { code: 127 };
                }
            };
            // SAFETY: waitid filled a SIGCHLD siginfo.
            let status = unsafe { info.si_status() };
            match info.si_code {
                libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED => {
                    let mut g = self.reaped.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = self.wait_raw(libc::WEXITED);
                    *g = true;
                    return match info.si_code {
                        libc::CLD_EXITED => Event::Exited { code: status },
                        c => Event::Killed {
                            signal: status,
                            core: c == libc::CLD_DUMPED,
                        },
                    };
                }
                libc::CLD_STOPPED | libc::CLD_TRAPPED => {
                    let _ = self.wait_raw(libc::WSTOPPED | libc::WNOHANG);
                    return Event::Stopped { signal: status };
                }
                libc::CLD_CONTINUED => {
                    let _ = self.wait_raw(libc::WCONTINUED | libc::WNOHANG);
                    return Event::Continued {};
                }
                _ => {
                    // Nothing pending after all; wait again.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    fn signal(&self, sig: i32, group: bool) -> Result<(), String> {
        let g = self.reaped.lock().unwrap_or_else(|e| e.into_inner());
        if *g {
            return Err("the program ended".into());
        }
        let target = if group { -self.pid } else { self.pid };
        // SAFETY: kill takes plain integers.
        if unsafe { libc::kill(target, sig) } != 0 {
            let e = io::Error::last_os_error();
            // ESRCH for a group: the program left its group; try it alone.
            if group
                && e.raw_os_error() == Some(libc::ESRCH)
                && unsafe { libc::kill(self.pid, sig) } == 0
            {
                return Ok(());
            }
            return Err(e.to_string());
        }
        Ok(())
    }
}

/// The real store: each secret's backend, its tools run as the caller.
pub struct Stores;

impl Values for Stores {
    fn fetch(
        &self,
        who: &Who,
        cfg: &Tack,
        names: &[String],
    ) -> Result<Vec<(String, Secret)>, String> {
        let mut vars: std::collections::BTreeMap<String, String> =
            who.env.iter().cloned().collect();
        let uid = who.caller.uid;
        let run = format!("/run/user/{uid}");
        if !vars.contains_key("XDG_RUNTIME_DIR") && Path::new(&run).is_dir() {
            vars.insert("XDG_RUNTIME_DIR".into(), run.clone());
        }
        if !vars.contains_key("DBUS_SESSION_BUS_ADDRESS") && Path::new(&run).join("bus").exists() {
            vars.insert(
                "DBUS_SESSION_BUS_ADDRESS".into(),
                format!("unix:path={run}/bus"),
            );
        }
        vars.insert("HOME".into(), who.home.display().to_string());
        vars.insert("USER".into(), who.user.clone());
        vars.insert("LOGNAME".into(), who.user.clone());
        let env = crate::env::HostEnv::from_parts(
            who.home.clone(),
            who.user.clone(),
            uid,
            who.caller.gid,
            vars.clone(),
        );
        let prepare = as_user(who, vars);
        let mut out = Vec::new();
        for n in names {
            let s = cfg
                .secrets
                .get(n)
                .ok_or_else(|| format!("{n} is not in your configuration"))?;
            let store =
                super::store::open_as(&env, s, prepare.clone()).map_err(|e| format!("{n}: {e}"))?;
            let v = store
                .get(n)
                .map_err(|e| format!("{n} ({}): {e}", store.describe()))?;
            out.push((n.clone(), v));
        }
        Ok(out)
    }
}

/// Run a store tool as the caller, in the caller's environment.
fn as_user(who: &Who, vars: std::collections::BTreeMap<String, String>) -> super::store::Prepare {
    let uid = who.caller.uid;
    let gid = who.caller.gid;
    let groups: Vec<libc::gid_t> = who.caller.groups.clone();
    let umask = who.caller.umask as libc::mode_t;
    let drop_user = who.drop_user;
    let home = CString::new(who.home.as_os_str().as_bytes()).unwrap_or_else(|_| c"/".into());
    Arc::new(move |cmd: &mut Command| {
        cmd.env_clear().envs(vars.clone());
        let groups = groups.clone();
        let home = home.clone();
        // SAFETY: async-signal-safe calls on memory made before fork.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if drop_user {
                    if libc::setgroups(groups.len() as _, groups.as_ptr()) != 0
                        || libc::setresgid(gid, gid, gid) != 0
                        || libc::setresuid(uid, uid, uid) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::geteuid() != uid {
                        return Err(io::Error::from_raw_os_error(libc::EPERM));
                    }
                }
                libc::umask(umask);
                if libc::chdir(home.as_ptr()) != 0 {
                    libc::chdir(c"/".as_ptr());
                }
                Ok(())
            });
        }
    })
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
