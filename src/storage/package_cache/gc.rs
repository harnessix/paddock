//! Reclaim legacy worktree caches, preserving one uv cache when possible.
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn repository(env: &HostEnv, project: &Path) -> Option<PathBuf> {
    if project.exists() {
        let layout = crate::hostgit::Layout::discover(project).ok().flatten()?;
        if layout.linked {
            return Some(layout.common_dir);
        }
        return None;
    }
    // Paddock's host-side worktree directory names the original repo slug.
    // Match it only against host-side records of existing repositories.
    let rel = project
        .strip_prefix(env.paddock_data().join("worktrees"))
        .ok()?;
    let mut parts = rel.components();
    let repo_key = parts.next()?.as_os_str();
    parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let candidates = crate::state::recent(env)
        .into_iter()
        .map(|r| r.project)
        .chain(
            crate::recover::Session::all(env)
                .into_iter()
                .map(|s| s.project),
        );
    let mut found = None;
    for candidate in candidates {
        if std::ffi::OsStr::new(&crate::state::slug(&candidate)) != repo_key {
            continue;
        }
        let layout = crate::hostgit::Layout::discover(&candidate)
            .ok()
            .flatten()?;
        let owner = std::fs::canonicalize(layout.common_dir).ok()?;
        if found.as_ref().is_some_and(|p| *p != owner) {
            return None;
        }
        found = Some(owner);
    }
    found
}

