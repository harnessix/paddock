//! `shares.open`: the files that processes of the guest hold open under
//! `/Volumes/My Shared Files`, counted per share, so the host can say
//! which folder the app must open again after a grant change
//! (`macvm::grants_live`). `lsof` gets the mount point as its argument,
//! which lists every open file of that file system; the agent's own
//! process is left out. It runs in a thread of its own with a deadline.

use super::platform::Fail;
use crate::macvm::grants_live::count_open;
use crate::macvm::proto::{ErrorCode, SHARE_ROOT};
use crate::macvm::text;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The lsof of macOS.
pub const LSOF: &str = "/usr/sbin/lsof";
/// The longest wait for lsof.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// Count the open files per share. No share root: no open files.
pub fn count() -> Result<BTreeMap<String, u32>, Fail> {
    count_at(SHARE_ROOT)
}

fn count_at(root: &str) -> Result<BTreeMap<String, u32>, Fail> {
    if !Path::new(root).is_dir() {
        return Ok(BTreeMap::new());
    }
    let out = run(LSOF, &["-n", "-P", "-w", "-F", "n", "--", root], DEADLINE)?;
    Ok(count_open(&out, root, std::process::id()))
}

/// The output of a command, killed at the deadline. lsof exits 1 when it
/// finds no open file: any exit with an output that parses counts.
pub fn run(program: &str, args: &[&str], deadline: Duration) -> Result<String, Fail> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            (
                ErrorCode::Io,
                text::open_files_failed(program, &e.to_string()),
            )
        })?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err((ErrorCode::Timeout, text::open_files_timeout(program)));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                return Err((
                    ErrorCode::Io,
                    text::open_files_failed(program, &e.to_string()),
                ))
            }
        }
    }
    Ok(reader.join().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_reads_the_output_and_kills_at_the_deadline() {
        let out = run("/bin/sh", &["-c", "echo n/x; exit 1"], DEADLINE).unwrap();
        assert_eq!(out, "n/x\n");
        let started = Instant::now();
        let e = run("/bin/sh", &["-c", "sleep 5"], Duration::from_millis(200)).unwrap_err();
        assert_eq!(e.0, ErrorCode::Timeout);
        assert!(started.elapsed() < Duration::from_secs(3));
        let e = run("/nonexistent/lsof", &[], DEADLINE).unwrap_err();
        assert_eq!(e.0, ErrorCode::Io);
    }
}

/// Exercise the production lsof call against a real mounted file system.
#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;
    use std::io::{Read, Write};

    struct Mounted(std::path::PathBuf);
    impl Drop for Mounted {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", "-force"])
                .arg(&self.0)
                .status();
        }
    }

    #[test]
    fn lsof_counts_an_open_file_on_a_mounted_volume() {
        let t = tempfile::tempdir().unwrap();
        let image = t.path().join("test.dmg");
        let mount = t.path().join("volume");
        std::fs::create_dir(&mount).unwrap();
        assert!(Command::new("/usr/bin/hdiutil")
            .args([
                "create",
                "-size",
                "32m",
                "-fs",
                "HFS+",
                "-volname",
                "paddock-test"
            ])
            .arg(&image)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&image)
            .status()
            .unwrap()
            .success());
        let _mounted = Mounted(mount.clone());
        let root = std::fs::canonicalize(&mount).unwrap();
        std::fs::create_dir(root.join("ws-1")).unwrap();
        std::fs::write(root.join("ws-1/open.txt"), b"test").unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec 3<\"$1\"; printf ready; read line", "sh"])
            .arg(root.join("ws-1/open.txt"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0; 5];
        child.stdout.take().unwrap().read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        let counts = count_at(root.to_str().unwrap()).unwrap();
        writeln!(child.stdin.take().unwrap(), "done").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(counts.get("ws-1"), Some(&1), "{counts:?}");
    }
}
