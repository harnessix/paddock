use super::*;
use crate::env::HostEnv;
use crate::recover::Session;
use crate::state::{self, AuthMode, Running};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// The branches that paddock made for its worktrees, one file per worktree,
/// on the host where no sandbox writes.
fn worktree_branches_root(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("worktree-branches")
}

/// The record of a worktree: `<slug>/<id>` below the worktree folder.
fn worktree_branch_file(env: &HostEnv, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(worktrees_root(env)).ok()?;
    (rel.components().count() == 2).then(|| worktree_branches_root(env).join(rel))
}

/// Record the branch that paddock made for the worktree at `path`. The
/// worktree scan trusts this record, not the branch that the worktree is on
/// now: a sandbox can switch branches there.
pub fn record_worktree_branch(env: &HostEnv, path: &Path, branch: &str) {
    let Some(f) = worktree_branch_file(env, path) else {
        return;
    };
    let r = f
        .parent()
        .map_or(Ok(()), crate::privfs::dir)
        .and_then(|_| crate::privfs::write(&f, branch));
    if let Err(e) = r {
        log::warn!("cannot record the branch of {}: {e}", path.display());
    }
    if let Err(e) = lease_worktree(env, path) {
        log::warn!("cannot lease {}: {e:#}", path.display());
    }
}

/// The lease of a worktree: a file that the process which made the
/// worktree holds with a shared flock until it exits. The cleanup leaves a
/// worktree alone while another process holds it, also before the sandbox
/// registers as running. A crash releases it. The folder is on tmpfs, so a
/// reboot clears it.
fn lease_file(env: &HostEnv, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(worktrees_root(env)).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (parts.len() == 2).then(|| {
        env.paddock_run()
            .join("worktree-leases")
            .join(parts.join("--"))
    })
}

/// The leases this process holds, until it exits.
type Leases = Vec<(PathBuf, std::fs::File)>;
static LEASES: std::sync::Mutex<Leases> = std::sync::Mutex::new(Vec::new());

fn leased_here(f: &Path) -> bool {
    let held = LEASES.lock().unwrap_or_else(|e| e.into_inner());
    held.iter().any(|(p, _)| p == f)
}

fn lease_worktree(env: &HostEnv, path: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;
    let f = lease_file(env, path).context("not a paddock worktree")?;
    crate::privfs::dir(f.parent().context("no parent")?)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&f)?;
    // SAFETY: flock on a file descriptor that this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } != 0 {
        bail!("cannot lock {}", f.display());
    }
    LEASES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((f, file));
    Ok(())
}

/// Drop the leases of this process, as its exit does.
#[cfg(test)]
pub fn drop_leases_for_test(env: &HostEnv, path: &Path) {
    let f = lease_file(env, path);
    let mut held = LEASES.lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|(p, _)| Some(p) != f.as_ref());
}

/// Another process that made the worktree still runs. The lease of this
/// process does not count: it ends its own run.
pub fn worktree_in_use(env: &HostEnv, path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Some(f) = lease_file(env, path) else {
        return false;
    };
    if leased_here(&f) {
        return false;
    }
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(&f) else {
        return false;
    };
    // SAFETY: flock on a file descriptor that this function owns; closing
    // it drops the lock again.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
}

pub(super) fn recorded_worktree_branch(env: &HostEnv, path: &Path) -> Option<String> {
    let f = worktree_branch_file(env, path)?;
    let b = std::fs::read_to_string(f).ok()?.trim().to_string();
    (!b.is_empty()).then_some(b)
}

