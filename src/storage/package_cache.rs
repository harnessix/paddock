//! Project-scoped package caches and their run leases.
use crate::env::HostEnv;
use anyhow::{Context, Result};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub mod gc;
pub mod limits;

pub fn identity(project: &Path) -> Result<PathBuf> {
    let path = match crate::hostgit::Layout::discover(project)? {
        Some(layout) => layout.common_dir,
        None => project.to_path_buf(),
    };
    std::fs::canonicalize(&path)
        .with_context(|| format!("resolve cache identity {}", path.display()))
}

pub fn root(env: &HostEnv) -> PathBuf {
    env.paddock_cache().join("package-projects")
}

pub fn project_dir(env: &HostEnv, project: &Path) -> Result<PathBuf> {
    Ok(root(env).join(crate::state::slug(&identity(project)?)))
}

pub fn uv_dir(env: &HostEnv, project: &Path) -> Result<PathBuf> {
    Ok(project_dir(env, project)?.join("uv"))
}

pub fn prepare(env: &HostEnv, project: &Path, cache: &str) -> Result<PathBuf> {
    let uv = project_dir(env, project)?.join(cache);
    let dir = uv.parent().context("cache has no project")?;
    gc::migrate_on_prepare(env, project, cache)?;
    let _guard = lease(env, dir, false)?;
    let _tool_guard = lease_tool(env, dir, cache, false)?;
    crate::privfs::dir(&uv)?;
    crate::privfs::write_atomic(
        &dir.join("project"),
        identity(project)?.to_string_lossy().as_bytes(),
    )?;
    Ok(uv)
}

pub fn prepare_uv(env: &HostEnv, project: &Path) -> Result<PathBuf> {
    prepare(env, project, "uv")
}

/// The lock is outside the writable cache and stays in place after eviction.
pub struct Lease(#[allow(dead_code)] std::fs::File);

pub fn lease(env: &HostEnv, project_dir: &Path, exclusive: bool) -> Result<Lease> {
    let key = project_dir
        .file_name()
        .context("cache project has no name")?;
    lock_key(env, key, exclusive)
}

pub fn lease_tool(
    env: &HostEnv,
    project_dir: &Path,
    cache: &str,
    exclusive: bool,
) -> Result<Lease> {
    if !matches!(cache, "uv" | "npm") {
        anyhow::bail!("invalid shared cache name");
    }
    let name = project_dir
        .file_name()
        .context("cache project has no name")?
        .to_string_lossy();
    let key = format!("{name}--tool--{cache}");
    lock_key(env, std::ffi::OsStr::new(&key), exclusive)
}

fn lock_key(env: &HostEnv, key: &std::ffi::OsStr, exclusive: bool) -> Result<Lease> {
    let locks = env.paddock_cache().join("package-locks");
    crate::privfs::dir(&locks)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(locks.join(key))?;
    let operation = if exclusive {
        libc::LOCK_EX | libc::LOCK_NB
    } else {
        libc::LOCK_SH
    };
    // SAFETY: file is a valid open descriptor and remains open in Lease.
    if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
        return Err(std::io::Error::last_os_error()).context("package cache is in use");
    }
    Ok(Lease(file))
}

/// Acquire before launch and recreate the source under the lease in case gc
/// removed it between building the sandbox specification and starting it.
pub fn lease_run(env: &HostEnv, spec: &crate::spec::SandboxSpec) -> Result<Vec<Lease>> {
    let mut guards = Vec::new();
    let legacy = super::layers::project_dir(env, &spec.project);
    let has_legacy = spec
        .mounts
        .iter()
        .any(|m| m.src.as_ref().is_some_and(|p| p.starts_with(&legacy)));
    if has_legacy {
        let guard = lease(env, &legacy, false)?;
        crate::privfs::dir(&legacy)?;
        crate::privfs::write_atomic(
            &legacy.join("project"),
            spec.project.to_string_lossy().as_bytes(),
        )?;
        for m in &spec.mounts {
            if let Some(src) = &m.src {
                if src.starts_with(&legacy) {
                    crate::privfs::dir(src)?;
                    if let Some(layers) = &m.layers {
                        crate::privfs::dir(&layers.work)?;
                    }
                    touch(src.parent().unwrap_or(src));
                }
            }
        }
        guards.push(guard);
    }
    let dir = project_dir(env, &spec.project)?;
    let used: Vec<_> = spec
        .mounts
        .iter()
        .filter_map(|m| m.src.as_ref())
        .filter(|p| p.parent() == Some(dir.as_path()))
        .collect();
    if !used.is_empty() {
        for path in used {
            let cache = path
                .file_name()
                .and_then(|n| n.to_str())
                .context("invalid cache name")?;
            guards.push(lease_tool(env, &dir, cache, false)?);
            crate::privfs::dir(path)?;
            touch(path);
        }
        crate::privfs::write_atomic(
            &dir.join("project"),
            identity(&spec.project)?.to_string_lossy().as_bytes(),
        )?;
    }
    Ok(guards)
}

