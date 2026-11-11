//! Memory accounting for the cgroup of a running sandbox.
pub mod limit;

use crate::state::Running;
use serde::Serialize;
use std::path::{Component, Path};

#[derive(Debug, Serialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub rss_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub name: &'static str,
    pub rss_bytes: u64,
    pub processes: Vec<Process>,
}

#[derive(Debug, Serialize)]
pub struct Memory {
    pub anon_bytes: Option<u64>,
    pub file_bytes: Option<u64>,
    pub swap_bytes: Option<u64>,
    pub high_bytes: Option<u64>,
    pub high_unlimited: bool,
    pub groups: Vec<Group>,
}

/// Return None if this run has no accessible, verified cgroup v2 scope.
pub fn of_run(run: &Running) -> Option<Memory> {
    read(
        Path::new("/proc"),
        Path::new("/sys/fs/cgroup"),
        run.pid,
        run.unit.as_deref()?,
    )
}

fn read(proc_root: &Path, cgroup_root: &Path, pid: u32, unit: &str) -> Option<Memory> {
    if !crate::freeze::ours(unit) {
        return None;
    }
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
    let path = text.lines().find_map(|l| l.strip_prefix("0::"))?;
    let relative = Path::new(path.strip_prefix('/')?);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    if relative.file_name()?.to_str()? != unit {
        return None;
    }
    let dir = cgroup_root.join(relative);
    let stat = std::fs::read_to_string(dir.join("memory.stat")).ok()?;
    let value = |key| {
        stat.lines().find_map(|l| {
            let (k, v) = l.split_once(' ')?;
            (k == key).then(|| v.trim().parse().ok()).flatten()
        })
    };
    let swap_bytes = std::fs::read_to_string(dir.join("memory.swap.current"))
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let high = std::fs::read_to_string(dir.join("memory.high")).ok();
    let high_bytes = high.as_ref().and_then(|s| s.trim().parse().ok());
    let high_unlimited = high.as_ref().is_some_and(|s| s.trim() == "max");
    let mut ids = Vec::new();
    collect_pids(&dir, 16, &mut ids);
    ids.sort_unstable();
    ids.dedup();
    let mut groups: Vec<Group> = ["agent", "mcp", "paddock", "build", "other"]
        .into_iter()
        .map(|name| Group {
            name,
            rss_bytes: 0,
            processes: Vec::new(),
        })
        .collect();
    for pid in ids {
        let dir = proc_root.join(pid.to_string());
        let Ok(raw) = std::fs::read(dir.join("cmdline")) else {
            continue;
        };
        let args: Vec<_> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let Some(first) = args.first() else { continue };
        let name = Path::new(first)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(rss_bytes) = std::fs::read_to_string(dir.join("status"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| {
                        l.strip_prefix("VmRSS:")
                            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                    })
                    .map(|n| n.saturating_mul(1024))
            })
        else {
            continue;
        };
        let group = classify(&name, &args);
        groups[group].rss_bytes = groups[group].rss_bytes.saturating_add(rss_bytes);
        groups[group].processes.push(Process {
            pid,
            name,
            rss_bytes,
        });
    }
    for group in &mut groups {
        group
            .processes
            .sort_by(|a, b| b.rss_bytes.cmp(&a.rss_bytes).then(a.pid.cmp(&b.pid)));
        group.processes.truncate(5);
    }
    Some(Memory {
        anon_bytes: value("anon"),
        file_bytes: value("file"),
        swap_bytes,
        high_bytes,
        high_unlimited,
        groups,
    })
}

fn classify(name: &str, args: &[String]) -> usize {
    let name = name.to_ascii_lowercase();
    let words = args
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let cargo_test = args.first().is_some_and(|arg| {
        let parts: Vec<_> = Path::new(arg).components().collect();
        parts
            .windows(3)
            .any(|w| w[0].as_os_str() == "target" && w[2].as_os_str() == "deps")
    });
    if cargo_test {
        3
    } else if words.contains("mcp") || words.contains("modelcontextprotocol") {
        1
    } else if matches!(
        name.as_str(),
        "rustc" | "cargo" | "cc" | "gcc" | "clang" | "make" | "ninja" | "ld" | "cmake"
    ) {
        3
    } else if name.starts_with("paddock")
        || matches!(name.as_str(), "bwrap" | "pasta" | "fusermount3")
    {
        2
    } else if matches!(name.as_str(), "codex" | "claude" | "grok" | "agy") {
        0
    } else {
        4
    }
}

fn collect_pids(dir: &Path, depth: usize, out: &mut Vec<u32>) {
    if let Ok(s) = std::fs::read_to_string(dir.join("cgroup.procs")) {
        out.extend(s.lines().filter_map(|l| l.trim().parse::<u32>().ok()));
    }
    if depth == 0 {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                collect_pids(&entry.path(), depth - 1, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cargo_test_binary_is_build_even_if_named_paddock() {
        let path = "/tmp/target/debug/deps/paddock-123".to_string();
        assert_eq!(classify("paddock-123", &[path]), 3);
        assert_eq!(classify("paddock", &["/usr/bin/paddock".into()]), 2);
    }
    #[test]
    fn fake_scope_counts_nested_processes_and_unavailable_values() {
        let tmp = tempfile::tempdir().unwrap();
        let proc = tmp.path().join("proc");
        let cg = tmp.path().join("cg");
        let scope = cg.join("user/paddock-sb-test.scope");
        std::fs::create_dir_all(scope.join("child")).unwrap();
        std::fs::write(scope.join("memory.stat"), "anon 123\nfile 456\n").unwrap();
        std::fs::write(scope.join("memory.swap.current"), "789\n").unwrap();
        std::fs::write(scope.join("memory.high"), "2048\n").unwrap();
        std::fs::write(scope.join("cgroup.procs"), "1\n2\n").unwrap();
        std::fs::write(scope.join("child/cgroup.procs"), "2\n3\n").unwrap();
        for (pid, cmd, rss) in [(1, "codex", 2), (2, "node\0server-mcp", 4), (3, "cargo", 8)] {
            let dir = proc.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("cmdline"), cmd).unwrap();
            std::fs::write(dir.join("status"), format!("VmRSS:\t{rss} kB\n")).unwrap();
        }
        std::fs::write(proc.join("1/cgroup"), "0::/user/paddock-sb-test.scope\n").unwrap();
        let m = read(&proc, &cg, 1, "paddock-sb-test.scope").unwrap();
        assert_eq!(
            (m.anon_bytes, m.file_bytes, m.swap_bytes),
            (Some(123), Some(456), Some(789))
        );
        assert_eq!(m.high_bytes, Some(2048));
        assert!(!m.high_unlimited);
        std::fs::write(scope.join("memory.high"), "max\n").unwrap();
        assert!(
            read(&proc, &cg, 1, "paddock-sb-test.scope")
                .unwrap()
                .high_unlimited
        );
        assert_eq!(m.groups[0].rss_bytes, 2048);
        assert_eq!(m.groups[1].rss_bytes, 4096);
        assert_eq!(m.groups[3].rss_bytes, 8192);
        assert!(read(&proc, &cg, 1, "paddock-other.scope").is_none());
        std::fs::write(proc.join("1/cgroup"), "0::/../paddock-sb-test.scope\n").unwrap();
        assert!(read(&proc, &cg, 1, "paddock-sb-test.scope").is_none());
    }
}
