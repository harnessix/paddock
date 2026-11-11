//! Best-effort cache reclaim when a detached sandbox becomes idle.
use crate::env::HostEnv;
use crate::state::Running;
use std::path::{Component, Path, PathBuf};

const MAX_RECLAIM: u64 = 1 << 30;

/// Schedule reclaim away from the idle watcher: the kernel write can block.
pub fn on_idle(env: &HostEnv, id: &str, since: u64) {
    let env = env.clone();
    let id = id.to_string();
    std::thread::spawn(move || {
        if crate::detach::idle_since(&env, &id) != Some(since) {
            return;
        }
        let Ok(bytes) = std::fs::read(Running::file(&env, &id)) else {
            return;
        };
        let Ok(run) = serde_json::from_slice::<Running>(&bytes) else {
            return;
        };
        if run.id != id || !matches!(run.backend.as_str(), "bwrap" | "srt") {
            return;
        }
        if !dedicated_scope(&run) {
            return;
        }
        if crate::detach::idle_since(&env, &id) != Some(since) {
            return;
        }
        match reclaim(
            Path::new("/proc"),
            Path::new("/sys/fs/cgroup"),
            run.pid,
            run.unit.as_deref().unwrap(),
        ) {
            Ok(Some(how)) => {
                let _ = crate::detach::add_note(&env, &id, how);
            }
            Ok(None) => {}
            Err(e) => log::debug!("idle cache reclaim for {id} unavailable: {e}"),
        }
    });
}

fn dedicated_scope(run: &Running) -> bool {
    let Some(unit) = run.unit.as_deref() else {
        return false;
    };
    if unit == crate::freeze::scope_name(&run.id) {
        return true;
    }
    // A spawned child has its own systemd scope, named by its spawn id.
    // Other inherited scopes may contain another sandbox and are not safe.
    run.spawn_id.as_deref().is_some_and(|id| {
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && unit == format!("paddock-spawn-{id}.scope")
    })
}

fn scope_dir(proc_root: &Path, root: &Path, pid: u32, unit: &str) -> Option<PathBuf> {
    if !crate::freeze::ours(unit) {
        return None;
    }
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
    let raw = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    let rel = Path::new(raw.strip_prefix('/')?);
    if rel.components().any(|c| !matches!(c, Component::Normal(_)))
        || rel.file_name()?.to_str()? != unit
    {
        return None;
    }
    Some(root.join(rel))
}

fn reclaim(
    proc_root: &Path,
    root: &Path,
    pid: u32,
    unit: &str,
) -> std::io::Result<Option<&'static str>> {
    let Some(dir) = scope_dir(proc_root, root, pid, unit) else {
        return Ok(None);
    };
    let stats = std::fs::read_to_string(dir.join("memory.stat"))?;
    let bytes = stats
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key == "inactive_file")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
        .unwrap_or(0)
        .min(MAX_RECLAIM);
    if bytes == 0 {
        return Ok(None);
    }
    request(bytes, |value| {
        std::fs::write(dir.join("memory.reclaim"), value)
    })
    .map(Some)
}

