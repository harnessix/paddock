//! A cargo build cache per repository, for `--worktree` runs and spawned
//! workers.
//!
//! Without it, each worktree has its own `target/`, and each worker compiles
//! the whole project from nothing. With it, the cargo target folder of a
//! worktree sandbox lives outside the worktree:
//!
//! ```text
//! <paddock cache>/build/<repository slug>/
//!   project      the repository path, for gc (a gone repository)
//!   base/        the target folder of the newest sandbox that ended
//!   w/<id>/      the target folder of one sandbox (CARGO_TARGET_DIR)
//!   trash/       old bases and leftovers that the gc removes
//!   .lock        shared while paddock seeds, exclusive while it promotes
//! ```
//!
//! At the start, paddock clones `base/` into `w/<id>/` with copy-on-write
//! (`cp --reflink=always`), so the clone takes almost no space and no
//! compile. Without a base, the repository's own `target/` is the seed when
//! it has a `CACHEDIR.TAG`. At the end, `w/<id>/` becomes the new base.
//!
//! Why this design:
//!
//! - One shared `CARGO_TARGET_DIR` for all workers is not correct. cargo's
//!   lock lets one build run at a time, and between its build and its tests
//!   a worker can find `target/debug/<bin>` and the test binaries replaced by
//!   another worker that built another commit.
//! - Hard links from a shared base are not correct either: cargo rewrites its
//!   fingerprint files in place, so one worker's write changes the files of
//!   the base and of every other worker, and a fresh fingerprint can then
//!   sit next to an old artifact.
//! - sccache needs a tool that is not installed by default and a server in
//!   each sandbox, and it does not cache the incremental builds of the
//!   workspace crates.
//!
//! A copy-on-write clone per sandbox has none of these problems: each worker
//! has its own files, two builds of different commits never wait for each
//! other or see each other's output, and cargo's fingerprints decide what to
//! build again. On a file system without reflinks (ext4) a clone would be a
//! full copy, so paddock starts each sandbox with an empty folder and keeps
//! no base. The output still leaves the worktree, and goes at the end.
//!
//! Only the sandbox's own `w/<id>/` is mounted, at a fixed place in its home.
//! The base of a repository comes only from sandboxes of that repository, so
//! no project sees the cache of another. `[storage] build_cache = false`
//! turns it off, and `[setup] shared_build_cache = false` in the project's
//! `paddock.toml` turns it off for one project.

use super::settings::{Settings, GIB};
use crate::env::HostEnv;
use crate::spec::{Mount, SandboxSpec};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the target folder appears in the sandbox, relative to its home.
pub const DST: &str = ".cache/paddock-build/target";
pub const ORIGIN: &str = "build cache: the cargo target folder of this sandbox";
const PROJECT_FILE: &str = "project";
/// The longest a seed clone may take; then the sandbox starts with an
/// empty folder.
const CLONE_SECS: u64 = 120;
/// A worker folder of a sandbox that does not run is a leftover after this.
const LEFTOVER_SECS: u64 = 600;

pub fn root(env: &HostEnv) -> PathBuf {
    env.paddock_cache().join("build")
}

pub fn project_dir(env: &HostEnv, repo: &Path) -> PathBuf {
    root(env).join(crate::state::slug(repo))
}

/// One repository's build cache, for `paddock du`, the doctor and the gc.
#[derive(Debug, Clone, Serialize)]
pub struct CacheDir {
    pub path: PathBuf,
    pub project: Option<PathBuf>,
    /// Its repository no longer exists.
    pub orphan: bool,
    /// When the base was last made or used.
    pub last_used: u64,
    pub bytes: u64,
}

fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

fn mtime(p: &Path) -> u64 {
    std::fs::symlink_metadata(p)
        .map(|m| m.mtime().max(0) as u64)
        .unwrap_or(0)
}

fn touch(p: &Path) {
    let _ = std::fs::File::open(p).and_then(|f| f.set_modified(std::time::SystemTime::now()));
}

/// An flock on `<dir>/.lock`, held until it is dropped.
struct Lock(#[allow(dead_code)] std::fs::File);

fn lock(dir: &Path, exclusive: bool) -> Option<Lock> {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))
        .ok()?;
    let op = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    // SAFETY: flock on a file descriptor that this function owns.
    (unsafe { libc::flock(f.as_raw_fd(), op) } == 0).then_some(Lock(f))
}

/// The copy-on-write copy of a folder tree. On Linux `cp --reflink=always`
/// fails instead of copying the data.
fn clone_cmd(src: &Path, dst: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("cp");
    #[cfg(target_os = "macos")]
    c.args(["-a", "-c"]);
    #[cfg(not(target_os = "macos"))]
    c.args(["-a", "--reflink=always", "--"]);
    c.arg(src).arg(dst);
    c
}

