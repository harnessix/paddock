//! Where manifests and artifacts live. A transport is one index; the first
//! is a folder on disk:
//!
//! - `manifests/<org>/<name>/<version>.json`: one manifest per version.
//! - `objects/<hash>`: one artifact per hash.
//!
//! A git checkout of such a folder is a git-backed index as it is.
//!
//! A version is immutable: a second publish with another artifact is
//! refused, the same manifest again is a no-op. Every file is written to a
//! temporary name, synced, and linked to its final name, which never
//! replaces a file that is there, so two publishers of one version cannot
//! both win.

use super::manifest::Manifest;
use super::reference::{compare_versions, valid_ident, valid_version};
use super::{artifact_hash, check_artifact};
use crate::horse::Hash;
use crate::privfs;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

pub trait Transport {
    /// The name of the index, for messages and for `Resolved::index`.
    fn name(&self) -> &str;
    fn manifest(&self, org: &str, name: &str, version: &str) -> Result<Option<Manifest>>;
    /// The versions of `org/name`, lowest first. Empty when it has none.
    fn versions(&self, org: &str, name: &str) -> Result<Vec<String>>;
    fn object(&self, h: &Hash) -> Result<Option<Vec<u8>>>;
    fn put(&self, m: &Manifest, bytes: &[u8]) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct FileIndex {
    pub name: String,
    pub root: PathBuf,
    protected: Option<crate::horse::Store>,
}

fn check_path(org: &str, name: &str) -> Result<()> {
    if !valid_ident(org) || !valid_ident(name) {
        bail!("{org}/{name} is not ORG/NAME in lower-case letters, digits and dashes");
    }
    Ok(())
}

fn read_opt(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Write `bytes` to `path` only when nothing is there: a synced temporary
/// file in the same folder, then a hard link. Returns false when a file
/// was there already; the caller compares it.
fn write_once(path: &Path, bytes: &[u8]) -> Result<bool> {
    let dir = path.parent().expect("an index path has a folder");
    privfs::dir(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        crate::horse::hex(&crate::horse::random_bytes::<8>()?)
    ));
    let written = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    let linked = written.and_then(|()| std::fs::hard_link(&tmp, path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {
            crate::horse::store::sync_dir(dir)?;
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot write {}", path.display())),
    }
}

impl FileIndex {
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> FileIndex {
        FileIndex {
            name: name.into(),
            root: root.into(),
            protected: None,
        }
    }

    fn read_record(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let Some(bytes) = read_opt(path)? else {
            return Ok(None);
        };
        match &self.protected {
            Some(store) => {
                let rel = path.strip_prefix(&self.root)?.to_string_lossy();
                Ok(Some(store.open_registry(&rel, &bytes)?))
            }
            None => Ok(Some(bytes)),
        }
    }

    fn write_record(&self, path: &Path, bytes: &[u8]) -> Result<bool> {
        match &self.protected {
            Some(store) => {
                let rel = path.strip_prefix(&self.root)?.to_string_lossy();
                write_once(path, &store.seal_registry(&rel, bytes)?)
            }
            None => write_once(path, bytes),
        }
    }

    fn manifest_path(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.root
            .join("manifests")
            .join(org)
            .join(name)
            .join(format!("{version}.json"))
    }

    fn object_path(&self, h: &Hash) -> PathBuf {
        self.root.join("objects").join(h.as_str())
    }

    /// Every `(org, name)` with a manifest folder, sorted. A folder that
    /// is not a valid org or name is left out.
    pub fn names(&self) -> Result<Vec<(String, String)>> {
        let list = |d: &Path| -> Result<Vec<String>> {
            let entries = match std::fs::read_dir(d) {
                Ok(e) => e,
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(e).with_context(|| format!("cannot list {}", d.display())),
            };
            let mut v = Vec::new();
            for e in entries {
                let e = e.with_context(|| format!("cannot list {}", d.display()))?;
                if let Some(n) = e.file_name().to_str().filter(|n| valid_ident(n)) {
                    if e.path().is_dir() {
                        v.push(n.to_string());
                    }
                }
            }
            v.sort();
            Ok(v)
        };
        let root = self.root.join("manifests");
        let mut out = Vec::new();
        for org in list(&root)? {
            for name in list(&root.join(&org))? {
                out.push((org.clone(), name));
            }
        }
        Ok(out)
    }
}

impl Transport for FileIndex {
    fn name(&self) -> &str {
        &self.name
    }

    fn manifest(&self, org: &str, name: &str, version: &str) -> Result<Option<Manifest>> {
        check_path(org, name)?;
        if !valid_version(version) {
            bail!("{version:?} is not a version");
        }
        let path = self.manifest_path(org, name, version);
        let Some(b) = self.read_record(&path)? else {
            return Ok(None);
        };
        Manifest::parse(&b)
            .with_context(|| format!("index {}: {}", self.name, path.display()))
            .map(Some)
    }

    fn versions(&self, org: &str, name: &str) -> Result<Vec<String>> {
        check_path(org, name)?;
        let dir = self.root.join("manifests").join(org).join(name);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot list {}", dir.display())),
        };
        let mut v = Vec::new();
        for e in entries {
            let e = e.with_context(|| format!("cannot list {}", dir.display()))?;
            let file = e.file_name();
            // Temporary files and anything else that is not a version are
            // not listed.
            if let Some(ver) = file.to_str().and_then(|f| f.strip_suffix(".json")) {
                if valid_version(ver) {
                    v.push(ver.to_string());
                }
            }
        }
        v.sort_by(|a, b| compare_versions(a, b));
        Ok(v)
    }

    fn object(&self, h: &Hash) -> Result<Option<Vec<u8>>> {
        self.read_record(&self.object_path(h))
    }

    fn put(&self, m: &Manifest, bytes: &[u8]) -> Result<()> {
        let (org, name) = m.org_name()?;
        if !valid_version(&m.version) {
            bail!("{:?} is not a version that can be published", m.version);
        }
        check_artifact(m, bytes)?;
        let obj = self.object_path(&m.hash);
        if !self.write_record(&obj, bytes)? {
            let there = self
                .read_record(&obj)?
                .with_context(|| format!("cannot read {}", obj.display()))?;
            if artifact_hash(&there) != m.hash {
                bail!(
                    "index {}: {} is damaged: its content does not have that hash; remove it and publish again",
                    self.name,
                    obj.display()
                );
            }
        }
        let path = self.manifest_path(org, name, &m.version);
        if self.write_record(&path, &m.encode())? {
            return Ok(());
        }
        let there = self
            .read_record(&path)?
            .with_context(|| format!("cannot read {}", path.display()))?;
        let old = Manifest::parse(&there)
            .with_context(|| format!("index {}: {}", self.name, path.display()))?;
        if old.hash != m.hash {
            bail!(
                "index {}: {}@{} is already published with the artifact {}, not {}; a version cannot change, publish a new version",
                self.name,
                m.name,
                m.version,
                old.hash,
                m.hash
            );
        }
        if old != *m {
            bail!(
                "index {}: {}@{} is already published with this artifact but other metadata (author, signature, parents or time); a version cannot change",
                self.name,
                m.name,
                m.version
            );
        }
        Ok(())
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    #[serde(default)]
    pub(super) index: Vec<IndexEntry>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IndexEntry {
    pub(super) name: String,
    pub(super) path: PathBuf,
}

/// The indexes of `<state_dir>/registry/registry.toml`, in order. Without
/// the file: one index `local` at `<state_dir>/registry/local`.
pub fn open_indexes(state_dir: &Path) -> Result<Vec<Box<dyn Transport>>> {
    Ok(configured(state_dir)?
        .into_iter()
        .map(|i| Box::new(i) as Box<dyn Transport>)
        .collect())
}

/// The default local index is private horse content. User-configured external
/// indexes retain their transport format for sharing with other hosts.
pub fn configured_for_store(
    state_dir: &Path,
    store: &crate::horse::Store,
) -> Result<Vec<FileIndex>> {
    let mut indexes = configured(state_dir)?;
    let local = state_dir.join("registry/local");
    for index in &mut indexes {
        if index.root == local {
            index.protected = Some(store.clone());
        }
    }
    Ok(indexes)
}

pub fn open_indexes_for_store(
    state_dir: &Path,
    store: &crate::horse::Store,
) -> Result<Vec<Box<dyn Transport>>> {
    Ok(configured_for_store(state_dir, store)?
        .into_iter()
        .map(|i| Box::new(i) as Box<dyn Transport>)
        .collect())
}

/// The folder indexes of `open_indexes`, with their paths.
pub fn configured(state_dir: &Path) -> Result<Vec<FileIndex>> {
    let dir = state_dir.join("registry");
    let file = dir.join("registry.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(vec![FileIndex::new("local", dir.join("local"))]);
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    let c: Config =
        toml::from_str(&text).with_context(|| format!("{} is not valid", file.display()))?;
    if c.index.is_empty() {
        bail!(
            "{} lists no index: add [[index]] with name and path, or remove the file to use the local index",
            file.display()
        );
    }
    let mut out: Vec<FileIndex> = Vec::new();
    for (i, e) in c.index.into_iter().enumerate() {
        if e.name.trim().is_empty() {
            bail!("{}: index {} has an empty name", file.display(), i + 1);
        }
        if !e.path.is_absolute() {
            bail!(
                "{}: the path {} of index {} must be absolute",
                file.display(),
                e.path.display(),
                e.name
            );
        }
        if out.iter().any(|t| t.name == e.name) {
            bail!(
                "{}: the index name {} is used twice",
                file.display(),
                e.name
            );
        }
        out.push(FileIndex::new(e.name, e.path));
    }
    Ok(out)
}
