//! The scan of a host folder: the state that a borrow records and that a
//! return compares with.

use super::{join_rel, utf8, Entry, Kind, HASH_MAX, MAX_DEPTH, MAX_ENTRIES};
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub(super) fn hash_reader(mut r: impl Read) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(crate::manifest::hex(&h.finalize()))
}

/// The entries of a host folder that only the host writes. Symlinks are
/// recorded, never followed. FIFOs, sockets and devices are left out.
pub fn scan(root: &Path) -> Result<BTreeMap<String, Entry>> {
    let mut out = BTreeMap::new();
    scan_at(root, "", 0, &mut out)?;
    Ok(out)
}

/// The entries of the opened folder. Linux names it through
/// /proc/self/fd, so the scan reads the open folder itself.
pub fn scan_open(src: &crate::safefs::Source) -> Result<BTreeMap<String, Entry>> {
    if cfg!(target_os = "linux") {
        return scan(&src.proc_path()?);
    }
    scan_by_path(src)
}

/// macOS has no path that names an open folder: the scan reads its real
/// path, and the folder there must be the open one before and after. The
/// original is a host folder that the sandbox cannot write (`fetch`
/// refuses those), and a return scans it by its path too (`check_original`).
pub(super) fn scan_by_path(src: &crate::safefs::Source) -> Result<BTreeMap<String, Entry>> {
    let real = src.real_path()?;
    let check = || -> Result<()> {
        let m = std::fs::symlink_metadata(&real)?;
        if m.file_type().is_symlink() || src.id()? != (m.dev(), m.ino()) {
            bail!("{} changed while paddock copied it", real.display());
        }
        Ok(())
    };
    check()?;
    let entries = scan(&real)?;
    check()?;
    Ok(entries)
}

fn scan_at(dir: &Path, rel: &str, depth: usize, out: &mut BTreeMap<String, Entry>) -> Result<()> {
    if depth > MAX_DEPTH {
        bail!("{} is too deep", dir.display());
    }
    let mut names = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    for n in names {
        let r = join_rel(rel, &utf8(&n, rel)?);
        let p = dir.join(&n);
        let m = std::fs::symlink_metadata(&p)?;
        let t = m.file_type();
        let e = if t.is_symlink() {
            Entry::symlink(std::fs::read_link(&p)?)
        } else if t.is_dir() {
            Entry::dir()
        } else if t.is_file() {
            let mut e = Entry::file(&m);
            if e.size <= HASH_MAX {
                e.hash = Some(hash_reader(std::fs::File::open(&p)?)?);
            }
            e
        } else {
            continue;
        };
        let is_dir = e.kind == Kind::Dir;
        out.insert(r.clone(), e);
        if out.len() > MAX_ENTRIES {
            bail!("the folder has more than {MAX_ENTRIES} entries");
        }
        if is_dir {
            scan_at(&p, &r, depth + 1, out)?;
        }
    }
    Ok(())
}