fn touch(path: &Path) {
    let _ = std::fs::File::open(path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
}

fn lease_record(env: &HostEnv, id: &str) -> Result<PathBuf> {
    crate::kv::shard::check_id(id)?;
    Ok(env.paddock_run().join(id).join("cache-leases.json"))
}

/// Cache names recorded for a running sandbox. Missing or invalid means unknown.
pub fn run_tools(env: &HostEnv, id: &str) -> Option<Vec<String>> {
    let file = lease_record(env, id).ok()?;
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// Visible holders, backed by the live-run registry and the kernel locks.
pub fn holders(env: &HostEnv, project: &Path) -> Vec<(String, String)> {
    let Ok(owner) = identity(project) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for run in crate::state::Running::list(env) {
        if identity(run.kv_project.as_deref().unwrap_or(&run.project))
            .ok()
            .as_ref()
            != Some(&owner)
        {
            continue;
        }
        for tool in run_tools(env, &run.id).unwrap_or_default() {
            if matches!(tool.as_str(), "uv" | "npm") {
                out.push((format!("lease:cache:{tool}"), run.id.clone()));
            }
        }
    }
    out.sort();
    out
}

/// Holds leases throughout a run and trims again after releasing them.
pub struct RunCache {
    env: HostEnv,
    guards: Vec<Lease>,
    used: Vec<PathBuf>,
    record: PathBuf,
}
impl RunCache {
    pub fn start(env: &HostEnv, spec: &crate::spec::SandboxSpec) -> Result<Self> {
        if let Err(e) = limits::configured(env) {
            log::warn!("package cache cleanup: {e:#}");
        }
        let guards = lease_run(env, spec)?;
        let used: Vec<PathBuf> = spec
            .mounts
            .iter()
            .filter_map(|m| m.src.as_ref())
            .filter(|p| {
                p.starts_with(root(env)) || p.starts_with(super::layers::projects_root(env))
            })
            .map(|p| {
                if p.file_name().is_some_and(|n| n == "upper") {
                    p.parent().unwrap_or(p).to_path_buf()
                } else {
                    p.to_path_buf()
                }
            })
            .collect();
        let dir = project_dir(env, &spec.project)?;
        let tools: Vec<_> = used
            .iter()
            .filter(|p| p.parent() == Some(dir.as_path()))
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        let record = lease_record(env, &spec.id)?;
        crate::privfs::dir(
            record
                .parent()
                .context("cache lease record has no parent")?,
        )?;
        crate::privfs::write_atomic(&record, serde_json::to_vec(&tools)?)?;
        Ok(Self {
            env: env.clone(),
            guards,
            used,
            record,
        })
    }
}
impl Drop for RunCache {
    fn drop(&mut self) {
        for path in &self.used {
            touch(path);
        }
        self.guards.clear();
        let _ = std::fs::remove_file(&self.record);
        if let Err(e) = limits::configured(&self.env) {
            log::warn!("package cache cleanup: {e:#}");
        }
    }
}

/// Isolate tests that reacquire a lease from forks in other test threads.
#[cfg(test)]
pub(crate) fn isolated_test(name: &str) -> bool {
    const CHILD: &str = "PADDOCK_TEST_CACHE_ISOLATED";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .env("RUST_TEST_THREADS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::MountKind;
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

    #[test]
    fn uv_is_shared_by_worktrees_but_not_other_projects() {
        let t = tempfile::tempdir().unwrap();
        let env = super::super::layers::test_env(t.path());
        let repo = t.path().join("repo");
        let other = t.path().join("other");
        let wt = t.path().join("wt");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&other).unwrap();
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
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "test-wt",
                wt.to_str().unwrap(),
            ],
        );
        let mount = |p: &Path, name| {
            super::super::layers::mount_with(&env, p, name, "/home/demo/cache".into(), "test", true)
                .unwrap()
        };
        let a = mount(&repo, "uv");
        let b = mount(&wt, "uv");
        let c = mount(&other, "uv");
        assert_eq!(a.kind, MountKind::Rw);
        assert_eq!(a.src, b.src);
        assert_ne!(a.src, c.src);
        std::fs::write(a.src.as_ref().unwrap().join("fake"), b"cached").unwrap();
        let shared = b.src.unwrap();
        assert_eq!(std::fs::read(shared.join("fake")).unwrap(), b"cached");
        std::thread::scope(|scope| {
            for name in ["writer-a", "writer-b"] {
                let shared = &shared;
                scope.spawn(move || std::fs::write(shared.join(name), name).unwrap());
            }
        });
        assert!(shared.join("writer-a").is_file() && shared.join("writer-b").is_file());
        assert_eq!(mount(&repo, "npm").src, mount(&wt, "npm").src);
        assert_ne!(mount(&repo, "npm").src, mount(&other, "npm").src);
        assert_ne!(mount(&repo, "pip").src, mount(&wt, "pip").src);
        assert!(mount(&wt, "cargo-registry").origin.contains("per worktree"));
    }

    #[test]
    fn leases_prevent_eviction_until_all_users_leave() {
        if isolated_test(
            "storage::package_cache::tests::leases_prevent_eviction_until_all_users_leave",
        ) {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let dir = project_dir(&env, &project).unwrap();
        let a = lease(&env, &dir, false).unwrap();
        let b = lease(&env, &dir, false).unwrap();
        assert!(lease(&env, &dir, true).is_err());
        drop(a);
        assert!(lease(&env, &dir, true).is_err());
        drop(b);
        assert!(lease(&env, &dir, true).is_ok());
    }
    #[test]
    fn tool_leases_are_independent() {
        if isolated_test("storage::package_cache::tests::tool_leases_are_independent") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let dir = project_dir(&env, &project).unwrap();
        let uv = lease_tool(&env, &dir, "uv", false).unwrap();
        assert!(lease_tool(&env, &dir, "uv", true).is_err());
        assert!(lease_tool(&env, &dir, "npm", true).is_ok());
        drop(uv);
        assert!(lease_tool(&env, &dir, "uv", true).is_ok());
    }

    #[test]
    fn run_lease_recreates_the_cache_and_blocks_eviction() {
        if isolated_test(
            "storage::package_cache::tests::run_lease_recreates_the_cache_and_blocks_eviction",
        ) {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let env = super::super::layers::test_env(t.path());
        let project = t.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let mut spec = crate::spec::SandboxSpec::new(
            "test".into(),
            "fake",
            &project,
            t.path().join("home"),
            Path::new("/home/demo"),
            1000,
            1000,
            "test",
        );
        spec.add_mount(
            super::super::layers::mount_with(
                &env,
                &project,
                "uv",
                "/home/demo/.cache/uv".into(),
                "test",
                true,
            )
            .unwrap(),
        );
        spec.add_mount(
            super::super::layers::mount_with(
                &env,
                &project,
                "npm",
                "/home/demo/.npm".into(),
                "test",
                true,
            )
            .unwrap(),
        );
        let dir = project_dir(&env, &project).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let guard = RunCache::start(&env, &spec).unwrap();
        assert_eq!(run_tools(&env, &spec.id).unwrap(), vec!["uv", "npm"]);
        let run: crate::state::Running = serde_json::from_value(serde_json::json!({
            "id": spec.id, "harness": "fake", "project": project, "backend": "fake",
            "network": "none", "gui": "off", "pid": std::process::id(), "started": 1
        }))
        .unwrap();
        run.register(&env).unwrap();
        assert_eq!(
            holders(&env, &project),
            vec![
                ("lease:cache:npm".into(), spec.id.clone()),
                ("lease:cache:uv".into(), spec.id.clone()),
            ]
        );
        assert!(uv_dir(&env, &project).unwrap().is_dir());
        assert!(dir.join("npm").is_dir());
        assert!(lease_tool(&env, &dir, "uv", true).is_err());
        assert!(lease_tool(&env, &dir, "npm", true).is_err());
        drop(guard);
        assert!(run_tools(&env, &spec.id).is_none());
        assert!(holders(&env, &project).is_empty());
        assert!(lease_tool(&env, &dir, "uv", true).is_ok());
        assert!(lease_tool(&env, &dir, "npm", true).is_ok());
    }
}
