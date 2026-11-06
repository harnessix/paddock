//! Backups of the disk pair of a profile. A backup is the folder
//! `backups/<reason>-<time>/` with `data.img`, `system.img`, `aux.img`,
//! `machine-id`, the `stamp.json` of that moment and `backup.json`. Every
//! file is a clone (`clonefile` on macOS, a copy on Linux, the `clone`
//! closure of the caller), mode 0600 in a 0700 folder, written as
//! `.partial` and renamed. A data disk alone would be unreadable after any
//! loss of its system disk, so no backup holds the data disk alone. Plain
//! code: the tests run on every system.

use super::golden::used_bytes;
use super::layout::{commit, partial, remove_any, safe_name};
use super::session::{Profile, Stamp};
use super::text;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The file of a backup folder that describes it.
pub const META: &str = "backup.json";
/// The automatic backups that `retain` keeps per profile.
pub const KEEP_AUTO: usize = 3;
/// The oldest automatic backup that `retain` keeps.
pub const MAX_AGE_SECS: u64 = 30 * 24 * 3600;
/// The files of the pair in a backup, in the order they are cloned.
pub const FILES: [&str; 4] = ["data.img", "system.img", "aux.img", "machine-id"];

/// A clone of `src` as the new file `dst`.
pub type Clone<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

/// `backup.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub reason: String,
    /// Seconds since the epoch.
    pub time: u64,
    /// Made by `paddock app backup`: never removed by `retain`.
    #[serde(default)]
    pub manual: bool,
    /// The used bytes of the data disk at the backup.
    #[serde(default)]
    pub bytes: u64,
}

/// One backup, as `list` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub dir: PathBuf,
    pub name: String,
    pub meta: Meta,
    /// A `.img` file of the data disk alone, from before the disk pair.
    pub data_only: bool,
}

impl Backup {
    /// `2026-11-05 10:00`, from the time of the meta.
    pub fn when(&self) -> String {
        let t = crate::waiting::format_rfc3339(self.meta.time);
        t.get(..16).map(|s| s.replace('T', " ")).unwrap_or(t)
    }
}

/// What `backup` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Made(PathBuf),
    /// The volume lacks the used size of the data disk.
    Skipped {
        needed: u64,
        free: u64,
    },
    /// The profile has no data disk.
    Nothing,
}

/// The folder name of a backup: `<reason>-<time>`.
pub fn name(reason: &str, now: u64) -> String {
    format!(
        "{}-{}",
        safe_name(reason),
        crate::waiting::format_rfc3339(now).replace(':', "-")
    )
}

