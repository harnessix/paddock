//! The secret files of a sandbox as FIFOs, for backends without the FUSE
//! mount (Seatbelt on macOS).
//!
//! Each `as = "file"` credential gets a FIFO `secret/NAME`, mode 0600, in
//! the run folder of the sandbox, and one thread on the host. The thread
//! opens the FIFO for writing without blocking: that fails with ENXIO while
//! nobody reads, so it tries again every 50 ms. When a reader opens it, the
//! thread counts one use (the same audit as a FUSE open), writes the value
//! whole and closes: the reader gets the value, then end of file.
//!
//! Before it closes, the thread puts a new FIFO in place of the old one
//! (rename is atomic). The reader keeps the old one until its end of file,
//! and the next open of the path is a new reader, so one open is one use.
//!
//! A regular file would hold the value on disk for the whole run, and a
//! revoke could not fail the read of a file that is open already. After a
//! revoke, or at the end of the run, the thread stops within 50 ms: it ends
//! a reader that waits with an empty read and removes the FIFO, so a later
//! open fails with ENOENT.

use super::SecretFiles;
use anyhow::{Context, Result};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a thread tries to open its FIFO, and looks for new secrets.
const POLL: Duration = Duration::from_millis(50);
/// The longest time a write of one value may take.
const WRITE_LIMIT: Duration = Duration::from_secs(5);

/// Where the secret file `name` is inside the sandbox: on macOS (`fifo`)
/// a FIFO in the folder that `$PADDOCK_DEVICES` names, else the file in
/// `/dev/paddock/secret`.
pub fn path_on(fifo: bool, name: &str) -> String {
    use crate::devices::{MOUNT_IN_SANDBOX, SECRET_DIR};
    if fifo {
        format!("$PADDOCK_DEVICES/{SECRET_DIR}/{name}")
    } else {
        format!("{MOUNT_IN_SANDBOX}/{SECRET_DIR}/{name}")
    }
}

