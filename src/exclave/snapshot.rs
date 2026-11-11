//! Content-addressed project snapshots. No transport or peer access occurs here.
//! Callers must pass all effective `[secrets] hide` paths in `hidden`.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const MAX_BLOB: u64 = 256 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 50_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub hash: String,
    pub size: u64,
    pub executable: bool,
    /// Source modification time in nanoseconds since the Unix epoch.
    #[serde(default)]
    pub modified_ns: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub files: BTreeMap<String, Entry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
}
#[derive(Debug)]
pub struct Snapshot {
    pub manifest: Manifest,
    pub blobs: BTreeMap<String, Vec<u8>>,
}

fn path(s: &str) -> Result<&Path> {
    let p = Path::new(s);
    if s.is_empty()
        || s.split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c == ".git" || c == ".paddock")
        || !p.components().all(|c| matches!(c, Component::Normal(_)))
    {
        bail!("invalid snapshot path");
    }
    Ok(p)
}
pub(crate) fn permitted(s: &str, hidden: &[PathBuf]) -> Result<bool> {
    let p = path(s)?;
    for h in hidden {
        if h.is_absolute() || h.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!("hidden paths must be project-relative");
        }
    }
    Ok(!p.components().any(|c| match c {
        Component::Normal(n) => crate::detect::is_secret_name(&n.to_string_lossy()),
        _ => true,
    }) && !hidden.iter().any(|h| p.starts_with(h)))
}
fn owned(m: &std::fs::Metadata) -> Result<()> {
    // SAFETY: geteuid has no arguments and no side effects.
    if m.uid() != unsafe { libc::geteuid() } {
        bail!("file belongs to another user");
    }
    Ok(())
}
fn read_file(root: &crate::safefs::Root, rel: &str) -> Result<(Vec<u8>, bool, i64)> {
    let p = path(rel)?;
    let mut parent = PathBuf::new();
    for c in p.parent().unwrap_or(Path::new("")).components() {
        parent.push(c.as_os_str());
        let d = root.dir(&parent, false)?;
        owned(&std::fs::metadata(d.real_path()?)?)?;
    }
    let dir = root.dir(p.parent().unwrap_or(Path::new("")), false)?;
    let mut f = dir
        .open_read(p.file_name().unwrap())?
        .context("file missing")?;
    let m = f.metadata()?;
    owned(&m)?;
    if m.len() > MAX_BLOB {
        bail!("snapshot file exceeds size limit");
    }
    let mut bytes = Vec::new();
    (&mut f).take(MAX_BLOB + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BLOB {
        bail!("snapshot file exceeds size limit");
    }
    let end = f.metadata()?;
    if m.len() != end.len() || m.mtime() != end.mtime() || m.mtime_nsec() != end.mtime_nsec() {
        bail!("snapshot file changed during capture");
    }
    let modified = m
        .mtime()
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(m.mtime_nsec()))
        .context("invalid modification time")?;
    Ok((bytes, m.mode() & 0o111 != 0, modified))
}
fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
pub(crate) fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.files.len() + self.links.len() > MAX_FILES {
            bail!("snapshot has too many files");
        }
        let mut sizes = BTreeMap::new();
        for (p, e) in &self.files {
            if e.size > MAX_BLOB {
                bail!("snapshot file exceeds size limit");
            }
            if !permitted(p, &[])? {
                bail!("credential path in manifest");
            }
            if !valid_hash(&e.hash) {
                bail!("invalid blob hash");
            }
            if sizes
                .insert(&e.hash, e.size)
                .is_some_and(|size| size != e.size)
            {
                bail!("conflicting blob sizes");
            }
        }
        let total = sizes
            .values()
            .try_fold(0u64, |sum, size| sum.checked_add(*size))
            .context("snapshot size overflow")?;
        if total > MAX_TOTAL {
            bail!("snapshot exceeds size limit");
        }
        for (name, target) in &self.links {
            if !permitted(name, &[])? || link_leaves(&self.links, name, target) {
                bail!("symlink {name} escapes the project or is invalid");
            }
        }
        for p in self.files.keys().chain(self.links.keys()) {
            if self.files.contains_key(p) && self.links.contains_key(p) {
                bail!("conflicting file paths");
            }
            let mut a = Path::new(p).parent();
            while let Some(q) = a {
                if let Some(q) = q.to_str() {
                    if self.files.contains_key(q) || self.links.contains_key(q) {
                        bail!("conflicting file paths");
                    }
                }
                a = q.parent();
            }
        }
        Ok(())
    }
    pub fn hash(&self) -> Result<String> {
        self.validate()?;
        Ok(digest(&serde_json::to_vec(self)?))
    }
}
fn link_leaves(links: &BTreeMap<String, String>, name: &str, target: &str) -> bool {
    let mut at: Vec<String> = name.split('/').map(str::to_owned).collect();
    at.pop();
    let mut todo = Vec::new();
    fn push(todo: &mut Vec<String>, target: &str) -> bool {
        let p = Path::new(target);
        if target.is_empty() || p.is_absolute() {
            return false;
        }
        for c in p.components().rev() {
            match c {
                Component::Normal(n) => todo.push(n.to_string_lossy().into_owned()),
                Component::ParentDir => todo.push("..".into()),
                Component::CurDir => {}
                _ => return false,
            }
        }
        true
    }
    if !push(&mut todo, target) {
        return true;
    }
    let mut hops = 0;
    while let Some(part) = todo.pop() {
        if part == ".." {
            if at.pop().is_none() {
                return true;
            }
            continue;
        }
        at.push(part);
        if let Some(next) = links.get(&at.join("/")) {
            hops += 1;
            if hops > 40 {
                return true;
            }
            at.pop();
            if !push(&mut todo, next) {
                return true;
            }
        }
    }
    false
}
fn build(
    root: &Path,
    names: impl IntoIterator<Item = String>,
    hidden: &[PathBuf],
    allow_links: bool,
) -> Result<Snapshot> {
    let fs = crate::safefs::Root::open(root)?;
    owned(&std::fs::symlink_metadata(root)?)?;
    let mut result = Snapshot {
        manifest: Manifest::default(),
        blobs: BTreeMap::new(),
    };
    let mut total = 0u64;
    for name in names {
        if !permitted(&name, hidden)? {
            continue;
        }
        let rel = path(&name)?;
        let dir = fs.dir(rel.parent().unwrap_or(Path::new("")), false)?;
        if allow_links
            && std::fs::symlink_metadata(root.join(rel))?
                .file_type()
                .is_symlink()
        {
            let target = dir.read_link(rel.file_name().unwrap())?;
            let target = target.to_str().context("non-UTF-8 symlink target")?;
            result.manifest.links.insert(name, target.to_owned());
            continue;
        }
        let (bytes, executable, modified_ns) = read_file(&fs, &name)?;
        let hash = digest(&bytes);
        let entry = Entry {
            hash: hash.clone(),
            size: bytes.len() as u64,
            executable,
            modified_ns,
        };
        if !result.blobs.contains_key(&hash) {
            total = total
                .checked_add(bytes.len() as u64)
                .context("snapshot size overflow")?;
            if total > MAX_TOTAL {
                bail!("snapshot exceeds size limit");
            }
        }
        result.blobs.insert(hash, bytes);
        result.manifest.files.insert(name, entry);
        if result.manifest.files.len() > MAX_FILES {
            bail!("snapshot has too many files");
        }
    }
    result.manifest.validate()?;
    Ok(result)
}
/// Tracked files and explicitly allowed untracked file paths. Ignored files,
/// including force-added tracked files, and credential names are excluded.
pub fn capture(
    project: &Path,
    allowed_untracked: &[String],
    hidden: &[PathBuf],
) -> Result<Snapshot> {
    if std::fs::symlink_metadata(project.join(".git"))
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        for name in allowed_untracked {
            path(name)?;
        }
        return plain::capture(project, hidden);
    }
    let Some(layout) = crate::hostgit::Layout::discover(project)? else {
        for name in allowed_untracked {
            path(name)?;
        }
        return plain::capture(project, hidden);
    };
    if project.canonicalize()? != layout.work_tree.canonicalize()? {
        bail!("snapshot project must be the worktree root");
    }
    layout.guard()?;
    let list = |args: &[&str]| -> Result<BTreeSet<String>> {
        let mut cmd = crate::hostgit::hardened(&layout.git_dir, Some(&layout.work_tree));
        cmd.current_dir(project).args(args);
        let (code, out, _) = crate::broker::run_capture(&mut cmd, Duration::from_secs(60))?;
        if code != 0 || out.len() >= 1024 * 1024 || out.contains('\u{fffd}') {
            bail!("git file selection failed or exceeded its safe limit");
        }
        Ok(out
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    };
    let tracked = list(&["ls-files", "-z", "--cached"])?;
    let untracked = list(&["ls-files", "-z", "--others", "--exclude-standard"])?;
    let mut names = tracked;
    for p in allowed_untracked {
        path(p)?;
        if untracked.contains(p) {
            names.insert(p.clone());
        }
    }
    let input = names.iter().map(|s| format!("{s}\0")).collect::<String>();
    let mut cmd = crate::hostgit::hardened(&layout.git_dir, Some(&layout.work_tree));
    cmd.current_dir(project)
        .args(["check-ignore", "--no-index", "-z", "--stdin"]);
    let (code, out, _) = crate::broker::run_capture_input(
        &mut cmd,
        Duration::from_secs(60),
        Some(input.as_bytes()),
    )?;
    if (code != 0 && code != 1) || out.len() >= 1024 * 1024 || out.contains('\u{fffd}') {
        bail!("git ignore check failed or exceeded its safe limit");
    }
    for ignored in out.split('\0').filter(|s| !s.is_empty()) {
        names.remove(ignored);
    }
    // Deleted tracked files are absent from the snapshot.
    names.retain(|p| std::fs::symlink_metadata(project.join(p)).is_ok());
    build(project, names, hidden, true)
}
/// Collect exactly the declared regular files from a completed workspace.
pub fn collect_outputs(
    project: &Path,
    declared: &[String],
    hidden: &[PathBuf],
) -> Result<Snapshot> {
    build(project, declared.iter().cloned(), hidden, false)
}