/// The file system of `dir` shares data between copies.
pub fn can_clone(dir: &Path) -> bool {
    let tag = format!("{}-{}", std::process::id(), crate::state::now());
    let a = dir.join(format!(".probe-a-{tag}"));
    let b = dir.join(format!(".probe-b-{tag}"));
    let ok = std::fs::write(&a, b"paddock").is_ok()
        && crate::broker::run_capture(&mut clone_cmd(&a, &b), Duration::from_secs(10))
            .is_ok_and(|(code, _, _)| code == 0);
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    ok
}

/// A folder that git would take for a repository. A writable mount with
/// one at its top needs read-only git overrides (policy.rs), so such a
/// folder is never a seed or a base: the next start would fail.
fn looks_like_git(p: &Path) -> bool {
    std::fs::symlink_metadata(p.join(".git")).is_ok()
        || (p.join("HEAD").exists() && p.join("objects").exists())
}

/// The seed of a new worker folder: the base, else the repository's own
/// target folder when it says it is a cache.
fn seed_source(dir: &Path, repo: &Path) -> Option<PathBuf> {
    let base = dir.join("base");
    let own = repo.join("target");
    let src = if is_real_dir(&base) {
        base
    } else if is_real_dir(&own) && super::buildout::has_tag(&own) {
        own
    } else {
        return None;
    };
    (!looks_like_git(&src)).then_some(src)
}

/// Clone the seed into `w`. A clone that fails or takes too long leaves an
/// empty folder: the build is then slower, never wrong.
fn seed(dir: &Path, repo: &Path, w: &Path) {
    let _l = lock(dir, false);
    let Some(src) = seed_source(dir, repo) else {
        return;
    };
    if !can_clone(dir) {
        return;
    }
    let r = crate::broker::run_capture(&mut clone_cmd(&src, w), Duration::from_secs(CLONE_SECS));
    match r {
        Ok((0, _, _)) => {
            touch(&src);
            log::info!("build cache: cloned {} into {}", src.display(), w.display());
        }
        other => {
            log::warn!(
                "build cache: cannot clone {} ({other:?}); the sandbox starts with an empty target folder",
                src.display()
            );
            let _ = crate::safefs::remove_tree_path(w);
        }
    }
}

/// Give a worktree sandbox of a Rust project its own target folder from the
/// build cache of the repository. Nothing for other runs, for a project
/// that opts out, or on macOS (an app there runs under Seatbelt, which
/// cannot mount a folder at another place).
pub fn apply_with(
    env: &HostEnv,
    repo: &Path,
    worktree: bool,
    setup: &crate::manifest::SetupCfg,
    spec: &mut SandboxSpec,
) -> Result<()> {
    if !spec.project.join("Cargo.toml").is_file() {
        return Ok(());
    }
    let settings = Settings::load(env).0;
    if settings.lean_builds {
        lean(spec);
    }
    if cfg!(target_os = "macos")
        || !worktree
        || setup.shared_build_cache == Some(false)
        || !settings.build_cache
    {
        return Ok(());
    }
    let dir = project_dir(env, repo);
    crate::privfs::dir(&dir.join("w"))?;
    let name = dir.join(PROJECT_FILE);
    let text = repo.to_string_lossy();
    if std::fs::read_to_string(&name).ok().as_deref() != Some(&*text) {
        crate::privfs::write_atomic(&name, text.as_bytes())?;
    }
    let w = dir.join("w").join(&spec.id);
    if !w.exists() {
        seed(&dir, repo, &w);
    }
    crate::privfs::dir(&w)?;
    let dst = spec.home.join(DST);
    spec.add_mount(Mount::bind(&w, &dst, true, ORIGIN));
    spec.set_env("CARGO_TARGET_DIR", dst.display().to_string(), ORIGIN);
    Ok(())
}

/// The variables of `[storage] lean_builds`. A debug build of a large
/// crate writes gigabytes per run: the incremental folder is rewritten on
/// every build, and full debug information is most of the size of each
/// test binary. Line tables keep file and line numbers in backtraces.
pub const LEAN_ENV: &[(&str, &str)] = &[
    ("CARGO_INCREMENTAL", "0"),
    ("CARGO_PROFILE_DEV_DEBUG", "line-tables-only"),
    ("CARGO_PROFILE_TEST_DEBUG", "line-tables-only"),
];

fn lean(spec: &mut SandboxSpec) {
    for (k, v) in LEAN_ENV {
        if !spec.env.contains_key(*k) {
            spec.set_env(k, v.to_string(), ORIGIN);
        }
    }
}

