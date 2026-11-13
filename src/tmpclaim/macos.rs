//! macOS: stop a process of a run only when it runs in that run's sandbox.
//!
//! A pid file is written by the sandbox, so its pid can name any process.
//! `sandbox_check` asks the kernel what the Seatbelt profile of a process
//! allows. A process is this run's when it is sandboxed, can read the run
//! folder and write the run's TMPDIR (only this run's profile allows both),
//! and cannot list the host home (every paddock profile denies it).
//!
//! The end of a run asks this of every process of the user, so it finds the
//! server that left the process group (setsid) and the helpers it started,
//! by the sandbox they run in and never by a program name.

use super::Record;
use anyhow::{bail, Result};
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SANDBOX_FILTER_NONE: c_int = 0;
const SANDBOX_FILTER_PATH: c_int = 1;

extern "C" {
    // libsystem_sandbox, part of libSystem. 0: allowed (or, without an
    // operation, not sandboxed); 1: denied (or sandboxed).
    fn sandbox_check(pid: libc::pid_t, operation: *const c_char, kind: c_int, ...) -> c_int;
}

fn allowed(pid: libc::pid_t, op: &str, path: &Path) -> Result<bool> {
    let op = CString::new(op)?;
    let p = CString::new(path.as_os_str().as_encoded_bytes())?;
    // SAFETY: NUL-terminated strings that live across the call.
    let r = unsafe { sandbox_check(pid, op.as_ptr(), SANDBOX_FILTER_PATH, p.as_ptr()) };
    Ok(r == 0)
}

/// `SZOMB` of `sys/proc.h`: the process ended and waits for its parent.
const SZOMB: u32 = 5;

fn bsd(pid: u32) -> Option<libc::proc_bsdinfo> {
    // SAFETY: proc_bsdinfo is plain data; zeros are a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
    // SAFETY: the buffer is one proc_bsdinfo of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (n == size).then_some(info)
}

/// The processes of this user.
pub fn list() -> Vec<u32> {
    // SAFETY: geteuid has no arguments and cannot fail.
    let me = unsafe { libc::geteuid() };
    let mut v: Vec<u32> = crate::mark::procs::proc_table()
        .into_keys()
        .filter(|&pid| bsd(pid).is_some_and(|i| i.pbi_uid == me))
        .collect();
    v.sort_unstable();
    v
}

/// The process runs: a zombie does not count.
pub fn alive(pid: u32) -> bool {
    crate::state::alive(pid) && bsd(pid).is_none_or(|i| i.pbi_status != SZOMB)
}

fn sandboxed(pid: libc::pid_t) -> bool {
    // SAFETY: no operation: the call only asks whether pid is sandboxed.
    unsafe { sandbox_check(pid, std::ptr::null(), SANDBOX_FILTER_NONE) == 1 }
}

/// The process may list the host home: a host process, or a sandbox that
/// is not paddock's.
fn lists_host_home(pid: libc::pid_t) -> Result<bool> {
    match std::env::var_os("HOME") {
        Some(h) => allowed(pid, "file-read-data", Path::new(&h)),
        None => Ok(true),
    }
}

/// One line for `paddock tmp debug`: pid, parent, command, and what
/// the kernel tells about the sandbox of the process.
pub fn describe(pid: u32) -> String {
    let (ppid, comm) = bsd(pid)
        .map(|i| {
            let c: Vec<u8> = i
                .pbi_comm
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as u8)
                .collect();
            (i.pbi_ppid, String::from_utf8_lossy(&c).into_owned())
        })
        .unwrap_or_default();
    let p = pid as libc::pid_t;
    format!(
        "pid {pid} ppid {ppid} {comm:?} alive {} sandboxed {} lists_host_home {:?}",
        alive(pid),
        sandboxed(p),
        lists_host_home(p).map_err(|e| e.to_string())
    )
}

/// The two probes of `of_run` for `paddock tmp debug`.
pub fn probes(pid: u32, rec: &Record) -> String {
    let p = pid as libc::pid_t;
    format!(
        "read {:?} write {:?}",
        allowed(p, "file-read-data", &rec.probe_read).map_err(|e| e.to_string()),
        allowed(p, "file-write-data", &rec.probe_write).map_err(|e| e.to_string())
    )
}

/// Is `pid` a process of a paddock sandbox of the home `home`, whose run
/// folder is none of `keep`? An earlier run of this home left it.
pub fn of_home(pid: u32, home: &Path, keep: &[PathBuf]) -> Result<bool> {
    let pid = pid as libc::pid_t;
    if home.as_os_str().is_empty() || !sandboxed(pid) || lists_host_home(pid)? {
        return Ok(false);
    }
    if !allowed(pid, "file-write-data", home)? {
        return Ok(false);
    }
    for k in keep {
        if allowed(pid, "file-read-data", k)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Is `pid` a process of the sandbox of `rec`?
pub fn of_run(pid: u32, rec: &Record) -> Result<bool> {
    if rec.probe_read.as_os_str().is_empty() || rec.probe_write.as_os_str().is_empty() {
        bail!("the record has no probe paths");
    }
    let pid = pid as libc::pid_t;
    if !sandboxed(pid) {
        return Ok(false);
    }
    let lists_home = lists_host_home(pid)?;
    Ok(allowed(pid, "file-read-data", &rec.probe_read)?
        && allowed(pid, "file-write-data", &rec.probe_write)?
        && !lists_home)
}

/// One process: SIGTERM, then SIGKILL after 5 seconds. The release uses
/// `stop_where` of tmpclaim.rs for all processes of a run at once. A process of another sandbox or
/// of the host is left alone.
#[cfg_attr(not(test), allow(dead_code))]
pub fn stop(pid: u32, rec: &Record) -> Result<()> {
    if !crate::state::alive(pid) {
        return Ok(());
    }
    if !of_run(pid, rec)? {
        bail!("it does not run in this sandbox, so paddock leaves it alone");
    }
    // SAFETY: kill with a plain signal.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let t = Instant::now();
    while crate::state::alive(pid) && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if crate::state::alive(pid) {
        // SAFETY: kill with a plain signal.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }
    Ok(())
}
