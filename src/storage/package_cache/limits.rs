//! Size limits for package caches. A tool directory is the unit of eviction.
use super::{identity, lease, root};
use crate::env::HostEnv;
use anyhow::Result;
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

struct Entry {
    dir: PathBuf,
    project_dir: PathBuf,
    owner: PathBuf,
    bytes: u64,
    last_used: i64,
}

fn dirs(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

fn entries(env: &HostEnv) -> Vec<Entry> {
    let mut out = Vec::new();
    for (base, shared) in [
        (super::super::layers::projects_root(env), false),
        (root(env), true),
    ] {
        for project_dir in dirs(&base) {
            let Ok(text) = std::fs::read_to_string(project_dir.join("project")) else {
                continue;
            };
            let project = PathBuf::from(text);
            if project_dir
                .file_name()
                .is_none_or(|n| n != std::ffi::OsStr::new(&crate::state::slug(&project)))
            {
                continue;
            }
            let owner = if shared {
                project
            } else {
                identity(&project).unwrap_or(project)
            };
            for cache in super::super::layers::CACHES {
                if shared && !matches!(*cache, "uv" | "npm") {
                    continue;
                }
                let dir = project_dir.join(cache);
                let Ok(meta) = std::fs::symlink_metadata(&dir) else {
                    continue;
                };
                if !meta.is_dir() {
                    continue;
                }
                out.push(Entry {
                    bytes: super::super::allocated(&dir),
                    last_used: meta.mtime(),
                    dir,
                    project_dir: project_dir.clone(),
                    owner: owner.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| (a.last_used, &a.dir).cmp(&(b.last_used, &b.dir)));
    out
}

/// Trim after acquiring exclusive per-project leases. A live process from an
/// older paddock without leases is also protected by its running record.
pub fn trim(env: &HostEnv, per_project: u64, total_cap: u64) -> Result<Vec<PathBuf>> {
    if per_project == 0 && total_cap == 0 {
        return Ok(Vec::new());
    }
    // Only one pass may calculate and apply a budget at a time.
    let Ok(_pass) = lease(env, &root(env).join(".limits"), true) else {
        return Ok(Vec::new());
    };
    let mut entries = entries(env);
    let mut sizes = BTreeMap::<PathBuf, u64>::new();
    let mut total = 0u64;
    for e in &entries {
        let size = sizes.entry(e.owner.clone()).or_default();
        *size = size.saturating_add(e.bytes);
        total = total.saturating_add(e.bytes);
    }
    let mut running = Vec::new();
    let mut unknown = false;
    for run in crate::state::Running::list(env) {
        match identity(run.kv_project.as_deref().unwrap_or(&run.project)) {
            Ok(owner) => {
                running.push((owner, super::run_tools(env, &run.id)));
            }
            Err(_) => unknown = true,
        }
    }
    let mut removed = Vec::new();
    for e in &mut entries {
        let project_over = per_project != 0 && sizes[&e.owner] > per_project;
        let total_over = total_cap != 0 && total > total_cap;
        let shared = e.project_dir.parent() == Some(root(env).as_path());
        let tool = e.dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let busy = running.iter().any(|(owner, tools)| {
            owner == &e.owner
                && (!shared || tools.as_ref().is_none_or(|v| v.iter().any(|t| t == tool)))
        });
        if !(project_over || total_over) || unknown || busy {
            continue;
        }
        let Ok(_project_guard) = lease(env, &e.project_dir, true) else {
            continue;
        };
        let guard = if e.project_dir.parent() == Some(root(env).as_path()) {
            e.dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|name| super::lease_tool(env, &e.project_dir, name, true).ok())
        } else {
            None
        };
        if e.project_dir.parent() == Some(root(env).as_path()) && guard.is_none() {
            continue;
        }
        let _tool_guard = guard;
        // The tree may have grown since the scan. The lease prevents new runs
        // from starting while it is removed.
        let bytes = super::super::allocated(&e.dir);
        if super::super::remove_path(env, &e.dir).is_ok() {
            total = total.saturating_sub(bytes);
            if let Some(size) = sizes.get_mut(&e.owner) {
                *size = size.saturating_sub(bytes);
            }
            removed.push(e.dir.clone());
        }
    }
    Ok(removed)
}

pub fn configured(env: &HostEnv) -> Result<Vec<PathBuf>> {
    let (settings, _) = super::super::settings::Settings::load(env);
    let gib = super::super::settings::GIB;
    trim(
        env,
        settings.package_cache_project_gib.saturating_mul(gib),
        settings.package_cache_total_gib.saturating_mul(gib),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn stamp(path: &Path, seconds: u64) {
        std::fs::File::open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn project_cap_evicts_oldest_whole_tool_and_respects_active_lease() {
        if super::super::isolated_test("storage::package_cache::limits::tests::project_cap_evicts_oldest_whole_tool_and_respects_active_lease") { return; }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let uv = super::super::prepare_uv(&env, &project).unwrap();
        let pip = super::super::super::layers::mount_with(
            &env,
            &project,
            "pip",
            "/h/pip".into(),
            "test",
            false,
        )
        .unwrap()
        .src
        .unwrap();
        std::fs::write(uv.join("fake"), vec![1; 8192]).unwrap();
        std::fs::write(pip.join("fake"), vec![2; 8192]).unwrap();
        let pip_dir = pip.parent().unwrap();
        stamp(pip_dir, 1);
        stamp(&uv, 2);
        let max = super::super::super::allocated(&uv).max(super::super::super::allocated(pip_dir));
        let legacy = super::super::super::layers::project_dir(&env, &project);
        let guard = lease(&env, &legacy, false).unwrap();
        let removed = trim(&env, max, 0).unwrap();
        assert!(!removed.contains(&pip_dir.to_path_buf()));
        assert!(pip.join("fake").is_file());
        drop(guard);
        // Restore both caches after the first pass might evict uv instead.
        std::fs::create_dir_all(&uv).unwrap();
        std::fs::write(uv.join("fake"), vec![1; 8192]).unwrap();
        stamp(pip_dir, 1);
        stamp(&uv, 2);
        let removed = trim(&env, max, 0).unwrap();
        assert_eq!(removed, vec![pip_dir.to_path_buf()]);
        assert!(uv.join("fake").is_file());
        assert!(!pip_dir.exists());
    }

    #[test]
    fn npm_is_counted_and_a_live_lease_prevents_eviction() {
        if super::super::isolated_test("storage::package_cache::limits::tests::npm_is_counted_and_a_live_lease_prevents_eviction") { return; }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let npm = super::super::prepare(&env, &project, "npm").unwrap();
        std::fs::write(npm.join("fake"), vec![1; 8192]).unwrap();
        let guard = super::super::lease_tool(&env, npm.parent().unwrap(), "npm", false).unwrap();
        assert!(trim(&env, 1, 0).unwrap().is_empty());
        assert!(npm.join("fake").exists());
        drop(guard);
        assert_eq!(trim(&env, 1, 0).unwrap(), vec![npm.clone()]);
        assert!(!npm.exists());
    }

    #[test]
    fn an_idle_tool_can_be_evicted_while_another_is_leased() {
        if super::super::isolated_test("storage::package_cache::limits::tests::an_idle_tool_can_be_evicted_while_another_is_leased") { return; }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let uv = super::super::prepare(&env, &project, "uv").unwrap();
        let npm = super::super::prepare(&env, &project, "npm").unwrap();
        std::fs::write(uv.join("fake"), vec![1; 8192]).unwrap();
        std::fs::write(npm.join("fake"), vec![1; 8192]).unwrap();
        let guard = super::super::lease_tool(&env, uv.parent().unwrap(), "uv", false).unwrap();
        let removed = trim(&env, 1, 0).unwrap();
        assert_eq!(removed, vec![npm.clone()]);
        assert!(uv.join("fake").exists());
        drop(guard);
    }

    #[test]
    fn total_cap_evicts_oldest_across_projects() {
        if super::super::isolated_test(
            "storage::package_cache::limits::tests::total_cap_evicts_oldest_across_projects",
        ) {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let a = t.path().join("a");
        let b = t.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let ca = super::super::prepare_uv(&env, &a).unwrap();
        let cb = super::super::prepare_uv(&env, &b).unwrap();
        std::fs::write(ca.join("fake"), vec![1; 8192]).unwrap();
        std::fs::write(cb.join("fake"), vec![2; 8192]).unwrap();
        stamp(&ca, 1);
        stamp(&cb, 2);
        let max = super::super::super::allocated(&ca).max(super::super::super::allocated(&cb));
        assert_eq!(trim(&env, 0, max).unwrap(), vec![ca.clone()]);
        assert!(!ca.exists());
        assert!(cb.join("fake").exists());
    }

    #[test]
    fn an_inherited_lease_prevents_eviction_until_the_child_exits() {
        if super::super::isolated_test("storage::package_cache::limits::tests::an_inherited_lease_prevents_eviction_until_the_child_exits") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cache = super::super::prepare_uv(&env, &project).unwrap();
        std::fs::write(cache.join("fake"), vec![1; 8192]).unwrap();
        let guard = lease(&env, cache.parent().unwrap(), false).unwrap();
        let mut pipe = [0; 2];
        // SAFETY: pipe is writable; the child calls only async-signal-safe
        // operations before _exit.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork failed");
        if child == 0 {
            unsafe {
                libc::close(pipe[1]);
                let mut byte = 0u8;
                libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
                libc::_exit(0);
            }
        }
        unsafe { libc::close(pipe[0]) };
        drop(guard);
        let while_in_use = trim(&env, 1, 0).unwrap();
        let kept = cache.join("fake").exists();
        unsafe {
            libc::write(pipe[1], [1u8].as_ptr().cast(), 1);
            libc::close(pipe[1]);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
        assert!(while_in_use.is_empty() && kept);
        assert_eq!(trim(&env, 1, 0).unwrap(), vec![cache]);
    }

    #[test]
    fn package_caps_are_configurable() {
        let (s, warnings) = super::super::super::settings::Settings::parse(
            "[storage]\npackage_cache_project_gib = 3\npackage_cache_total_gib = 12\n",
        );
        assert!(warnings.is_empty());
        assert_eq!(
            (s.package_cache_project_gib, s.package_cache_total_gib),
            (3, 12)
        );
    }
}
