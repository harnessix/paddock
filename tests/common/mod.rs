//! The folder that the end-to-end tests work in.
//!
//! Each test gets a newly created directory. We never scan or prune earlier
//! runs, or unmount anything in them. Sandbox projects need a short path
//! outside the home and shared cache.

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// The world of the device sandbox tests.
#[allow(dead_code)]
pub mod device_world;

/// The world of the shared device tests.
#[allow(dead_code)]
pub mod shared_world;

/// The device scenarios that need bubblewrap and FUSE, as functions.
#[allow(dead_code)]
pub mod scenarios;

/// The shared device under the systemd user manager, as a function.
#[allow(dead_code)]
pub mod unit_scenario;

/// The stand-in Mac of the worked example tests: an sshd of the test user.
#[allow(dead_code)]
pub mod sshd;

const PARENT_VAR: &str = "PADDOCK_TEST_PARENT";

fn passwd_home() -> io::Result<PathBuf> {
    // SAFETY: getpwuid_r writes only to the supplied record and buffer. The
    // home is copied while that buffer is still alive.
    unsafe {
        let mut record: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0u8; 16384];
        let mut result = std::ptr::null_mut();
        let err = libc::getpwuid_r(
            libc::geteuid(),
            &mut record,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        );
        if err != 0 || result.is_null() || record.pw_dir.is_null() {
            return Err(io::Error::other("cannot determine the account home"));
        }
        Ok(PathBuf::from(OsStr::from_bytes(
            CStr::from_ptr(record.pw_dir).to_bytes(),
        )))
    }
}

// Resolve existing ancestors as well as an existing path. The protected
// directory might not exist yet, while a parent of it may be a symlink.
fn resolved(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(io::Error::other(
            "protected paths must be absolute and not contain ..",
        ));
    }
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(e)?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("invalid path"))?;
            Ok(resolved(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

/// Create one directory only after validating its parent. Protected paths are
/// homes and caches.
#[allow(dead_code)]
pub fn tempdir_in(parent: &Path, protected: &[PathBuf]) -> io::Result<tempfile::TempDir> {
    if !parent.is_absolute()
        || parent
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        || parent == Path::new("/")
    {
        return Err(io::Error::other(
            "test parent must be an absolute directory",
        ));
    }
    let real = std::fs::canonicalize(parent)?;
    if real != parent {
        return Err(io::Error::other("test parent must not contain symlinks"));
    }
    let m = std::fs::metadata(&real)?;
    // A private parent is safe. A root-owned sticky temporary directory is
    // also safe: only the child owner or root can rename or remove it.
    let private = m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0;
    let sticky = m.uid() == 0 && m.mode() & 0o1000 != 0;
    if !m.is_dir() || !(private || sticky) {
        return Err(io::Error::other(
            "test parent must be private or root-owned and sticky",
        ));
    }
    for p in protected {
        if real.starts_with(resolved(p)?) {
            return Err(io::Error::other("test parent is protected"));
        }
    }
    let dir = tempfile::Builder::new().prefix("t-").tempdir_in(real)?;
    for p in protected {
        if dir.path().starts_with(resolved(p)?) {
            return Err(io::Error::other("new test folder is protected"));
        }
    }
    Ok(dir)
}

// Leave room for the random directory, a run id and Unix socket names.
#[allow(dead_code)]
pub fn fits_socket_paths(parent: &Path) -> bool {
    parent.as_os_str().as_bytes().len() <= 32
}

/// A new, test-owned folder for one test. No other test folder is removed.
pub fn tempdir() -> tempfile::TempDir {
    let env_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME is needed to protect the user's folders");
    let account_home = passwd_home().expect("cannot protect the account home");
    assert!(
        env_home.is_absolute() && account_home.is_absolute(),
        "HOME must be absolute"
    );
    let mut protected = vec![env_home, account_home];
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        let cache = PathBuf::from(cache);
        if cache.is_absolute() {
            protected.push(cache);
        }
    }
    // Linux's standard sticky /var/tmp works with Paddock's explicit project
    // binds on the supported host. macOS uses its short /tmp socket path.
    // Resolve the platform's standard path (macOS /var is a system symlink);
    // an explicit override must itself be a real path, without symlinks.
    let parent = if let Some(p) = std::env::var_os(PARENT_VAR) {
        PathBuf::from(p)
    } else {
        #[cfg(target_os = "linux")]
        let standard = PathBuf::from("/var/tmp");
        #[cfg(target_os = "macos")]
        let standard = PathBuf::from("/tmp");
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let standard = std::env::temp_dir();
        std::fs::canonicalize(&standard).unwrap_or(standard)
    };
    let result = if fits_socket_paths(&parent) {
        tempdir_in(&parent, &protected)
    } else {
        Err(io::Error::other("test parent is too long for Unix sockets"))
    };
    result.unwrap_or_else(|e| {
        panic!("cannot create a safe test folder ({}: {e}). Set {PARENT_VAR} to a short private directory or root-owned sticky temporary directory outside HOME and caches", parent.display())
    })
}

/// The address of the user's D-Bus session bus, read before a test points
/// XDG_RUNTIME_DIR at a scratch folder. `systemd-run --user` finds the user
/// manager through DBUS_SESSION_BUS_ADDRESS, or else through
/// `$XDG_RUNTIME_DIR/bus`, so a process with a scratch runtime folder needs
/// this address to reach the manager. None: no bus socket on this host.
#[allow(dead_code)]
pub fn user_bus_address() -> Option<String> {
    if let Some(a) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").filter(|a| !a.is_empty()) {
        return a.into_string().ok();
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|r| !r.is_empty())
        .map(PathBuf::from)
        // SAFETY: getuid has no failure case.
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    let bus = runtime.join("bus");
    bus.exists().then(|| format!("unix:path={}", bus.display()))
}

