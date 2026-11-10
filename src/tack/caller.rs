//! What tackd copies from the calling process onto the program it starts:
//! the user and its groups, the umask, the resource limits, the nice
//! value and the cgroup. All of it comes from the kernel (`/proc`), not
//! from the request, and it never gives the program more than the caller
//! has: the groups are the caller's own, not those of the group database.
//!
//! The parsers are plain functions, so that the tests run anywhere.

use std::path::Path;

/// One resource limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub resource: i32,
    /// `None`: unlimited.
    pub soft: Option<u64>,
    pub hard: Option<u64>,
}

/// The calling process, as tackd copies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    pub umask: u32,
    pub limits: Vec<Limit>,
    /// The cgroup (v2) path, such as `/user.slice/user-1000.slice/session-2.scope`.
    pub cgroup: Option<String>,
    pub nice: Option<i32>,
}

/// The user lines of `/proc/PID/status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub uids: [u32; 4],
    pub gids: [u32; 4],
    pub groups: Vec<u32>,
    pub umask: Option<u32>,
}

fn four(line: &str) -> Option<[u32; 4]> {
    let v: Vec<u32> = line
        .split_whitespace()
        .map(|w| w.parse().ok())
        .collect::<Option<_>>()?;
    v.try_into().ok()
}

pub fn parse_status(text: &str) -> Result<Status, String> {
    let mut uids = None;
    let mut gids = None;
    let mut groups = None;
    let mut umask = None;
    for l in text.lines() {
        if let Some(r) = l.strip_prefix("Uid:") {
            uids = four(r);
        } else if let Some(r) = l.strip_prefix("Gid:") {
            gids = four(r);
        } else if let Some(r) = l.strip_prefix("Groups:") {
            groups = r
                .split_whitespace()
                .map(|w| w.parse().ok())
                .collect::<Option<Vec<u32>>>();
        } else if let Some(r) = l.strip_prefix("Umask:") {
            umask = u32::from_str_radix(r.trim(), 8).ok();
        }
    }
    Ok(Status {
        uids: uids.ok_or("no Uid line")?,
        gids: gids.ok_or("no Gid line")?,
        groups: groups.ok_or("no Groups line")?,
        umask,
    })
}

/// The limits that tackd copies, by their name in `/proc/PID/limits`.
fn resources() -> Vec<(&'static str, i32)> {
    #[allow(clippy::unnecessary_cast)]
    let v = vec![
        ("Max cpu time", libc::RLIMIT_CPU as i32),
        ("Max file size", libc::RLIMIT_FSIZE as i32),
        ("Max data size", libc::RLIMIT_DATA as i32),
        ("Max stack size", libc::RLIMIT_STACK as i32),
        ("Max core file size", libc::RLIMIT_CORE as i32),
        ("Max resident set", libc::RLIMIT_RSS as i32),
        ("Max processes", libc::RLIMIT_NPROC as i32),
        ("Max open files", libc::RLIMIT_NOFILE as i32),
        ("Max locked memory", libc::RLIMIT_MEMLOCK as i32),
        ("Max address space", libc::RLIMIT_AS as i32),
    ];
    v
}

pub fn parse_limits(text: &str) -> Vec<Limit> {
    let value = |w: &str| -> Option<Option<u64>> {
        if w == "unlimited" {
            Some(None)
        } else {
            w.parse().ok().map(Some)
        }
    };
    let mut out = Vec::new();
    for l in text.lines() {
        for (name, resource) in resources() {
            let Some(rest) = l.strip_prefix(name) else {
                continue;
            };
            // "Max processes" must not take "Max processes something".
            if !rest.starts_with(' ') {
                continue;
            }
            let w: Vec<&str> = rest.split_whitespace().collect();
            if let (Some(Some(soft)), Some(Some(hard))) =
                (w.first().map(|w| value(w)), w.get(1).map(|w| value(w)))
            {
                out.push(Limit {
                    resource,
                    soft,
                    hard,
                });
            }
        }
    }
    out
}

/// The cgroup v2 path of `/proc/PID/cgroup` (`0::/path`). None on a
/// cgroup v1 host, or for a path that is not plain.
pub fn parse_cgroup(text: &str) -> Option<String> {
    let p = text.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let plain = p.starts_with('/')
        && !p.contains('\0')
        && !p.ends_with(" (deleted)")
        && p.split('/').all(|c| c != ".." && c != ".");
    plain.then(|| p.to_string())
}

