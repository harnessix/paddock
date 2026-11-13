//! The backups of a store: `backups/<backup id>/` with `meta.json` and
//! `files/`.
//!
//! - `start of run`: every file of the project at the start of a run. It
//!   is also the base that an apply compares with.
//! - `before apply` and `before restore`: the files that the action
//!   replaced or removed, and a record of the files that it added. So
//!   each write into the original has an undo.
//!
//! Only the host writes a backup. Where the file system can share data, a
//! backup costs no bytes until the project changes.

use super::run::Synced;
use super::store::Store;
use super::tree::{self, Entry, Method, Side};
use crate::snapshot::Skip;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

const META: &str = "meta.json";
const FILES: &str = "files";
/// Backups of the start of a run that a store keeps, besides those that a
/// copy still needs.
pub const KEEP_START: usize = 10;
/// Backups from before an apply or a restore that a store keeps.
pub const KEEP_BEFORE: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Start,
    Apply,
    Restore,
}

impl Kind {
    /// The words for lists.
    pub fn words(self) -> &'static str {
        match self {
            Kind::Start => "start of run",
            Kind::Apply => "before apply",
            Kind::Restore => "before restore",
        }
    }

    /// The word in an id and in JSON.
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Start => "start",
            Kind::Apply => "apply",
            Kind::Restore => "restore",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub format: u32,
    pub id: String,
    pub kind: Kind,
    pub created: u64,
    /// Nanoseconds since 1970: the order of backups of one second.
    #[serde(default)]
    pub stamp: u64,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub harness: Option<String>,
    pub project: PathBuf,
    pub method: Method,
    /// Bytes of the files, as the project had them.
    pub bytes: u64,
    /// Every file of the project (`start`), or only the files that the
    /// action touched.
    pub full: bool,
    #[serde(default)]
    pub skip: Skip,
    /// The state before. None: the path did not exist.
    pub files: BTreeMap<String, Option<Entry>>,
    /// What the action left at each path, with a hash. An undo restores
    /// only a path that still looks like this.
    #[serde(default)]
    pub after: BTreeMap<String, Option<Entry>>,
    /// The sync records of the run from before an apply (run.rs).
    #[serde(default)]
    pub synced: BTreeMap<String, Option<Synced>>,
    /// When `paddock undo` took this action back.
    #[serde(default)]
    pub undone: Option<u64>,
    /// An undo made this backup. `paddock undo` does not take an undo back.
    #[serde(default)]
    pub by_undo: bool,
}

#[derive(Debug, Clone)]
pub struct Backup {
    pub dir: PathBuf,
    pub meta: Meta,
}

fn stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn new_id(kind: Kind, now: u64) -> String {
    let nonce = crate::broker::new_request_id();
    let hex = crate::manifest::hex(&blake3::hash(nonce.as_bytes()).as_bytes()[..3]);
    format!("{now}-{}-{hex}", kind.tag())
}

impl Backup {
    pub fn load(dir: &Path) -> Result<Backup> {
        let f = dir.join(META);
        let text =
            std::fs::read_to_string(&f).with_context(|| format!("cannot read {}", f.display()))?;
        let meta: Meta =
            serde_json::from_str(&text).with_context(|| format!("{} is invalid", f.display()))?;
        if meta.format > super::FORMAT {
            bail!(
                "{} has format {}, which a newer paddock wrote. Update paddock",
                f.display(),
                meta.format
            );
        }
        Ok(Backup {
            dir: dir.to_path_buf(),
            meta,
        })
    }

    pub fn save(&self) -> Result<()> {
        crate::privfs::write_atomic(&self.dir.join(META), serde_json::to_vec(&self.meta)?)?;
        Ok(())
    }

    pub fn files_dir(&self) -> PathBuf {
        self.dir.join(FILES)
    }

    /// The content of `rel` in the backup.
    pub fn file(&self, rel: &str) -> Option<File> {
        Side::open(&self.files_dir()).ok()?.file(rel)
    }

    /// Files and symlinks that the backup holds.
    pub fn count(&self) -> usize {
        self.meta.files.values().filter(|e| e.is_some()).count()
    }

    /// The manifest of a full backup.
    pub fn manifest(&self) -> tree::Manifest {
        self.meta
            .files
            .iter()
            .filter_map(|(p, e)| e.clone().map(|e| (p.clone(), e)))
            .collect()
    }

    /// `paddock undo` can take this action back.
    pub fn undoable(&self) -> bool {
        self.meta.kind != Kind::Start && self.meta.undone.is_none() && !self.meta.by_undo
    }
}

/// The backups of a store, newest first.
pub fn list(store: &Store) -> Vec<Backup> {
    let mut out: Vec<Backup> = std::fs::read_dir(store.backups())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| Backup::load(&e.path()).ok())
        .collect();
    out.sort_by(|a, b| {
        (b.meta.created, b.meta.stamp, &b.meta.id).cmp(&(a.meta.created, a.meta.stamp, &a.meta.id))
    });
    out
}

