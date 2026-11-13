//! The store of one project: `<data>/copies/<store id>/`.
//!
//! `store.json` records which folder the store belongs to: its device, its
//! inode and its last path. The folder names the store through its pointer
//! (pointer.rs). paddock finds a store by its id, never by a path, so a
//! project that moves keeps its store.

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const META: &str = "store.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreMeta {
    pub format: u32,
    pub id: String,
    pub created: u64,
    /// The last path of the original folder.
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
    pub meta: StoreMeta,
}

/// A random UUID (version 4), as text.
pub fn new_id() -> Result<String> {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| anyhow::anyhow!("no random bytes for a store id"))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = crate::manifest::hex(&b);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

/// A store id is a UUID: 36 characters of hex and dashes. Nothing else
/// becomes a folder name.
pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        })
}

/// The device and inode of a folder. A symlink is not followed.
pub fn folder_id(p: &Path) -> Option<(u64, u64)> {
    let m = std::fs::symlink_metadata(p).ok()?;
    m.is_dir().then(|| (m.dev(), m.ino()))
}

impl Store {
    /// A new, empty store for `project` in `root`.
    pub fn create(root: &Path, project: &Path) -> Result<Store> {
        let (dev, ino) =
            folder_id(project).with_context(|| format!("{} is not a folder", project.display()))?;
        let id = new_id()?;
        let dir = root.join(&id);
        crate::privfs::dir(&dir)?;
        let s = Store {
            dir,
            meta: StoreMeta {
                format: super::FORMAT,
                id,
                created: crate::state::now(),
                path: project.to_path_buf(),
                dev,
                ino,
            },
        };
        s.save()?;
        Ok(s)
    }

    /// The store `id` of `root`. None when it is not there.
    pub fn open(root: &Path, id: &str) -> Result<Option<Store>> {
        if !valid_id(id) {
            bail!("{id} is not a store id");
        }
        let dir = root.join(id);
        let f = dir.join(META);
        let text = match std::fs::read_to_string(&f) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", f.display())),
        };
        let meta: StoreMeta =
            serde_json::from_str(&text).with_context(|| format!("{} is invalid", f.display()))?;
        if meta.format > super::FORMAT {
            bail!(
                "{} has format {}, which a newer paddock wrote. Update paddock",
                f.display(),
                meta.format
            );
        }
        if meta.id != id {
            bail!("{} names another store", f.display());
        }
        Ok(Some(Store { dir, meta }))
    }

    /// The store `id` in the first of `roots` that has it.
    pub fn find(roots: &[PathBuf], id: &str) -> Result<Option<Store>> {
        for r in roots {
            if let Some(s) = Self::open(r, id)? {
                return Ok(Some(s));
            }
        }
        Ok(None)
    }

    /// Every store of `roots`. A broken one is left out.
    pub fn all(roots: &[PathBuf]) -> Vec<Store> {
        let mut out = Vec::new();
        for r in roots {
            let mut ids: Vec<String> = std::fs::read_dir(r)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| valid_id(n))
                .collect();
            ids.sort();
            for id in ids {
                if let Ok(Some(s)) = Self::open(r, &id) {
                    out.push(s);
                }
            }
        }
        out
    }

    pub fn save(&self) -> Result<()> {
        crate::privfs::write_atomic(&self.dir.join(META), serde_json::to_vec_pretty(&self.meta)?)?;
        Ok(())
    }

    pub fn backups(&self) -> PathBuf {
        self.dir.join("backups")
    }

    pub fn runs(&self) -> PathBuf {
        self.dir.join("runs")
    }

    /// One writer at a time: an apply, an undo, a restore or a start. The
    /// lock goes with the returned file.
    pub fn lock(&self) -> Result<std::fs::File> {
        let f = crate::privfs::append(&self.dir.join("lock"))?;
        use std::os::fd::AsRawFd;
        // SAFETY: flock on an open file.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("cannot lock {}", self.dir.display()));
        }
        Ok(f)
    }
}