fn clone_into(profile: &Profile, into: &Path, clone: Clone) -> Result<()> {
    for f in FILES {
        let src = profile.dir.join(f);
        let dst = into.join(f);
        clone(&src, &dst).with_context(|| text::cannot_clone(&src, &dst))?;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600))?;
    }
    if let Ok(stamp) = std::fs::read(profile.stamp()) {
        let dst = into.join("stamp.json");
        std::fs::write(&dst, stamp).with_context(|| text::cannot_write(&dst))?;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn write_meta(dir: &Path, meta: &Meta) -> Result<()> {
    let p = dir.join(META);
    crate::privfs::write(&p, serde_json::to_string_pretty(meta)?)
        .with_context(|| text::cannot_write(&p))?;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn new_dir(dir: &Path) -> Result<()> {
    let _ = remove_any(dir);
    std::fs::create_dir_all(dir).with_context(|| text::cannot_make(dir))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// A backup of the pair into `backups/<reason>-<time>/`, skipped when
/// `free` is below the used size of the data disk. A profile with a data
/// disk but no system disk has no pair, and the error says so.
pub fn backup(
    profile: &Profile,
    reason: &str,
    manual: bool,
    now: u64,
    free: u64,
    clone: Clone,
) -> Result<Outcome> {
    if std::fs::symlink_metadata(profile.data()).is_err() {
        return Ok(Outcome::Nothing);
    }
    for f in &FILES[1..] {
        if std::fs::symlink_metadata(profile.dir.join(f)).is_err() {
            bail!(text::backup_incomplete(&profile.dir, f));
        }
    }
    let needed = used_bytes(&profile.data());
    if free < needed {
        return Ok(Outcome::Skipped { needed, free });
    }
    let backups = profile.backups();
    std::fs::create_dir_all(&backups).with_context(|| text::cannot_make(&backups))?;
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o700))?;
    let dir = backups.join(name(reason, now));
    let tmp = partial(&dir);
    new_dir(&tmp)?;
    clone_into(profile, &tmp, clone)?;
    write_meta(
        &tmp,
        &Meta {
            reason: reason.into(),
            time: now,
            manual,
            bytes: needed,
        },
    )?;
    commit(&tmp).with_context(|| text::cannot_commit(&dir))?;
    Ok(Outcome::Made(dir))
}

/// `paddock app backup NAME --out DIR`: the pair into a folder of the
/// user's choice, which must not exist.
pub fn backup_to(profile: &Profile, out: &Path, now: u64, clone: Clone) -> Result<()> {
    if std::fs::symlink_metadata(out).is_ok() {
        bail!(text::exists_already(out));
    }
    for f in FILES {
        if std::fs::symlink_metadata(profile.dir.join(f)).is_err() {
            bail!(text::backup_incomplete(&profile.dir, f));
        }
    }
    let tmp = partial(out);
    new_dir(&tmp)?;
    clone_into(profile, &tmp, clone)?;
    write_meta(
        &tmp,
        &Meta {
            reason: "manual".into(),
            time: now,
            manual: true,
            bytes: used_bytes(&profile.data()),
        },
    )?;
    commit(&tmp).with_context(|| text::cannot_commit(out))
}

/// Read one backup folder or data-only file.
pub fn read(path: &Path) -> Option<Backup> {
    let name = path.file_name()?.to_str()?.to_string();
    if super::layout::is_partial(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if meta.is_file() {
        if !name.ends_with(".img") {
            return None;
        }
        return Some(Backup {
            dir: path.to_path_buf(),
            name,
            meta: Meta {
                reason: "data".into(),
                time: mtime,
                manual: false,
                bytes: used_bytes(path),
            },
            data_only: true,
        });
    }
    let m: Meta = std::fs::read_to_string(path.join(META))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Meta {
            reason: "unknown".into(),
            time: mtime,
            manual: false,
            bytes: used_bytes(&path.join("data.img")),
        });
    Some(Backup {
        dir: path.to_path_buf(),
        name,
        meta: m,
        data_only: false,
    })
}

/// Every backup of a profile, oldest first.
pub fn list(profile: &Profile) -> Vec<Backup> {
    let mut out: Vec<Backup> = std::fs::read_dir(profile.backups())
        .map(|rd| rd.flatten().filter_map(|e| read(&e.path())).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| a.meta.time.cmp(&b.meta.time).then(a.name.cmp(&b.name)));
    out
}

/// The newest backup of the pair.
pub fn newest(profile: &Profile) -> Option<Backup> {
    list(profile).into_iter().rev().find(|b| !b.data_only)
}

/// A backup by its folder, its name, or a part of its name (a time).
pub fn find(profile: &Profile, which: &str) -> Option<Backup> {
    let p = Path::new(which);
    if p.is_absolute() || which.contains('/') {
        return read(p);
    }
    let all = list(profile);
    if let Some(b) = all.iter().find(|b| b.name == which) {
        return Some(b.clone());
    }
    let hits: Vec<&Backup> = all.iter().filter(|b| b.name.contains(which)).collect();
    match hits.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

/// Put a backup in place of the pair of the profile. The pair and the
/// stamp of before go to `backups/pre-restore-<time>/` as renames. The
/// stamp of the backup comes back with `state_id` cleared, so the next
/// start is a cold boot. Returns the folder of the pair from before.
pub fn restore(profile: &Profile, from: &Backup, now: u64, clone: Clone) -> Result<PathBuf> {
    if from.data_only {
        bail!(text::restore_refused_data_only(&from.dir));
    }
    for f in FILES {
        if std::fs::symlink_metadata(from.dir.join(f)).is_err() {
            bail!(text::backup_incomplete(&from.dir, f));
        }
    }
    let backups = profile.backups();
    std::fs::create_dir_all(&backups).with_context(|| text::cannot_make(&backups))?;
    let aside = backups.join(name("pre-restore", now));
    let tmp = partial(&aside);
    new_dir(&tmp)?;
    let mut bytes = 0;
    for p in profile.pair() {
        if std::fs::symlink_metadata(&p).is_ok() {
            if p == profile.data() {
                bytes = used_bytes(&p);
            }
            let dst = tmp.join(p.file_name().unwrap_or_default());
            std::fs::rename(&p, &dst).with_context(|| text::cannot_move(&p, &dst))?;
        }
    }
    write_meta(
        &tmp,
        &Meta {
            reason: "pre-restore".into(),
            time: now,
            manual: false,
            bytes,
        },
    )?;
    commit(&tmp).with_context(|| text::cannot_commit(&aside))?;
    for f in FILES {
        let src = from.dir.join(f);
        let dst = profile.dir.join(f);
        let p = partial(&dst);
        let _ = remove_any(&p);
        clone(&src, &p).with_context(|| text::cannot_clone(&src, &p))?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        commit(&p).with_context(|| text::cannot_commit(&dst))?;
    }
    if let Ok(mut stamp) = Stamp::read(&from.dir.join("stamp.json")) {
        stamp.state_id = None;
        stamp.state_len = None;
        stamp.saved = None;
        stamp.clean = true;
        stamp.write(&profile.stamp())?;
    }
    Ok(aside)
}

/// The automatic backups that the retire rule removes: every
/// one beyond the newest [`KEEP_AUTO`], and every one older than
/// [`MAX_AGE_SECS`]. Manual backups never. Pure over the list.
pub fn retire_plan(all: &[Backup], now: u64) -> Vec<Backup> {
    let mut auto: Vec<&Backup> = all.iter().filter(|b| !b.meta.manual).collect();
    auto.sort_by_key(|b| std::cmp::Reverse(b.meta.time));
    let mut out: Vec<Backup> = auto
        .iter()
        .enumerate()
        .filter(|(i, b)| *i >= KEEP_AUTO || now.saturating_sub(b.meta.time) > MAX_AGE_SECS)
        .map(|(_, b)| (*b).clone())
        .collect();
    out.sort_by(|a, b| a.meta.time.cmp(&b.meta.time).then(a.name.cmp(&b.name)));
    out
}

/// Remove what `retire_plan` names. Returns the paths that went.
pub fn retain(profile: &Profile, now: u64) -> Result<Vec<PathBuf>> {
    let mut gone = Vec::new();
    for b in retire_plan(&list(profile), now) {
        remove_any(&b.dir).with_context(|| text::cannot_remove(&b.dir))?;
        gone.push(b.dir);
    }
    Ok(gone)
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