fn forget_worktree_branch(env: &HostEnv, path: &Path) {
    if let Some(l) = lease_file(env, path) {
        let _ = std::fs::remove_file(l);
    }
    if let Some(f) = worktree_branch_file(env, path) {
        let _ = std::fs::remove_file(&f);
        if let Some(parent) = f.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Remove the homes of `paddock doctor --escape` runs that are over: the
/// verified origin names a project that is gone, and no sandbox runs in the
/// home. Each run has its own scratch project, so each run has its own home.
/// A home without a verified origin stays for `paddock gc`. Returns the
/// removed homes.
pub fn remove_doctor_homes(env: &HostEnv) -> Vec<PathBuf> {
    let running: BTreeSet<PathBuf> = Running::list(env)
        .into_iter()
        .filter(|r| r.harness == DOCTOR_HARNESS)
        .filter_map(|r| {
            let auth = r.auth.parse::<AuthMode>().ok()?;
            Some(
                homes_root(env)
                    .join(DOCTOR_HARNESS)
                    .join(login_dir(&auth))
                    .join(state::slug(&r.project)),
            )
        })
        .collect();
    let mut removed = Vec::new();
    for ldir in subdirs(&homes_root(env).join(DOCTOR_HARNESS)) {
        let Some(auth) = auth_from_dir(&name(&ldir)) else {
            continue;
        };
        let auth = auth.to_string();
        for path in subdirs(&ldir) {
            if running.contains(&path) {
                continue;
            }
            let over = read_origin(&path).is_some_and(|o| {
                origin_matches(&o, DOCTOR_HARNESS, &auth, &name(&path)) && !o.project.exists()
            });
            if over && crate::safefs::remove_tree_path(&path).is_ok() {
                removed.push(path);
            }
        }
    }
    removed
}

/// Refuse a path that is not strictly inside one of paddock's own
/// directories, that is a symlink, or whose parents lead out through a symlink.
/// Returns that directory, as a real path, and the path below it.
pub fn check_removable(env: &HostEnv, path: &Path) -> Result<(PathBuf, PathBuf)> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!("{} is not a plain absolute path", path.display());
    }
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("{} does not exist", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!(
            "{} is a symlink; paddock does not follow it",
            path.display()
        );
    }
    let (Some(parent), Some(file)) = (path.parent(), path.file_name()) else {
        bail!("{} has no parent", path.display());
    };
    let parent = std::fs::canonicalize(parent)?;
    // An old shared cache: only the exact folders that paddock replaced.
    if let Ok(shared) = std::fs::canonicalize(shared_cache(env)) {
        if parent == shared && OLD_CACHES.iter().any(|(old, _)| file == *old) {
            return Ok((shared, PathBuf::from(file)));
        }
    }
    for root in removable_roots(env) {
        let Ok(root) = std::fs::canonicalize(&root) else {
            continue;
        };
        if let Ok(rel) = parent.join(file).strip_prefix(&root) {
            if !rel.as_os_str().is_empty() {
                return Ok((root.clone(), rel.to_path_buf()));
            }
        }
    }
    bail!("{} is outside paddock's own directories", path.display())
}

/// Delete a file or a tree after `check_removable`. Never follows a symlink:
/// the walk starts at paddock's own folder and opens one step at a time, and
/// read-only folders (a Go module cache) get write permission through the
/// open folder, not by path (see safefs).
pub fn remove_path(env: &HostEnv, path: &Path) -> Result<()> {
    let (root, rel) = check_removable(env, path)?;
    crate::safefs::Root::open(&root)?
        .remove_tree(&rel)
        .with_context(|| format!("remove {}", path.display()))
}

/// Remove a worktree that paddock made, and its branch with `delete_branch`.
/// Gives back a note when it kept the branch.
pub fn remove_worktree(
    env: &HostEnv,
    path: &Path,
    repo: Option<&Path>,
    branch: Option<&str>,
    delete_branch: bool,
    force: bool,
) -> Result<Option<String>> {
    check_removable(env, path)?;
    let mut note = None;
    match repo {
        Some(repo) => {
            let p = path.display().to_string();
            let mut a = vec!["worktree", "remove"];
            if force {
                a.push("--force");
            }
            a.push(&p);
            // The worktree itself is sandbox-written: its own
            // configuration must be harmless too, because the
            // removal reads that work tree.
            if let Ok(Some(l)) = crate::hostgit::Layout::discover(path) {
                l.guard()?;
            }
            // One worktree change per repository at a time: two removals
            // or a removal and an add at once fail on each other.
            let co = crate::hostgit::Checkout::open(repo)?;
            let lock = co.layout.lock_worktrees(env)?;
            co.run_worktree(&lock, &a)?;
            match branch {
                Some(b) if delete_branch => {
                    co.run_worktree(&lock, &["branch", "-D", b])?;
                }
                Some(b) => note = Some(format!("kept the branch {b}")),
                None => {}
            }
        }
        None => remove_path(env, path)?,
    }
    forget_worktree_branch(env, path);
    // The per-repository folder, when it is empty now.
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(note)
}