/// Read the caller `pid` of `uid` under `proc` (`/proc`). The process must
/// run as `uid` in every one of its user ids, and with one group id: a
/// setuid or setgid program in the middle is refused.
pub fn read(proc: &Path, pid: u32, uid: u32) -> Result<Caller, String> {
    let d = proc.join(pid.to_string());
    let status = std::fs::read_to_string(d.join("status"))
        .map_err(|e| format!("cannot read the calling process: {e}"))?;
    let st = parse_status(&status)?;
    if st.uids.iter().any(|u| *u != uid) {
        return Err(format!(
            "the calling process has user ids {:?}, not {uid} in each: tackd starts nothing for a process that changed its user",
            st.uids
        ));
    }
    let limits = std::fs::read_to_string(d.join("limits"))
        .map(|t| parse_limits(&t))
        .unwrap_or_default();
    let cgroup = std::fs::read_to_string(d.join("cgroup"))
        .ok()
        .and_then(|t| parse_cgroup(&t));
    // SAFETY: getpriority takes plain integers; -1 is also a valid value,
    // so errno tells an error apart.
    let nice = unsafe {
        *errno() = 0;
        let n = libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t);
        (*errno() == 0).then_some(n)
    };
    if st.gids.iter().any(|g| *g != st.gids[0]) {
        return Err(format!(
            "the calling process has group ids {:?}: tackd starts nothing for a process that changed its group",
            st.gids
        ));
    }
    Ok(Caller {
        uid,
        gid: st.gids[0],
        groups: st.groups,
        umask: st.umask.unwrap_or(0o022),
        limits,
        cgroup,
        nice,
    })
}

#[cfg(target_os = "linux")]
unsafe fn errno() -> *mut libc::c_int {
    libc::__errno_location()
}

#[cfg(target_os = "macos")]
unsafe fn errno() -> *mut libc::c_int {
    libc::__error()
}

#[cfg(test)]
#[allow(clippy::unnecessary_cast)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tgh\nUmask:\t0027\nState:\tS (sleeping)\nUid:\t1000\t1000\t1000\t1000\nGid:\t100\t1000\t1000\t1000\nGroups:\t10 998 1000 \n";

    #[test]
    fn the_status_lines_give_users_groups_and_umask() {
        let s = parse_status(STATUS).unwrap();
        assert_eq!(s.uids, [1000; 4]);
        assert_eq!(s.gids, [100, 1000, 1000, 1000]);
        assert_eq!(s.groups, [10, 998, 1000]);
        assert_eq!(s.umask, Some(0o027));
        let empty = parse_status("Uid:\t1 1 1 1\nGid:\t1 1 1 1\nGroups:\t\n").unwrap();
        assert!(empty.groups.is_empty());
        assert!(parse_status("Uid:\t1 1 1\nGid:\t1 1 1 1\nGroups:\n").is_err());
        assert!(parse_status("Gid:\t1 1 1 1\n").is_err());
    }

    #[test]
    fn limits_are_read_by_name_with_unlimited() {
        let text = "Limit                     Soft Limit           Hard Limit           Units     \nMax cpu time              unlimited            unlimited            seconds   \nMax stack size            8388608              unlimited            bytes     \nMax open files            1024                 524288               files     \nMax pending signals       127448               127448               signals   \n";
        let l = parse_limits(text);
        assert_eq!(l.len(), 3);
        assert!(l.contains(&Limit {
            resource: libc::RLIMIT_NOFILE as i32,
            soft: Some(1024),
            hard: Some(524288)
        }));
        assert!(l.contains(&Limit {
            resource: libc::RLIMIT_CPU as i32,
            soft: None,
            hard: None
        }));
    }

    #[test]
    fn only_a_plain_v2_cgroup_path_is_taken() {
        assert_eq!(
            parse_cgroup("0::/user.slice/user-1000.slice/session-2.scope\n").as_deref(),
            Some("/user.slice/user-1000.slice/session-2.scope")
        );
        assert_eq!(parse_cgroup("12:cpu:/x\n"), None);
        assert_eq!(parse_cgroup("0::/a/../b\n"), None);
        assert_eq!(parse_cgroup("0::/a (deleted)\n"), None);
        assert_eq!(parse_cgroup("0::relative\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_reads_as_its_own_caller() {
        let me = unsafe { libc::geteuid() };
        let c = read(Path::new("/proc"), std::process::id(), me).unwrap();
        assert_eq!(c.uid, me);
        assert_eq!(c.gid, unsafe { libc::getegid() });
        assert!(!c.limits.is_empty());
        let e = read(Path::new("/proc"), std::process::id(), me + 1).unwrap_err();
        assert!(e.contains("changed its user"), "{e}");
    }
}