/// The backup that `input` names: its id, or the start of its last part.
pub fn find(store: &Store, input: &str) -> Result<Backup> {
    let all = list(store);
    match crate::ids::find(input, &all, |b| &b.meta.id) {
        Ok(Some(b)) => Ok(b.clone()),
        Ok(None) => bail!("no backup {input}. `paddock backups` lists them"),
        Err(e) => bail!("{}", format!("{e:#}").replace("sandbox id", "backup id")),
    }
}

/// A new backup folder, hidden until `publish`.
fn stage(store: &Store, kind: Kind) -> Result<(String, PathBuf)> {
    let id = new_id(kind, crate::state::now());
    let tmp = store.backups().join(format!(".{id}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    crate::privfs::dir(&tmp.join(FILES))?;
    Ok((id, tmp))
}

fn publish(store: &Store, tmp: &Path, meta: Meta) -> Result<Backup> {
    let dir = store.backups().join(&meta.id);
    let b = Backup {
        dir: tmp.to_path_buf(),
        meta,
    };
    b.save()?;
    std::fs::rename(tmp, &dir)?;
    Ok(Backup { dir, meta: b.meta })
}

/// The backup at the start of a run: the files of `project` that the scan
/// `found` with the rules `skip`. Without shared data, a project above `max` bytes is
/// refused.
pub fn start(
    store: &Store,
    project: &Path,
    found: &tree::Scan,
    run: (&str, &str),
    skip: &Skip,
    max: u64,
) -> Result<Backup> {
    let total: u64 = found.files.values().map(|e| e.size).sum();
    let (id, tmp) = stage(store, Kind::Start)?;
    let made = (|| -> Result<Meta> {
        let files_dir = tmp.join(FILES);
        let shared = tree::shares(project, &tmp, &found.files);
        if !shared && total > max {
            bail!(
                "this file system cannot share file data, and a copy of the project ({}) passes the limit [copy] max = {}. Start with --direct to work in the folder itself, or raise [copy] max in the user configuration",
                crate::storage::human(total),
                crate::storage::human(max)
            );
        }
        tree::clone_files(project, &files_dir, &found.files, shared)?;
        // The record is what the backup holds, not what the first scan saw:
        // a file can change between the two.
        let files = tree::scan(&files_dir, &Skip::default())?.files;
        Ok(Meta {
            format: super::FORMAT,
            id,
            kind: Kind::Start,
            created: crate::state::now(),
            stamp: stamp(),
            run: Some(run.0.to_string()),
            harness: Some(run.1.to_string()),
            project: project.to_path_buf(),
            method: Method::of(shared),
            bytes: files.values().map(|e| e.size).sum(),
            full: true,
            skip: skip.clone(),
            files: files.into_iter().map(|(p, e)| (p, Some(e))).collect(),
            after: BTreeMap::new(),
            synced: BTreeMap::new(),
            undone: None,
            by_undo: false,
        })
    })();
    match made.and_then(|m| publish(store, &tmp, m)) {
        Ok(b) => Ok(b),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

/// The backup before an apply or a restore: the state of `paths` in
/// `project` now. The caller adds `after` and saves again.
pub fn before(
    store: &Store,
    kind: Kind,
    project: &Path,
    run: Option<(&str, &str)>,
    paths: &BTreeSet<String>,
) -> Result<Backup> {
    let side = Side::open(project)?;
    let (id, tmp) = stage(store, kind)?;
    let made = (|| -> Result<Meta> {
        let mut files: BTreeMap<String, Option<Entry>> = BTreeMap::new();
        let mut have = tree::Manifest::new();
        for p in paths {
            let e = side.entry(p);
            if let Some(e) = &e {
                have.insert(p.clone(), e.clone());
            }
            files.insert(p.clone(), e);
        }
        let files_dir = tmp.join(FILES);
        let shared = tree::shares(project, &tmp, &have);
        let cloned = tree::clone_files(project, &files_dir, &have, shared)?;
        // A file that went away since the look: it did not exist.
        for p in &cloned.gone {
            files.insert(p.clone(), None);
        }
        Ok(Meta {
            format: super::FORMAT,
            id,
            kind,
            created: crate::state::now(),
            stamp: stamp(),
            run: run.map(|r| r.0.to_string()),
            harness: run.map(|r| r.1.to_string()),
            project: project.to_path_buf(),
            method: Method::of(shared),
            bytes: cloned.bytes,
            full: false,
            skip: Skip::default(),
            files,
            after: BTreeMap::new(),
            synced: BTreeMap::new(),
            undone: None,
            by_undo: false,
        })
    })();
    match made.and_then(|m| publish(store, &tmp, m)) {
        Ok(b) => Ok(b),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

/// Remove old backups. A start backup that a copy in `needed` compares
/// with always stays.
pub fn prune(store: &Store, needed: &BTreeSet<String>) {
    let (mut starts, mut befores) = (0, 0);
    for b in list(store) {
        let n = match b.meta.kind {
            Kind::Start => &mut starts,
            _ => &mut befores,
        };
        *n += 1;
        let keep = match b.meta.kind {
            Kind::Start => *n <= KEEP_START || needed.contains(&b.meta.id),
            _ => *n <= KEEP_BEFORE,
        };
        if !keep {
            let _ = crate::shadow::remove_dir(&b.dir);
        }
    }
}
