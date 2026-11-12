//! The hook files of an adoption (hooks.rs): read without following a
//! symlink, parsed as a JSON object, and written in place with a rename.

use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The longest hook file that paddock changes.
const MAX_FILE: u64 = 1 << 20;

/// A hook file read without following a symlink.
pub(super) struct File {
    pub(super) path: PathBuf,
    pub(super) bytes: Option<Vec<u8>>,
    pub(super) mode: u32,
    pub(super) stamp: Option<(u64, i64, i64, u64)>,
}

pub(super) fn read(home: &Path, rel: &Path) -> Result<File> {
    super::check_rel(rel)?;
    let path = home.join(rel);
    let parent = rel.parent().unwrap_or(Path::new(""));
    super::no_links(home, parent)?;
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => bail!(
            "~/{} is a symlink; paddock does not change it. Put the hook entries in by hand, or make it a plain file",
            rel.display()
        ),
        Ok(m) if !m.is_file() => bail!("~/{} is not a file", rel.display()),
        Ok(m) if m.len() > MAX_FILE => bail!("~/{} is larger than 1 MiB", rel.display()),
        Ok(m) => {
            let bytes = std::fs::read(&path)?;
            Ok(File {
                path,
                bytes: Some(bytes),
                mode: m.permissions().mode() & 0o7777,
                stamp: Some((m.len(), m.mtime(), m.mtime_nsec(), m.ino())),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File {
            path,
            bytes: None,
            mode: 0o600,
            stamp: None,
        }),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn parse(f: &File, rel: &Path) -> Result<Value> {
    let Some(b) = &f.bytes else {
        return Ok(json!({}));
    };
    if b.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_slice(b).with_context(|| {
        format!(
            "~/{} is not plain JSON; paddock does not change it",
            rel.display()
        )
    })?;
    if !v.is_object() {
        bail!(
            "~/{} is not a JSON object; paddock does not change it",
            rel.display()
        );
    }
    Ok(v)
}

/// Write `bytes` in place of the file: a new file beside it, then a
/// rename. The file must not have changed since `f` was read.
pub(super) fn write(f: &File, bytes: &[u8]) -> Result<()> {
    let now = std::fs::symlink_metadata(&f.path)
        .ok()
        .map(|m| (m.len(), m.mtime(), m.mtime_nsec(), m.ino()));
    if now != f.stamp {
        bail!(
            "{} changed while paddock changed it; nothing was written, run the command again",
            f.path.display()
        );
    }
    let dir = f.path.parent().context("a hook file has a folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.paddock-{}",
        f.path.file_name().unwrap_or_default().to_string_lossy(),
        crate::horse::hex(&crate::horse::random_bytes::<6>()?)
    ));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(f.mode)
        .open(&tmp)?;
    let r = out
        .write_all(bytes)
        .and_then(|_| out.sync_all())
        .and_then(|_| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(f.mode)))
        .and_then(|_| std::fs::rename(&tmp, &f.path));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("cannot write {}", f.path.display()));
    }
    Ok(())
}