/// The result of one removal.
#[derive(Debug, Clone, Serialize)]
pub struct Removed {
    pub what: String,
    pub bytes: u64,
    pub error: Option<String>,
    pub note: Option<String>,
}

fn layer_run<'a>(env: &HostEnv, path: &Path, running: &'a [Running]) -> Option<&'a Running> {
    let marker = std::fs::read_to_string(path.join("project")).ok();
    running.iter().find(|run| {
        layers::project_dir(env, &run.project) == path
            || marker
                .as_deref()
                .is_some_and(|p| Path::new(p) == run.project)
    })
}

fn shared_cache_run<'a>(env: &HostEnv, path: &Path, running: &'a [Running]) -> Option<&'a Running> {
    let marker = std::fs::read_to_string(path.join("project")).ok();
    running.iter().find(|run| {
        std::iter::once(run.project.as_path())
            .chain(run.kv_project.as_deref())
            .any(|project| {
                let identity = super::package_cache::identity(project).ok();
                identity.as_ref().is_some_and(|p| {
                    super::package_cache::root(env).join(state::slug(p)) == path
                        || marker.as_deref().is_some_and(|m| Path::new(m) == p)
                }) || marker.as_deref().is_some_and(|m| Path::new(m) == project)
                    || super::package_cache::root(env).join(state::slug(project)) == path
            })
    })
}

