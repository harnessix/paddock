//! Native transcript fingerprints captured outside the sandbox.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    inode: u64,
    device: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Snapshot(BTreeMap<PathBuf, Fingerprint>);

fn layout(harness: &str) -> Result<&'static str> {
    match harness {
        "dsh" => Ok(".dsh/sessions"),
        "claude" => Ok(".claude/projects"),
        "codex" => Ok(".codex/sessions"),
        _ => bail!("no native transcript layout for {harness}"),
    }
}

fn matches(harness: &str, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match harness {
        "dsh" => name == "session.v4.jsonl" || name == "session.v4.jsonl.zstd",
        "claude" => name
            .strip_suffix(".jsonl")
            .is_some_and(|id| super::super::claude::check_session_id(id).is_ok()),
        "codex" => name.starts_with("rollout-") && name.ends_with(".jsonl"),
        _ => false,
    }
}

fn visit(
    root: &Path,
    dir: &Path,
    harness: &str,
    files: &mut BTreeMap<PathBuf, Fingerprint>,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            visit(root, &path, harness, files)?;
        } else if meta.is_file() && matches(harness, &path) {
            files.insert(
                path.strip_prefix(root)?.to_path_buf(),
                Fingerprint {
                    size: meta.len(),
                    mtime: meta.mtime(),
                    mtime_nsec: meta.mtime_nsec(),
                    inode: meta.ino(),
                    device: meta.dev(),
                },
            );
        }
    }
    Ok(())
}

impl Snapshot {
    pub fn capture(home: &Path, harness: &str) -> Result<Self> {
        let mut files = BTreeMap::new();
        let root = home.join(layout(harness)?);
        // A symlink at the root is not a sandbox-owned transcript directory.
        match std::fs::symlink_metadata(&root) {
            Ok(meta) if meta.is_dir() => visit(home, &root, harness, &mut files)?,
            Ok(_) => bail!(
                "native transcript root is not a directory: {}",
                root.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Self(files))
    }

    pub fn save(&self, run_dir: &Path) -> Result<()> {
        crate::privfs::write_atomic(
            &run_dir.join("horse-native-snapshot.json"),
            serde_json::to_vec(self)?,
        )?;
        Ok(())
    }

    pub fn load(run_dir: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(
            run_dir.join("horse-native-snapshot.json"),
        )?)?)
    }

    pub fn changed(&self, home: &Path, harness: &str) -> Result<Vec<PathBuf>> {
        let current = Self::capture(home, harness)?;
        Ok(current
            .0
            .into_iter()
            .filter(|(path, meta)| self.0.get(path) != Some(meta))
            .map(|(path, _)| home.join(path))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn detects_new_changed_and_replaced_files_without_following_links() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let dir = home.join(".codex/sessions/2026/11/04");
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("rollout-old.jsonl");
        let replaced = dir.join("rollout-replaced.jsonl");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&replaced, b"same").unwrap();
        let snapshot = Snapshot::capture(&home, "codex").unwrap();
        let run = t.path().join("private-run");
        snapshot.save(&run).unwrap();
        assert_eq!(
            std::fs::metadata(run.join("horse-native-snapshot.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::write(&old, b"changed").unwrap();
        let replacement = dir.join("replacement");
        std::fs::write(&replacement, b"same").unwrap();
        std::fs::rename(replacement, &replaced).unwrap();
        let new = dir.join("rollout-new.jsonl");
        std::fs::write(&new, b"new").unwrap();
        symlink(&new, dir.join("rollout-link.jsonl")).unwrap();
        let changed = Snapshot::load(&run)
            .unwrap()
            .changed(&home, "codex")
            .unwrap();
        assert_eq!(changed, vec![new, old, replaced]);
    }

    #[test]
    fn claude_ignores_non_session_jsonl_and_missing_root() {
        let t = tempfile::tempdir().unwrap();
        let snapshot = Snapshot::capture(t.path(), "claude").unwrap();
        let dir = t.path().join(".claude/projects/project");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("other.jsonl"), b"x").unwrap();
        let session = dir.join("01934b20-1111-7777-8888-123456789abc.jsonl");
        std::fs::write(&session, b"x").unwrap();
        assert_eq!(snapshot.changed(t.path(), "claude").unwrap(), vec![session]);
        assert!(Snapshot::capture(t.path(), "grok").is_err());
    }
}