/// This host can run an exclave peer: bwrap starts a sandbox with a user
/// namespace, and `systemd-run --user --scope` reaches the user manager in
/// the environment of this test, the one a user has. The peer itself gets a
/// scratch XDG_RUNTIME_DIR; the product finds the manager from there.
///
/// false: the test skips, because the feature cannot work here (no bwrap,
/// or no user manager, as in a container or a CI runner). With
/// PADDOCK_REQUIRE_PEER=1, which the host gate sets, a skip is a failure:
/// a host with bwrap and a user bus must run the test.
#[allow(dead_code)]
pub fn peer_host_ready(test: &str) -> bool {
    let bwrap = bwrap_works();
    let scope = std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    let scope_error = match &scope {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Some(e.to_string()),
    };
    if bwrap && scope_error.is_none() {
        return true;
    }
    let why = format!(
        "{test} needs bwrap and a systemd user manager: bwrap {}, systemd-run --user --scope {}",
        if bwrap { "works" } else { "fails" },
        scope_error.map_or("works".to_string(), |e| format!("fails: {e}")),
    );
    if std::env::var_os("PADDOCK_REQUIRE_PEER").is_some_and(|v| v == "1") {
        panic!("PADDOCK_REQUIRE_PEER=1, but {why}");
    }
    eprintln!("skipped: {why}");
    false
}

/// bubblewrap is on PATH and can start a sandbox with a user namespace. It
/// cannot when the tests themselves run in a sandbox that allows no nested
/// namespace.
#[allow(dead_code)]
pub fn bwrap_works() -> bool {
    std::process::Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A real FUSE mount works here: paddock mounts an empty device file
/// system in a test folder, the mount answers, and paddock unmounts it.
/// A host shell with NoNewPrivs has /dev/fuse and fusermount3, but the
/// setuid helper cannot mount there. The check runs once per test binary.
#[allow(dead_code)]
pub fn fuse_mount_works() -> bool {
    static WORKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WORKS.get_or_init(|| {
        use std::os::unix::fs::MetadataExt;
        let dir = tempdir();
        let mp = dir.path().join("dev");
        let fs = paddock::devices::DeviceFs::new("fuse-check", Vec::new());
        let Ok(m) = paddock::devices::mount(&mp, fs) else {
            return false;
        };
        // The mount point is on another file system now, and it answers.
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        let mounted = dev(&mp).is_some() && dev(&mp) != dev(dir.path());
        let answers = std::fs::read_dir(&mp).is_ok();
        m.unmount();
        mounted && answers
    })
}

/// bubblewrap starts a sandbox and a FUSE mount works, so paddock can give
/// a sandbox its /dev/paddock. The check runs once per test binary.
#[allow(dead_code)]
pub fn sandbox_works() -> bool {
    static WORKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WORKS.get_or_init(|| bwrap_works() && fuse_mount_works())
}

/// A systemd user instance runs, and the session bus is known.
#[allow(dead_code)]
pub fn systemd_user() -> bool {
    std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
        && std::process::Command::new("systemctl")
            .args(["--user", "is-system-running"])
            .output()
            .is_ok_and(|o| {
                matches!(
                    String::from_utf8_lossy(&o.stdout).trim(),
                    "running" | "degraded"
                )
            })
}