/// Remove the selected items. The running sandboxes are read again first, so
/// one that started after the scan keeps its home.
pub fn apply(env: &HostEnv, containers: &dyn Containers, items: &[Item]) -> Vec<Removed> {
    let running: Vec<Running> = Running::list(env);
    let running_homes: BTreeSet<PathBuf> = Session::all(env)
        .into_iter()
        .filter(|s| running.iter().any(|r| r.id == s.id))
        .map(|s| s.home)
        .chain(running.iter().filter_map(|r| {
            let auth: AuthMode = r.auth.parse().ok()?;
            Some(
                homes_root(env)
                    .join(&r.harness)
                    .join(login_dir(&auth))
                    .join(state::slug(&r.project)),
            )
        }))
        .collect();
    let mut out = Vec::new();
    for i in items.iter().filter(|i| i.selected()) {
        let mut note = None;
        let r: Result<()> = match &i.action {
            Action::Image { cli, tags } => tags
                .iter()
                .try_for_each(|t| containers.run(cli, &["rmi", t]).map(|_| ())),
            // Only the records that paddock found: one id filter each.
            Action::BuildCache { cli, ids } => ids.iter().try_for_each(|id| {
                let f = format!("id={id}");
                containers
                    .run(cli, &["builder", "prune", "--force", "--filter", &f])
                    .map(|_| ())
            }),
            // Every sandbox may mount the global lower layer; its ownership is not per project.
            Action::Path { path }
                if !running.is_empty() && path.parent() == Some(shared_cache(env).as_path()) =>
            {
                Err(anyhow::anyhow!("a sandbox runs; skipped"))
            }
            Action::Path { path } if running_homes.contains(path) => {
                Err(anyhow::anyhow!("a sandbox started in it; skipped"))
            }
            Action::Path { path }
                if path.parent() == Some(layers::projects_root(env).as_path()) =>
            {
                if let Some(run) = layer_run(env, path, &running) {
                    Err(anyhow::anyhow!("in use by run {}; skipped", run.id))
                } else {
                    super::package_cache::gc::remove(env, path)
                }
            }
            Action::Path { path }
                if path.parent() == Some(super::package_cache::root(env).as_path()) =>
            {
                if let Some(run) = shared_cache_run(env, path, &running) {
                    Err(anyhow::anyhow!("in use by run {}; skipped", run.id))
                } else {
                    super::package_cache::gc::remove_shared(env, path)
                }
            }
            Action::Path { path } => remove_path(env, path),
            Action::Caches { home, .. } if running_homes.contains(home) => {
                Err(anyhow::anyhow!("a sandbox started in it; skipped"))
            }
            // The walk starts at paddock's own folder and never follows a
            // symlink that the sandbox planted on the way.
            Action::Caches { home, folders } => (|| {
                let (root, rel) = check_removable(env, home)?;
                let root = crate::safefs::Root::open(&root)?;
                for f in folders {
                    root.remove_tree(&rel.join(f))
                        .with_context(|| format!("remove {}", home.join(f).display()))?;
                }
                Ok(())
            })(),
            Action::Worktree { path, .. } | Action::BuildOutput { worktree: path, .. }
                if running.iter().any(|r| r.project == *path) || worktree_in_use(env, path) =>
            {
                Err(anyhow::anyhow!("a sandbox started in it; skipped"))
            }
            Action::BuildOutput { worktree, folders } => (|| {
                let (root, rel) = check_removable(env, worktree)?;
                let root = crate::safefs::Root::open(&root)?;
                for f in folders {
                    // Checked again: the folder must still say it is a cache.
                    if !super::buildout::has_tag(&worktree.join(f)) {
                        continue;
                    }
                    root.remove_tree(&rel.join(f))
                        .with_context(|| format!("remove {}", worktree.join(f).display()))?;
                }
                Ok(())
            })(),
            Action::View { id } => crate::view::remove(env, id),
            Action::LoftTrash { dir } => crate::loft::trash::remove(env, dir),
            Action::Macvm { dir } => crate::macvm::manage::gc_remove(env, dir),
            Action::Kv { sandboxes } => {
                crate::kv::gc_apply(env, sandboxes, state::now()).map(|_| ())
            }
            Action::Worktree {
                path,
                repo,
                branch,
                delete_branch,
                force,
            } => remove_worktree(
                env,
                path,
                repo.as_deref(),
                branch.as_deref(),
                *delete_branch,
                *force,
            )
            .map(|n| note = n),
        };
        out.push(Removed {
            what: i.what.clone(),
            bytes: i.bytes,
            error: r.err().map(|e| format!("{e:#}")),
            note,
        });
    }
    out
}

/// The automatic tier, after a new base image was built. `PADDOCK_GC_AFTER_BUILD=0`
/// turns it off.
pub fn after_build(env: &HostEnv, tag: &str) {
    if env.var("PADDOCK_GC_AFTER_BUILD") == Some("0") {
        return;
    }
    let containers = RealContainers(env);
    let mut host = Host::real(env, &containers);
    host.current_tags.insert(tag.to_string());
    let mut inv = scan(env, &host, false);
    let mut items = plan(
        &inv,
        &GcOptions {
            yes: true,
            older_than_days: DEFAULT_OLDER_THAN,
            ..Default::default()
        },
        state::now(),
    );
    items.retain(|i| i.selected());
    // The macOS guest folder: its automatic tier too.
    for line in crate::macvm::manage::auto_cleanup(env) {
        eprintln!("paddock: gc: {line}");
    }
    if items.is_empty() {
        return;
    }
    measure_plan(&mut inv, &mut items);
    for r in apply(env, &containers, &items) {
        match r.error {
            None => eprintln!("paddock: gc: removed {} ({})", r.what, human(r.bytes)),
            Some(e) => log::warn!("gc: cannot remove {}: {e}", r.what),
        }
    }
}
