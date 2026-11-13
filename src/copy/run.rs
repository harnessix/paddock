//! The copy of one run: `runs/<run id>/` in the store.
//!
//! - `run.json`: the record. Only the host writes it.
//! - `tree/`: the working copy. The sandbox writes it.
//! - `sync/`: the content that an apply wrote into the original, by hash.
//!   The host stages it there before the write, so the write is what
//!   paddock compared, also when the agent changes the file at that moment.
//!
//! After an apply, the applied content is the new base of that path. So a
//! later edit of the user in the original is an edit of one side, not a
//! conflict.

use super::backup::Backup;
use super::store::Store;
use super::tree::{Entry, Method};
use super::Config;
use crate::env::HostEnv;
use crate::snapshot::Skip;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

const META: &str = "run.json";
pub const TREE: &str = "tree";
const SYNC: &str = "sync";

/// The state of one path after an apply: both sides held this content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Synced {
    /// The hash of the content, or `link:TARGET` for a symlink. None: the
    /// apply removed the file.
    pub hash: Option<String>,
    /// The file of the copy then.
    pub copy: Option<Entry>,
    /// The file of the original after the write.
    pub original: Option<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRec {
    pub format: u32,
    pub id: String,
    pub harness: String,
    /// The original folder.
    pub project: PathBuf,
    pub store: String,
    /// The backup from the start of the run.
    pub base: String,
    pub started: u64,
    #[serde(default)]
    pub ended: Option<u64>,
    /// When the working copy went away: discarded, or cleaned up.
    #[serde(default)]
    pub removed: Option<u64>,
    pub method: Method,
    pub files: usize,
    pub bytes: u64,
    pub skip: Skip,
    /// The project is a git work tree, and `.git` came along.
    #[serde(default)]
    pub git: bool,
    /// The sandbox sees the copy at the path of the project. False: at the
    /// path of the copy itself.
    #[serde(default)]
    pub in_place: bool,
    /// Parts outside the change list that came along: `.git`, build output.
    #[serde(default)]
    pub carried: Vec<String>,
    /// Paths that stay in the copy: an apply of everything leaves them out.
    #[serde(default)]
    pub kept: BTreeSet<String>,
    #[serde(default)]
    pub synced: BTreeMap<String, Synced>,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub dir: PathBuf,
    pub store: Store,
    pub rec: RunRec,
}

/// A run id becomes a folder name.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

impl Run {
    pub fn load(store: &Store, id: &str) -> Result<Run> {
        if !valid_id(id) {
            bail!("{id} is not a run id");
        }
        let dir = store.runs().join(id);
        let f = dir.join(META);
        let text =
            std::fs::read_to_string(&f).with_context(|| format!("cannot read {}", f.display()))?;
        let rec: RunRec =
            serde_json::from_str(&text).with_context(|| format!("{} is invalid", f.display()))?;
        if rec.format > super::FORMAT {
            bail!(
                "{} has format {}, which a newer paddock wrote. Update paddock",
                f.display(),
                rec.format
            );
        }
        Ok(Run {
            dir,
            store: store.clone(),
            rec,
        })
    }

    pub fn save(&self) -> Result<()> {
        crate::privfs::write_atomic(&self.dir.join(META), serde_json::to_vec(&self.rec)?)?;
        Ok(())
    }

    /// The working copy.
    pub fn tree(&self) -> PathBuf {
        self.dir.join(TREE)
    }

    pub fn has_tree(&self) -> bool {
        self.rec.removed.is_none()
            && std::fs::symlink_metadata(self.tree()).is_ok_and(|m| m.is_dir())
    }

    /// The backup from the start of the run.
    pub fn base(&self) -> Result<Backup> {
        Backup::load(&self.store.backups().join(&self.rec.base)).with_context(|| {
            format!(
                "the backup from the start of {} is gone, so paddock cannot tell its changes",
                self.rec.id
            )
        })
    }

    /// Put the content of `from` into `sync/`, and return its hash. The
    /// name is the hash, so the same content is one file.
    pub fn stash(&self, from: &mut File) -> Result<String> {
        let dir = self.dir.join(SYNC);
        crate::privfs::dir(&dir)?;
        let tmp = dir.join(format!(".tmp-{}", crate::broker::new_request_id()));
        let made = (|| -> Result<String> {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            std::io::copy(from, &mut out)?;
            drop(out);
            // The hash of the private file: nobody else can change it now.
            let hash = super::tree::hash(&mut File::open(&tmp)?)?;
            std::fs::rename(&tmp, dir.join(&hash))?;
            Ok(hash)
        })();
        if made.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        made
    }

