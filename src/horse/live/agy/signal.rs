//! The interrupt of a headless agy: SIGINT to the process that reads the
//! custody pipe.
//!
//! stream-json has no interrupt line, and agy ends the session on a line
//! it does not take. The Antigravity CLI ends a running turn on SIGINT,
//! with `finished_reason: "interrupted"` in its `result`. paddock does not
//! start agy itself: the sandbox does (bwrap, its init, then agy). A
//! SIGINT to the sandbox would end the sandbox. So paddock looks for the
//! processes under the sandbox whose stdin is the pipe of the custody,
//! and that have no child process that reads it too: the harness, not the
//! sandbox processes that hand the pipe down.

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};

/// The pipe of a stdin: (device, inode).
pub type Pipe = (u64, u64);

/// The pipe of an open file, when it is a pipe.
pub fn pipe_of(fd: &impl std::os::fd::AsFd) -> Option<Pipe> {
    let f = std::fs::File::from(fd.as_fd().try_clone_to_owned().ok()?);
    let m = f.metadata().ok()?;
    m.file_type().is_fifo().then(|| (m.dev(), m.ino()))
}

/// The parent of every process that `/proc` shows.
fn parents() -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            continue;
        };
        // The command name is in parentheses and may hold blanks.
        let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
            continue;
        };
        if let Some(ppid) = rest.split_whitespace().nth(1).and_then(|p| p.parse().ok()) {
            out.insert(pid, ppid);
        }
    }
    out
}

fn reads(pid: u32, pipe: Pipe) -> bool {
    std::fs::metadata(format!("/proc/{pid}/fd/0"))
        .is_ok_and(|m| m.file_type().is_fifo() && (m.dev(), m.ino()) == pipe)
}

/// The processes under `root` (and `root`) that read `pipe` as their
/// stdin, without a child process that reads it too.
pub fn readers(root: u32, pipe: Pipe) -> Vec<u32> {
    let parents = parents();
    let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, ppid) in &parents {
        kids.entry(*ppid).or_default().push(*pid);
    }
    let mut under = vec![root];
    let mut i = 0;
    while i < under.len() {
        under.extend(kids.get(&under[i]).into_iter().flatten().copied());
        i += 1;
    }
    let reading: Vec<u32> = under.into_iter().filter(|p| reads(*p, pipe)).collect();
    let below = |p: u32, q: u32| {
        // q is under p.
        let mut x = q;
        while let Some(up) = parents.get(&x).copied() {
            if up == p {
                return true;
            }
            if up == x || up == 0 {
                break;
            }
            x = up;
        }
        false
    };
    let mut out: Vec<u32> = reading
        .iter()
        .copied()
        .filter(|p| !reading.iter().any(|q| q != p && below(*p, *q)))
        .collect();
    out.sort_unstable();
    out
}

/// Send SIGINT to the readers of `pipe` under `root`.
pub fn interrupt(root: u32, pipe: Pipe) -> Result<Vec<u32>> {
    let pids = readers(root, pipe);
    if pids.is_empty() {
        bail!("no process under {root} reads the input of the custody");
    }
    for p in &pids {
        // SAFETY: kill only sends a signal to a process of this user.
        if unsafe { libc::kill(*p as libc::pid_t, libc::SIGINT) } != 0 {
            bail!(
                "cannot send SIGINT to process {p}: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    Ok(pids)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn details(pids: &[u32]) -> String {
        pids.iter()
            .map(|p| {
                format!(
                    "{p}: comm={:?}, stat={:?}",
                    std::fs::read_to_string(format!("/proc/{p}/comm")),
                    std::fs::read_to_string(format!("/proc/{p}/stat"))
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[test]
    fn the_reader_of_the_pipe_is_the_deepest_process_that_reads_it() {
        // sh reads the pipe and hands it to cat, which reads it too.
        let mut c = Command::new("sh")
            .args(["-c", "cat; exit 0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pipe = pipe_of(c.stdin.as_ref().unwrap()).expect("a pipe");
        let root = c.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let found = loop {
            let r = readers(root, pipe);
            if r.len() == 1
                && r[0] != root
                && std::fs::read_to_string(format!("/proc/{}/comm", r[0]))
                    .is_ok_and(|s| s.trim() == "cat")
            {
                break r;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no cat: {}",
                details(&r)
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(found.len(), 1, "{}", details(&found));
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", found[0])).unwrap();
        assert_eq!(comm.trim(), "cat", "{}", details(&found));
        // SIGINT ends cat and not sh: sh ends with its own code.
        assert_eq!(interrupt(root, pipe).unwrap(), found, "{}", details(&found));
        assert!(c.wait().unwrap().success(), "{}", details(&found));
        assert!(
            interrupt(root, pipe).is_err(),
            "nothing reads it any more: {}",
            details(&readers(root, pipe))
        );
    }
}