/// Move a tool's former worktree upper into an empty project cache.
fn move_tool(legacy: &Path, project_dir: &Path, cache: &str) -> Result<bool> {
    let source = legacy.join(cache).join("upper");
    if !std::fs::symlink_metadata(&source).is_ok_and(|m| m.is_dir()) {
        return Ok(false);
    }
    let dest = project_dir.join(cache);
    match std::fs::symlink_metadata(&dest) {
        Ok(meta) if meta.is_dir() => {
            if std::fs::read_dir(&dest)?.next().is_some() {
                return Ok(false);
            }
            std::fs::remove_dir(&dest)?;
        }
        Ok(_) => bail!("cache destination is not a directory"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    std::fs::rename(&source, &dest).context("move old package cache")?;
    Ok(true)
}

/// At first use, migrate this member's old upper while neither side is in use.
pub(super) fn migrate_on_prepare(env: &HostEnv, project: &Path, cache: &str) -> Result<()> {
    if !matches!(cache, "uv" | "npm") {
        return Ok(());
    }
    let legacy = super::super::layers::project_dir(env, project);
    if !legacy.join(cache).join("upper").is_dir() {
        return Ok(());
    }
    if std::fs::read_to_string(legacy.join("project"))
        .ok()
        .as_deref()
        != Some(project.to_string_lossy().as_ref())
    {
        return Ok(());
    }
    let dir = super::project_dir(env, project)?;
    let Ok(_project_guard) = super::lease(env, &dir, true) else {
        return Ok(());
    };
    let _legacy_guard = if legacy.file_name() == dir.file_name() {
        None
    } else {
        match super::lease(env, &legacy, true) {
            Ok(g) => Some(g),
            Err(_) => return Ok(()),
        }
    };
    let Ok(_tool_guard) = super::lease_tool(env, &dir, cache, true) else {
        return Ok(());
    };
    crate::privfs::dir(&dir)?;
    crate::privfs::write_atomic(
        &dir.join("project"),
        super::identity(project)?.to_string_lossy().as_bytes(),
    )?;
    move_tool(&legacy, &dir, cache)?;
    Ok(())
}

/// The caller holds the legacy project's exclusive lease. Do not merge two
/// populated caches: rename the old upper only when the destination is empty.
fn migrate(env: &HostEnv, legacy: &Path, repo: &Path) -> Result<bool> {
    let dir = super::project_dir(env, repo)?;
    if legacy.file_name() == dir.file_name() {
        bail!("cache leases have the same key");
    }
    let _guard = super::lease(env, &dir, true)?;
    let _uv = super::lease_tool(env, &dir, "uv", true)?;
    let _npm = super::lease_tool(env, &dir, "npm", true)?;
    crate::privfs::dir(&dir)?;
    crate::privfs::write_atomic(
        &dir.join("project"),
        super::identity(repo)?.to_string_lossy().as_bytes(),
    )?;
    let mut moved = false;
    for cache in ["uv", "npm"] {
        moved |= move_tool(legacy, &dir, cache)?;
    }
    Ok(moved)
}

/// Remove a planned legacy cache after rechecking the marker and worktree.
pub fn remove(env: &HostEnv, legacy: &Path) -> Result<()> {
    let _guard = super::lease(env, legacy, true)?;
    let project = std::fs::read_to_string(legacy.join("project"))
        .ok()
        .map(PathBuf::from);
    if let Some(project) = &project {
        if legacy
            .file_name()
            .is_none_or(|n| n != std::ffi::OsStr::new(&crate::state::slug(project)))
        {
            bail!("cache project marker does not match its directory");
        }
        if project.is_dir() {
            let valid = super::super::worktrees(env)
                .iter()
                .any(|w| w.path == *project && !w.running && w.merged == Some(true));
            if !valid {
                bail!("worktree is no longer known to be merged and idle");
            }
        }
        if let Some(repo) = repository(env, project) {
            migrate(env, legacy, &repo)?;
        }
    }
    super::super::remove_path(env, legacy)
}

/// A shared cache is removable only while its recorded repository is gone.
pub fn remove_shared(env: &HostEnv, dir: &Path) -> Result<()> {
    let _guard = super::lease(env, dir, true)?;
    let _uv_guard = super::lease_tool(env, dir, "uv", true)?;
    let _npm_guard = super::lease_tool(env, dir, "npm", true)?;
    if let Ok(text) = std::fs::read_to_string(dir.join("project")) {
        let project = PathBuf::from(text);
        if dir
            .file_name()
            .is_none_or(|n| n != std::ffi::OsStr::new(&crate::state::slug(&project)))
        {
            bail!("cache project marker does not match its directory");
        }
        if project.is_dir() {
            bail!("the project exists again");
        }
    }
    super::super::remove_path(env, dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success());
    }

    fn old_cache(env: &HostEnv, repo: &Path) -> (PathBuf, PathBuf) {
        let worktree = env
            .paddock_data()
            .join("worktrees")
            .join(crate::state::slug(repo))
            .join("fake-worker");
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "paddock/fake",
                worktree.to_str().unwrap(),
            ],
        );
        let old = super::super::super::layers::project_dir(env, &worktree);
        std::fs::create_dir_all(old.join("uv/upper")).unwrap();
        std::fs::write(old.join("project"), worktree.to_string_lossy().as_bytes()).unwrap();
        std::fs::write(old.join("uv/upper/fake"), b"cached").unwrap();
        crate::state::record_recent(env, repo, "fake").unwrap();
        git(
            repo,
            &["worktree", "remove", "-f", worktree.to_str().unwrap()],
        );
        (worktree, old)
    }

    fn fixture() -> (tempfile::TempDir, HostEnv, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let repo = t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ],
        );
        (t, env, repo)
    }

    #[test]
    fn first_use_moves_existing_tool_caches_and_respects_legacy_users() {
        if super::super::isolated_test("storage::package_cache::gc::tests::first_use_moves_existing_tool_caches_and_respects_legacy_users") { return; }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let project = t.path().join("plain");
        std::fs::create_dir_all(&project).unwrap();
        let old = super::super::super::layers::project_dir(&env, &project);
        for tool in ["uv", "npm"] {
            std::fs::create_dir_all(old.join(tool).join("upper")).unwrap();
            std::fs::write(old.join(tool).join("upper/fake"), tool).unwrap();
        }
        std::fs::write(old.join("project"), project.to_string_lossy().as_bytes()).unwrap();
        let npm_inode = std::fs::metadata(old.join("npm/upper/fake")).unwrap().ino();
        let active = super::super::lease(&env, &old, false).unwrap();
        let uv = super::super::prepare(&env, &project, "uv").unwrap();
        assert!(old.join("uv/upper/fake").exists());
        assert!(!uv.join("fake").exists());
        drop(active);
        let uv = super::super::prepare(&env, &project, "uv").unwrap();
        let npm = super::super::prepare(&env, &project, "npm").unwrap();
        assert_eq!(std::fs::read(uv.join("fake")).unwrap(), b"uv");
        assert_eq!(std::fs::read(npm.join("fake")).unwrap(), b"npm");
        assert_eq!(
            std::fs::metadata(npm.join("fake")).unwrap().ino(),
            npm_inode
        );
        assert!(!old.join("npm/upper").exists());
    }

    #[test]
    fn gone_worktree_cache_moves_without_copy_and_obeys_lease() {
        if super::super::isolated_test("storage::package_cache::gc::tests::gone_worktree_cache_moves_without_copy_and_obeys_lease") { return; }
        let (_t, env, repo) = fixture();
        let (_, old) = old_cache(&env, &repo);
        let inode = std::fs::metadata(old.join("uv/upper/fake")).unwrap().ino();
        let lease = super::super::lease(&env, &old, false).unwrap();
        assert!(remove(&env, &old).is_err());
        drop(lease);
        remove(&env, &old).unwrap();
        let new = super::super::uv_dir(&env, &repo).unwrap();
        assert_eq!(std::fs::read(new.join("fake")).unwrap(), b"cached");
        assert_eq!(std::fs::metadata(new.join("fake")).unwrap().ino(), inode);
        assert!(!old.exists());
    }

    #[test]
    fn migration_preserves_a_populated_shared_cache() {
        if super::super::isolated_test(
            "storage::package_cache::gc::tests::migration_preserves_a_populated_shared_cache",
        ) {
            return;
        }
        let (_t, env, repo) = fixture();
        let (_, old) = old_cache(&env, &repo);
        let new = super::super::prepare_uv(&env, &repo).unwrap();
        std::fs::write(new.join("fake"), b"new").unwrap();
        remove(&env, &old).unwrap();
        assert_eq!(std::fs::read(new.join("fake")).unwrap(), b"new");
        assert!(!old.exists());
    }
    #[test]
    fn merged_worktree_cache_is_migrated_and_worktree_remains() {
        if super::super::isolated_test("storage::package_cache::gc::tests::merged_worktree_cache_is_migrated_and_worktree_remains") { return; }
        let (_t, env, repo) = fixture();
        let worktree = env
            .paddock_data()
            .join("worktrees")
            .join(crate::state::slug(&repo))
            .join("merged");
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "paddock/merged",
                worktree.to_str().unwrap(),
            ],
        );
        crate::state::record_recent(&env, &repo, "fake").unwrap();
        let old = super::super::super::layers::project_dir(&env, &worktree);
        std::fs::create_dir_all(old.join("uv/upper")).unwrap();
        std::fs::write(old.join("project"), worktree.to_string_lossy().as_bytes()).unwrap();
        std::fs::write(old.join("uv/upper/fake"), b"merged").unwrap();
        remove(&env, &old).unwrap();
        assert_eq!(
            std::fs::read(super::super::uv_dir(&env, &repo).unwrap().join("fake")).unwrap(),
            b"merged"
        );
        assert!(worktree.is_dir());
        assert!(!old.exists());
    }
    #[test]
    fn shared_cache_is_listed_and_removed_only_after_project_disappears() {
        if super::super::isolated_test("storage::package_cache::gc::tests::shared_cache_is_listed_and_removed_only_after_project_disappears") { return; }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let project = t.path().join("plain");
        std::fs::create_dir_all(&project).unwrap();
        let uv = super::super::prepare_uv(&env, &project).unwrap();
        let dir = uv.parent().unwrap().to_path_buf();
        assert!(super::super::super::layers::scan(&env)
            .iter()
            .any(|l| l.path == dir && !l.orphan));
        assert!(remove_shared(&env, &dir).is_err());
        std::fs::remove_dir(&project).unwrap();
        assert!(super::super::super::layers::scan(&env)
            .iter()
            .any(|l| l.path == dir && l.orphan));
        let guard = super::super::lease(&env, &dir, false).unwrap();
        assert!(remove_shared(&env, &dir).is_err());
        drop(guard);
        let guard = super::super::lease_tool(&env, &dir, "uv", false).unwrap();
        assert!(remove_shared(&env, &dir).is_err());
        drop(guard);
        remove_shared(&env, &dir).unwrap();
        assert!(!dir.exists());
    }
}
