//! The tmux servers of a test. A test that starts a tmux server, itself or
//! through a `paddock run`, holds a [`TmuxGuard`]. When the test ends, fails
//! or panics, the guard runs `tmux -S SOCK kill-server` for each server
//! socket. The guard must drop before the test folder: tmux reaches a server
//! only through its socket file. Unit tests and integration tests use this
//! one helper.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The file names of tmux server sockets that paddock makes: the view
/// server (`view.sock`) and the workbench of a pasture (`tmux`).
const NAMES: [&str; 2] = ["view.sock", crate::pasture::workbench::SOCKET];
/// A test folder is small. The walk stops at this depth.
const DEPTH: usize = 8;

/// Stops tmux servers on drop.
#[derive(Debug, Default)]
pub struct TmuxGuard {
    socks: Vec<PathBuf>,
    roots: Vec<PathBuf>,
}

impl TmuxGuard {
    /// Stop the server on `sock`.
    pub fn socket(sock: impl Into<PathBuf>) -> TmuxGuard {
        TmuxGuard {
            socks: vec![sock.into()],
            roots: vec![],
        }
    }

    /// Stop the server of `tmux`.
    pub fn of(tmux: &super::Tmux) -> TmuxGuard {
        Self::socket(tmux.sock.clone())
    }

    /// Stop every paddock tmux server with a socket in `root`, and the view
    /// server of a runtime folder `root/run` (on macOS its socket can be in
    /// a short folder outside `root`). A test can name `root` through a
    /// symlink (macOS `/var`), so the real path counts too.
    pub fn under(root: impl Into<PathBuf>) -> TmuxGuard {
        let root = root.into();
        let mut roots = vec![root.clone()];
        if let Ok(real) = std::fs::canonicalize(&root) {
            if real != root {
                roots.push(real);
            }
        }
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let socks = roots
            .iter()
            .map(|r| crate::sockpath::socket_dir(&r.join("run/paddock"), uid, "").join("view.sock"))
            .collect();
        TmuxGuard { socks, roots }
    }

    /// The sockets the guard stops now: the named ones and the ones it
    /// finds in its folders.
    pub fn sockets(&self) -> Vec<PathBuf> {
        let mut out = self.socks.clone();
        for r in &self.roots {
            find(r, DEPTH, &mut out);
        }
        out.sort();
        out.dedup();
        out
    }

    /// Stop the servers now. Drop does the same.
    pub fn stop(&self) {
        for sock in self.sockets() {
            kill_server(&sock);
        }
    }
}

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A test folder and the guard of the tmux servers in it: the view server
/// that a `paddock run` starts, and a workbench. The guard stops them when
/// the test ends, fails or panics, before the folder goes. It derefs to the
/// `TempDir`.
pub struct TestDir {
    // Fields drop in this order: the servers first, then the folder.
    _tmux: TmuxGuard,
    dir: tempfile::TempDir,
}

impl std::ops::Deref for TestDir {
    type Target = tempfile::TempDir;
    fn deref(&self) -> &tempfile::TempDir {
        &self.dir
    }
}

/// Guard the tmux servers of the test folder `dir`.
pub fn guarded(dir: tempfile::TempDir) -> TestDir {
    TestDir {
        _tmux: TmuxGuard::under(dir.path()),
        dir,
    }
}

/// A new temporary folder of a test, guarded. It panics when the folder
/// cannot be made.
pub fn tempdir() -> TestDir {
    guarded(tempfile::tempdir().expect("a temporary folder"))
}

/// `tmux -S SOCK kill-server`, quiet. Nothing when the socket is gone.
pub fn kill_server(sock: &Path) {
    if std::fs::symlink_metadata(sock).is_err() {
        return;
    }
    let _ = Command::new("tmux")
        .arg("-S")
        .arg(sock)
        .arg("kill-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The tmux sockets in `dir`, without following links. The walk stays on
/// the file system of `dir`: a test can leave a mount (FUSE) that hangs.
fn find(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    use std::os::unix::fs::MetadataExt;
    if let Ok(m) = std::fs::symlink_metadata(dir) {
        walk(dir, m.dev(), depth, out);
    }
}

fn walk(dir: &Path, dev: u64, depth: usize, out: &mut Vec<PathBuf>) {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth > 0 {
            let path = e.path();
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.dev() == dev) {
                walk(&path, dev, depth - 1, out);
            }
        } else if ft.is_socket() && NAMES.iter().any(|n| e.file_name() == *n) {
            out.push(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_stops_the_server_of_its_folder_on_drop() {
        let t = tempfile::tempdir().unwrap();
        let sock = t.path().join("run/paddock/view.sock");
        std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let tmux = |args: &[&str]| {
            Command::new("tmux")
                .arg("-S")
                .arg(&sock)
                .args(["-f", "/dev/null"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        };
        if tmux(&["-V"]).is_err() {
            eprintln!("skipped: tmux is not installed");
            return;
        }
        let guard = TmuxGuard::under(t.path());
        assert!(tmux(&["new-session", "-d", "-s", "g", "sleep 60"])
            .unwrap()
            .success());
        assert!(guard.sockets().contains(&sock), "{:?}", guard.sockets());
        drop(guard);
        assert!(!tmux(&["has-session", "-t", "g"]).unwrap().success());
    }
}