/// An isolated per-project blob store. Construct it with a directory chosen by
/// the host under the peer's private project data directory.
pub struct Store {
    root: crate::safefs::Root,
    dir: PathBuf,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        crate::privfs::dir(dir)?;
        owned(&std::fs::symlink_metadata(dir)?)?;
        Ok(Self {
            root: crate::safefs::Root::open(dir)?,
            dir: dir.to_path_buf(),
        })
    }
    pub(crate) fn directory(&self) -> &Path {
        &self.dir
    }
    pub fn put(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        if !valid_hash(hash) || digest(bytes) != hash {
            bail!("blob integrity mismatch");
        }
        self.root.write(Path::new(hash), bytes, 0o600)
    }
    pub fn get(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        if !valid_hash(hash) {
            bail!("invalid blob hash");
        }
        let data = self.root.read(Path::new(hash), MAX_BLOB)?;
        if let Some(ref b) = data {
            if digest(b) != hash {
                bail!("corrupt blob");
            }
        }
        Ok(data)
    }
    pub fn missing(&self, manifest: &Manifest) -> Result<BTreeSet<String>> {
        manifest.validate()?;
        let mut out = BTreeSet::new();
        for e in manifest.files.values() {
            match self.get(&e.hash)? {
                None => {
                    out.insert(e.hash.clone());
                }
                Some(b) if b.len() as u64 != e.size => bail!("blob size mismatch"),
                _ => {}
            }
        }
        Ok(out)
    }
    /// Materialize into a caller-created empty private directory.
    pub fn materialize(&self, manifest: &Manifest, destination: &Path) -> Result<()> {
        manifest.validate()?;
        if !self.missing(manifest)?.is_empty() {
            bail!("missing blobs");
        }
        let root = crate::safefs::Root::open(destination)?;
        owned(&std::fs::symlink_metadata(destination)?)?;
        if !root.dir(Path::new(""), false)?.entries()?.is_empty() {
            bail!("destination must be empty");
        }
        for (p, e) in &manifest.files {
            let bytes = self.get(&e.hash)?.context("missing blob")?;
            if bytes.len() as u64 != e.size {
                bail!("blob size mismatch");
            }
            let path = Path::new(p);
            root.write(path, &bytes, if e.executable { 0o700 } else { 0o600 })?;
            let dir = root.dir(path.parent().unwrap_or(Path::new("")), false)?;
            let file = dir
                .open_read(path.file_name().context("missing file name")?)?
                .context("materialized file missing")?;
            let magnitude = std::time::Duration::from_nanos(e.modified_ns.unsigned_abs());
            let time = if e.modified_ns < 0 {
                std::time::UNIX_EPOCH.checked_sub(magnitude)
            } else {
                std::time::UNIX_EPOCH.checked_add(magnitude)
            }
            .context("invalid modification time")?;
            file.set_times(std::fs::FileTimes::new().set_modified(time))?;
        }
        for (p, target) in &manifest.links {
            let path = Path::new(p);
            let dir = root.dir(path.parent().unwrap_or(Path::new("")), true)?;
            dir.symlink(
                Path::new(target),
                path.file_name().context("missing link name")?,
            )?;
        }
        Ok(())
    }
}
#[path = "snapshot_plain.rs"]
mod plain;

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
