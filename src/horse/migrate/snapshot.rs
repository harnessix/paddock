//! A no-symlink snapshot of a plaintext store or local registry.

use anyhow::{bail, Context as _, Result};
use sha2::{Digest, Sha256};
use std::io::Read as _;
use std::path::{Path, PathBuf};

pub(super) const MAX_RECORD: u64 = 256 * 1024 * 1024;

pub(super) struct Snapshot {
    pub digest: String,
    pub files: Vec<PathBuf>,
}

/// Hash names, directory structure and every file's exact bytes. Special
/// files and symlinks are refused, never followed.
pub(super) fn scan(path: &Path) -> Result<Snapshot> {
    let root = crate::safefs::Root::open(path)?;
    let mut hash = Sha256::new();
    let mut files = Vec::new();
    walk(&root, Path::new(""), &mut hash, &mut files, 0)?;
    Ok(Snapshot {
        digest: crate::horse::hex(&hash.finalize()),
        files,
    })
}

fn walk(
    root: &crate::safefs::Root,
    rel: &Path,
    hash: &mut Sha256,
    files: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<()> {
    if depth > 256 {
        bail!("migration source is nested too deeply");
    }
    let dir = root.dir(rel, false)?;
    let mut entries = dir.entries()?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, kind) in entries {
        let relative = rel.join(&name);
        let text = relative
            .to_str()
            .context("migration source has a non-UTF-8 name")?;
        let name_bytes = text.as_bytes();
        hash.update((name_bytes.len() as u64).to_be_bytes());
        hash.update(name_bytes);
        match kind {
            crate::safefs::Kind::Dir => {
                hash.update(b"D");
                walk(root, &relative, hash, files, depth + 1)?;
            }
            crate::safefs::Kind::File => {
                hash.update(b"F");
                let mut file = dir
                    .open_read(&name)?
                    .context("migration source changed while scanning")?;
                let size = file.metadata()?.len();
                hash.update(size.to_be_bytes());
                let mut content = Sha256::new();
                let mut buf = [0u8; 64 * 1024];
                let mut total = 0u64;
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    total = total
                        .checked_add(n as u64)
                        .context("migration source is too large")?;
                    content.update(&buf[..n]);
                }
                if total != size {
                    bail!("migration source changed while scanning");
                }
                hash.update(content.finalize());
                files.push(relative);
            }
            _ => bail!("migration source contains a symlink or special file: {text}"),
        }
    }
    Ok(())
}

pub(super) fn read(root: &crate::safefs::Root, path: &Path) -> Result<Vec<u8>> {
    root.read(path, MAX_RECORD)?
        .context("migration source changed while reading")
}