fn request(
    bytes: u64,
    mut write: impl FnMut(&str) -> std::io::Result<()>,
) -> std::io::Result<&'static str> {
    match write(&format!("{bytes} swappiness=0")) {
        Ok(()) => Ok("idle reclaim requested with swappiness=0"),
        Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => Ok("idle reclaim partially completed with swappiness=0"),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => match write(&bytes.to_string()) {
            Ok(()) => Ok("idle reclaim requested without swappiness override; anonymous memory may also be reclaimed"),
            Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => Ok("idle reclaim partially completed without swappiness override; anonymous memory may also be reclaimed"),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_plain_only_when_the_kernel_rejects_the_option() {
        let mut writes = Vec::new();
        let result = request(42, |value| {
            writes.push(value.to_string());
            if writes.len() == 1 {
                Err(std::io::Error::from_raw_os_error(libc::EINVAL))
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(writes, ["42 swappiness=0", "42"]);
        assert!(result.contains("without swappiness override"));
        assert!(result.contains("anonymous memory"));
        let mut count = 0;
        assert!(request(42, |_| {
            count += 1;
            Err(std::io::Error::from_raw_os_error(libc::EACCES))
        })
        .is_err());
        assert_eq!(count, 1);
    }

    #[test]
    fn only_the_sandboxes_own_or_spawn_scope_is_eligible() {
        let mut run: Running = serde_json::from_value(serde_json::json!({
            "id":"x", "harness":"codex", "project":"/tmp", "backend":"bwrap",
            "network":"none", "gui":"off", "pid":42, "started":1,
            "unit":"paddock-sb-x.scope"
        }))
        .unwrap();
        assert!(dedicated_scope(&run));
        run.unit = Some("paddock-sb-other.scope".into());
        assert!(!dedicated_scope(&run));
        run.spawn_id = Some("child-123".into());
        run.unit = Some("paddock-spawn-child-123.scope".into());
        assert!(dedicated_scope(&run));
        run.spawn_id = Some("../bad".into());
        assert!(!dedicated_scope(&run));
    }

    #[test]
    fn reclaims_only_the_verified_scope_and_bounds_the_request() {
        let t = tempfile::tempdir().unwrap();
        let proc_root = t.path().join("proc");
        let root = t.path().join("cg");
        let dir = root.join("user/paddock-sb-x.scope");
        std::fs::create_dir_all(proc_root.join("42")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(proc_root.join("42/cgroup"), "0::/user/paddock-sb-x.scope\n").unwrap();
        std::fs::write(
            dir.join("memory.stat"),
            "inactive_file 2147483648\nanon 100\n",
        )
        .unwrap();
        std::fs::write(dir.join("memory.reclaim"), "").unwrap();
        reclaim(&proc_root, &root, 42, "paddock-sb-x.scope").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("memory.reclaim")).unwrap(),
            "1073741824 swappiness=0"
        );
        std::fs::write(dir.join("memory.reclaim"), "unchanged").unwrap();
        reclaim(&proc_root, &root, 42, "paddock-sb-y.scope").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("memory.reclaim")).unwrap(),
            "unchanged"
        );
        std::fs::write(
            proc_root.join("42/cgroup"),
            "0::/user/../paddock-sb-x.scope\n",
        )
        .unwrap();
        assert!(scope_dir(&proc_root, &root, 42, "paddock-sb-x.scope").is_none());
    }
    #[test]
    #[ignore = "requires a writable systemd user cgroup on the host"]
    fn host_reclaims_a_fake_scope() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let id = format!("reclaim-test-{}", std::process::id());
        let unit = crate::freeze::scope_name(&id);
        let mut child = ChildGuard(Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", "--unit", &unit, "--", "sh", "-c", "dd if=/dev/zero of=\"$1/data\" bs=1M count=64 status=none; echo ready > \"$1/ready\"; read -r x", "sh"])
            .arg(tmp.path()).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !tmp.path().join("ready").exists() {
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!("fake scope exited before ready: {status}");
            }
            assert!(Instant::now() < deadline, "fake scope did not become ready");
            std::thread::sleep(Duration::from_millis(50));
        }
        let root = Path::new("/sys/fs/cgroup");
        let proc_root = Path::new("/proc");
        let dir = scope_dir(proc_root, root, child.0.id(), &unit)
            .expect("fake child is in its dedicated scope");
        let before = std::fs::read_to_string(dir.join("memory.stat")).unwrap();
        let inactive = before
            .lines()
            .find_map(|line| line.strip_prefix("inactive_file ")?.parse::<u64>().ok())
            .unwrap();
        assert!(
            inactive > 0,
            "fake scope did not create inactive file cache"
        );
        let result = reclaim(proc_root, root, child.0.id(), &unit)
            .expect("kernel accepts reclaim")
            .expect("nonzero request");
        eprintln!("fake scope inactive_file before={inactive}; {result}");
        let after = std::fs::read_to_string(dir.join("memory.stat")).unwrap();
        let remaining = after
            .lines()
            .find_map(|line| line.strip_prefix("inactive_file ")?.parse::<u64>().ok())
            .unwrap();
        eprintln!("fake scope inactive_file after={remaining}");
        assert!(
            remaining < inactive,
            "the fake scope did not reclaim any inactive file cache"
        );
    }
}