/// The FIFOs of one sandbox. Dropping it stops the threads and removes
/// the FIFOs and their folder.
pub struct SecretFifos {
    pub dir: PathBuf,
    stop: Arc<AtomicBool>,
    manager: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SecretFifos {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.manager.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// Make `path` a FIFO, mode 0600.
pub fn mkfifo(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: a valid C string; mkfifo only reads it.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600 as libc::mode_t) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Open the FIFO for writing without blocking: ENXIO while nobody reads.
fn open_writer(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
}

/// A name that can be a file in the folder.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains('/') && !name.contains('\0')
}

/// Serve the secret files of `secrets` as FIFOs in `dir`, which paddock
/// creates. The store fills after the start card: new names get their
/// FIFO within 50 ms.
pub fn start(dir: &Path, secrets: Arc<SecretFiles>, sandbox: &str) -> Result<SecretFifos> {
    crate::privfs::dir(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let stop = Arc::new(AtomicBool::new(false));
    let (d, s, sandbox) = (dir.to_path_buf(), stop.clone(), sandbox.to_string());
    let manager = std::thread::spawn(move || {
        let mut started: Vec<String> = Vec::new();
        let mut threads = Vec::new();
        while !s.load(Ordering::SeqCst) {
            for name in secrets.names() {
                if started.contains(&name) || !plain_name(&name) {
                    continue;
                }
                started.push(name.clone());
                let path = d.join(&name);
                let _ = std::fs::remove_file(&path);
                if let Err(e) = mkfifo(&path) {
                    log::warn!("cannot create the secret file {}: {e}", path.display());
                    continue;
                }
                let (secrets, s, sandbox) = (secrets.clone(), s.clone(), sandbox.clone());
                threads.push(std::thread::spawn(move || {
                    writer(&path, &name, &secrets, &s, &sandbox)
                }));
            }
            std::thread::sleep(POLL);
        }
        for t in threads {
            let _ = t.join();
        }
    });
    Ok(SecretFifos {
        dir: dir.to_path_buf(),
        stop,
        manager: Some(manager),
    })
}

/// The loop of one FIFO, until a revoke or the end of the run.
fn writer(path: &Path, name: &str, secrets: &SecretFiles, stop: &AtomicBool, sandbox: &str) {
    let fresh = path.with_file_name(format!(".{name}.new"));
    while !stop.load(Ordering::SeqCst) && secrets.check(name).is_ok() {
        let f = match open_writer(path) {
            Ok(f) => f,
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(e) => {
                log::warn!("secret file {name}: {e}");
                break;
            }
        };
        // The next reader gets a new FIFO; this reader keeps the old one.
        let _ = std::fs::remove_file(&fresh);
        let renewed = mkfifo(&fresh).and_then(|_| std::fs::rename(&fresh, path));
        let Ok(v) = secrets.open(name) else {
            // Revoked between the check and the open: an empty read.
            break;
        };
        log::info!("sandbox {sandbox}: secret file {name} opened");
        if let Err(e) = write_all(f, v.as_bytes(), stop) {
            log::debug!("secret file {name}: the write failed: {e}");
        }
        if let Err(e) = renewed {
            log::warn!("secret file {name}: cannot renew the FIFO: {e}");
            break;
        }
    }
    // A reader that waits in its open gets an empty read, not a hang.
    if let Ok(f) = open_writer(path) {
        drop(f);
    }
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(&fresh);
}

/// Write all of `data` to the FIFO that is open without blocking.
fn write_all(mut f: std::fs::File, mut data: &[u8], stop: &AtomicBool) -> std::io::Result<()> {
    let start = Instant::now();
    while !data.is_empty() {
        match f.write(data) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if stop.load(Ordering::SeqCst) || start.elapsed() > WRITE_LIMIT {
                    return Err(std::io::ErrorKind::TimedOut.into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::secret::Secret;
    use std::sync::Mutex;

    fn wait_for(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < limit, "{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn is_fifo(p: &Path) -> bool {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_fifo())
    }

    #[test]
    fn a_fifo_gives_the_value_once_per_open_and_goes_after_a_revoke() {
        if crate::storage::package_cache::isolated_test("credentials::secret::fifo::tests::a_fifo_gives_the_value_once_per_open_and_goes_after_a_revoke") { return; }
        let tmp = tempfile::tempdir().unwrap();
        let revoked = tmp.path().join("revoked");
        let reads = Arc::new(Mutex::new(Vec::new()));
        let r2 = reads.clone();
        let secrets = SecretFiles::new(
            Some(revoked.clone()),
            Some(Arc::new(move |n: &str| {
                r2.lock().unwrap().push(n.to_string())
            })),
        );
        assert_eq!(path_on(true, "T"), "$PADDOCK_DEVICES/secret/T");
        let dir = tmp.path().join("secret");
        let fifos = start(&dir, secrets.clone(), "box1").unwrap();
        // The store fills after the start.
        secrets.insert("T", Secret::from("tok-123"));
        let path = dir.join("T");
        wait_for("the FIFO appears", Duration::from_secs(2), || {
            is_fifo(&path)
        });
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let p2 = path.clone();
        let got = std::thread::spawn(move || std::fs::read(&p2).unwrap())
            .join()
            .unwrap();
        assert_eq!(got, b"tok-123");
        assert_eq!(*reads.lock().unwrap(), ["T"]);
        // A second read is a second use, and gets the whole value again.
        assert_eq!(std::fs::read(&path).unwrap(), b"tok-123");
        assert_eq!(reads.lock().unwrap().len(), 2);

        std::fs::write(&revoked, "T\n").unwrap();
        let t = Instant::now();
        wait_for("the FIFO goes", Duration::from_millis(100) + POLL, || {
            !path.exists()
        });
        assert!(
            t.elapsed() < Duration::from_millis(150),
            "{:?}",
            t.elapsed()
        );
        let e = std::fs::read(&path).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(reads.lock().unwrap().len(), 2);
        drop(fifos);
        assert!(!dir.exists());
    }

    #[test]
    fn the_end_of_the_run_removes_the_fifos() {
        let tmp = tempfile::tempdir().unwrap();
        let secrets = SecretFiles::new(None, None);
        secrets.insert("A", Secret::from("a"));
        secrets.insert("../B", Secret::from("b"));
        let dir = tmp.path().join("secret");
        let fifos = start(&dir, secrets, "box1").unwrap();
        wait_for("the FIFO appears", Duration::from_secs(2), || {
            is_fifo(&dir.join("A"))
        });
        assert!(!tmp.path().join("B").exists());
        drop(fifos);
        assert!(!dir.exists());
    }
}