    /// The content with `hash` in `sync/`.
    pub fn stashed(&self, hash: &str) -> Option<File> {
        if hash.is_empty() || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        File::open(self.dir.join(SYNC).join(hash)).ok()
    }

    /// The sandbox of this run is alive.
    pub fn running(&self, env: &HostEnv) -> bool {
        crate::state::Running::list(env)
            .iter()
            .any(|r| r.id == self.rec.id)
    }

    /// Remove the working copy and the staged content. The record stays.
    pub fn remove_tree(&mut self) -> Result<()> {
        crate::shadow::remove_dir(&self.tree())?;
        let _ = std::fs::remove_dir_all(self.dir.join(SYNC));
        self.rec.removed = Some(crate::state::now());
        self.save()
    }
}

/// The runs of one store, newest first.
pub fn of_store(store: &Store) -> Vec<Run> {
    let mut out: Vec<Run> = std::fs::read_dir(store.runs())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|id| Run::load(store, &id).ok())
        .collect();
    out.sort_by(|a, b| {
        b.rec
            .started
            .cmp(&a.rec.started)
            .then(b.rec.id.cmp(&a.rec.id))
    });
    out
}

/// Every run with a copy, newest first.
pub fn all(env: &HostEnv, cfg: &Config) -> Vec<Run> {
    let mut out: Vec<Run> = Store::all(&cfg.roots(env))
        .iter()
        .flat_map(of_store)
        .collect();
    out.sort_by(|a, b| {
        b.rec
            .started
            .cmp(&a.rec.started)
            .then(b.rec.id.cmp(&a.rec.id))
    });
    out
}

/// The run that `input` names: its id, or the start of the hex part.
pub fn find(env: &HostEnv, cfg: &Config, input: &str) -> Result<Run> {
    let runs = all(env, cfg);
    match crate::ids::find(input, &runs, |r| &r.rec.id)? {
        Some(r) => Ok(r.clone()),
        None => bail!(
            "no run {input} with a copy. `paddock changes` lists the runs that work on a copy"
        ),
    }
}

/// The run `id`, when it works on a copy.
pub fn of_id(env: &HostEnv, id: &str) -> Option<Run> {
    if !valid_id(id) {
        return None;
    }
    let cfg = Config::load(env).unwrap_or_default();
    Store::all(&cfg.roots(env))
        .iter()
        .find_map(|s| Run::load(s, id).ok())
}

/// The copy that the sandbox `id` writes, for `paddock ps`: the original
/// folder and the working copy.
pub fn summary(env: &HostEnv, id: &str) -> Option<(PathBuf, PathBuf)> {
    of_id(env, id).map(|r| (r.rec.project.clone(), r.tree()))
}

/// The start backups that a working copy still compares with.
pub fn needed_bases(store: &Store) -> BTreeSet<String> {
    of_store(store)
        .into_iter()
        .filter(|r| r.has_tree())
        .map(|r| r.rec.base)
        .collect()
}

/// A path that names the working copy of a run, for messages.
pub fn tree_of(dir: &Path) -> PathBuf {
    dir.join(TREE)
}

/// The original folder of the run whose working copy is `tree`. A run at
/// its own path works in `tree` as its project.
pub fn original_of_tree(env: &HostEnv, tree: &Path) -> Option<PathBuf> {
    if tree.file_name()? != TREE {
        return None;
    }
    let run_dir = tree.parent()?;
    let id = run_dir.file_name()?.to_str()?;
    let store_dir = run_dir.parent()?.parent()?;
    let store_id = store_dir.file_name()?.to_str()?;
    let cfg = Config::load(env).unwrap_or_default();
    if !cfg
        .roots(env)
        .iter()
        .any(|r| Some(r.as_path()) == store_dir.parent())
    {
        return None;
    }
    let store = Store::open(store_dir.parent()?, store_id).ok()??;
    let run = Run::load(&store, id).ok()?;
    (run.tree() == tree).then_some(run.rec.project)
}