/// [`apply_with`] for the sandbox of `project`: a run in a worktree that paddock
/// made shares the build cache of its repository like a `--worktree` run.
pub fn apply(
    env: &HostEnv,
    project: &Path,
    worktree: bool,
    setup: &crate::manifest::SetupCfg,
    spec: &mut SandboxSpec,
) -> Result<()> {
    let shared = worktree || in_paddock_worktree(env, project);
    apply_with(env, &crate::speckit::repo_of(project), shared, setup, spec)
}

/// True when `project` is a worktree that paddock made: a run there shares
/// the build cache of its repository like a `--worktree` run.
pub fn in_paddock_worktree(env: &HostEnv, project: &Path) -> bool {
    let root = env.paddock_data().join("worktrees");
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    std::fs::canonicalize(project).is_ok_and(|p| p.starts_with(&root) && p != root)
}

/// The worker folder of a sandbox, from its mounts.
fn worker_dir(env: &HostEnv, spec: &SandboxSpec) -> Option<PathBuf> {
    let w = spec
        .mounts
        .iter()
        .find(|m| m.origin == ORIGIN)?
        .src
        .clone()?;
    let ok = w.file_name()? == std::ffi::OsStr::new(&spec.id)
        && w.parent()?.file_name()? == "w"
        && w.parent()?.parent()?.parent()? == root(env);
    ok.then_some(w)
}

/// Move a folder into the cache's trash, for the gc to remove.
fn to_trash(dir: &Path, p: &Path, tag: &str) -> std::io::Result<()> {
    let trash = dir.join("trash");
    crate::privfs::dir(&trash)?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("{tag}-{}-{}-{n}", crate::state::now(), std::process::id());
    std::fs::rename(p, trash.join(name))
}

/// After the sandbox ended: its target folder becomes the base of the
/// repository, and the old base goes to the trash. Without reflinks there
/// is no base: the folder goes to the trash. Then a background gc empties
/// the trash.
pub fn after_run(env: &HostEnv, spec: &SandboxSpec) {
    let Some(w) = worker_dir(env, spec) else {
        return;
    };
    let Some(dir) = w.parent().and_then(Path::parent).map(Path::to_path_buf) else {
        return;
    };
    let r = (|| -> std::io::Result<()> {
        let _l = lock(&dir, true);
        if !is_real_dir(&w) {
            return Ok(());
        }
        let base = dir.join("base");
        if !can_clone(&dir) || looks_like_git(&w) {
            return to_trash(&dir, &w, &spec.id);
        }
        if is_real_dir(&base) {
            to_trash(&dir, &base, "base")?;
        }
        std::fs::rename(&w, &base)?;
        touch(&base);
        Ok(())
    })();
    if let Err(e) = r {
        log::warn!("build cache: cannot keep {}: {e}", w.display());
    }
    super::auto::spawn(env, &["--build-cache"]);
}

/// The build caches, without sizes.
pub fn scan(env: &HostEnv) -> Vec<CacheDir> {
    let mut v: Vec<CacheDir> = super::subdirs(&root(env))
        .into_iter()
        .map(|path| {
            let project = std::fs::read_to_string(path.join(PROJECT_FILE))
                .ok()
                .map(PathBuf::from);
            CacheDir {
                orphan: project.as_ref().is_none_or(|p| !p.is_dir()),
                project,
                last_used: mtime(&path.join("base")).max(mtime(&path.join("w"))),
                bytes: 0,
                path,
            }
        })
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

/// What the gc removes from the build caches: the trash, worker folders of
/// sandboxes that do not run, the caches of gone repositories, and the
/// least recently used bases while all of them are over the size limit.
/// `size` measures a folder.
pub fn plan(
    env: &HostEnv,
    s: &Settings,
    running: &BTreeSet<String>,
    now: u64,
    size: &dyn Fn(&Path) -> u64,
) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut bases = Vec::new();
    for c in scan(env) {
        if c.orphan {
            out.push((c.path.clone(), "its repository is gone".into()));
            continue;
        }
        for t in super::subdirs(&c.path.join("trash")) {
            out.push((t, "an old build cache".into()));
        }
        for w in super::subdirs(&c.path.join("w")) {
            let id = super::name(&w);
            if !running.contains(&id) && mtime(&w) + LEFTOVER_SECS < now {
                out.push((w, "the target folder of a sandbox that ended".into()));
            }
        }
        let base = c.path.join("base");
        if is_real_dir(&base) {
            bases.push((mtime(&base), size(&base), base));
        }
    }
    if s.build_cache_gib > 0 {
        let limit = s.build_cache_gib.saturating_mul(GIB);
        let mut total: u64 = bases.iter().map(|b| b.1).sum();
        bases.sort();
        for (_, bytes, base) in bases {
            if total <= limit {
                break;
            }
            total -= bytes;
            out.push((
                base,
                format!(
                    "the build caches are over [storage] build_cache_gib = {}",
                    s.build_cache_gib
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "buildcache_tests.rs"]
mod tests;
