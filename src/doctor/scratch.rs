//! The scratch folders of the escape runs of `paddock doctor`.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The folder under the paddock cache with one scratch folder per escape
/// run. Older versions used `doctor` and cleared all of it at each start, so
/// this name keeps their runs from deleting the files of a new one.
pub(super) const SCRATCH_DIR: &str = "doctor-runs";

/// The base of the scratch folders: `<paddock cache>/doctor-runs`. The
/// folder check lets an agent start in `<base>/<run>/project`.
pub fn scratch_base(env: &crate::env::HostEnv) -> PathBuf {
    env.paddock_cache().join(SCRATCH_DIR)
}

/// The scratch folder of one escape run: `<base>/<pid>-<random>`, 0700. It
/// holds the project, its remote and the canaries; the run's sandbox home
/// and the fake other home are removed with it. Concurrent runs each have
/// their own, so no run removes the files of another. Drop removes it all,
/// also after an error or a panic, and never follows a link that the probe
/// planted. Git makes its object folders read-only: the removal adds the
/// owner write bit first.
pub struct Scratch {
    pub root: PathBuf,
    extra: Vec<PathBuf>,
}

impl Scratch {
    /// Make a new scratch folder under `base`, after removing what ended
    /// runs left there.
    pub fn new(base: &Path) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt as _;
        crate::privfs::dir(base)?;
        clear_stale(base);
        for _ in 0..100 {
            let root = base.join(format!("{}-{}", std::process::id(), random_hex()));
            match std::fs::DirBuilder::new().mode(0o700).create(&root) {
                Ok(()) => {
                    crate::privfs::tighten(&root, 0o700)?;
                    return Ok(Self {
                        root,
                        extra: vec![],
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("no free scratch folder in {}", base.display())
    }

    /// Remove `path` together with the scratch folder.
    pub fn also_remove(&mut self, path: PathBuf) {
        self.extra.push(path);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for p in std::iter::once(&self.root).chain(self.extra.iter()) {
            if let Err(e) = crate::safefs::remove_tree_path(p) {
                eprintln!("paddock: cannot remove {}: {e:#}", p.display());
            }
        }
    }
}

pub(super) fn random_hex() -> String {
    use std::io::Read as _;
    let mut b = [0u8; 8];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        b = (n as u64).to_le_bytes();
    }
    crate::manifest::hex(&b)
}

/// Remove what runs that ended without cleanup left in `base`: folders of
/// processes that no longer run, and the shared tree of older versions.
/// Folders of live processes stay.
fn clear_stale(base: &Path) {
    for e in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let live = name
            .split_once('-')
            .and_then(|(pid, _)| pid.parse::<i32>().ok())
            .is_some_and(|pid| pid > 0 && process_alive(pid));
        if !live {
            let _ = crate::safefs::remove_tree_path(&e.path());
        }
    }
}

fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
