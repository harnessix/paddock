//! The git worktree that paddock creates for one run (`--worktree`), and
//! the report of what the run left in it.

use crate::env::HostEnv;
use crate::state;
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

/// A worktree that paddock created for one run.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
}

/// A worktree for one run. `app` names the branch after an app:
/// `paddock/<app>-<date>-<id>`. Terminal runs get `paddock/<id>`.
pub fn create_worktree(env: &HostEnv, repo: &Path, app: Option<&str>) -> Result<Worktree> {
    let layout = crate::hostgit::Layout::discover(repo)?;
    if layout.as_ref().map(|l| l.work_tree.as_path()) != Some(repo) {
        bail!(
            "--worktree needs a git repository: {} is not one",
            repo.display()
        );
    }
    let base = layout
        .as_ref()
        .unwrap()
        .query(&["rev-parse", "HEAD"])
        .map(|s| s.trim().to_string())
        .context("--worktree needs at least one commit")?;
    let id = state::new_id("wt", repo);
    let branch = match app {
        Some(a) => format!(
            "paddock/{a}-{}-{}",
            today(),
            id.rsplit('-').next().unwrap_or("0")
        ),
        None => format!("paddock/{id}"),
    };
    let path = env
        .paddock_data()
        .join("worktrees")
        .join(state::slug(repo))
        .join(&id);
    crate::privfs::dir(path.parent().unwrap())?;
    // Two runs that start together must not run into each other's git lock
    // files: one worktree change per repository at a time.
    let co = crate::hostgit::Checkout::open(repo)?;
    let lock = co.layout.lock_worktrees(env)?;
    co.run_worktree(
        &lock,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            &path.display().to_string(),
            &base,
        ],
    )?;
    crate::storage::record_worktree_branch(env, &path, &branch);
    Ok(Worktree {
        repo: repo.to_path_buf(),
        path,
        branch,
        base,
    })
}

/// Remove a worktree that paddock created, and its branch. Used when the
/// build or `explain` ends without a run.
pub fn discard_worktree(env: &HostEnv, w: &Worktree) {
    if let Err(e) =
        crate::storage::remove_worktree(env, &w.path, Some(&w.repo), Some(&w.branch), true, true)
    {
        log::warn!(
            "cannot remove leftover worktree {}: {e:#}",
            w.path.display()
        );
    }
}

/// What the run left in its worktree: commits, uncommitted changes and the
/// commands to review, merge or remove it. Empty lines are dropped.
pub fn worktree_report(env: &HostEnv, w: &Worktree) -> String {
    let repo = crate::hostgit::Layout::discover(&w.repo).ok().flatten();
    let query = |args: &[&str]| {
        repo.as_ref()
            .and_then(|l| l.try_query(args))
            .map(|s| s.trim_end().to_string())
            .unwrap_or_default()
    };
    let range = format!("{}..{}", w.base, w.branch);
    let commits = query(&["log", "--oneline", &range]);
    // `--no-textconv` keeps a textconv program in the repository
    // configuration from running on the host.
    let stat = query(&[
        "diff",
        "--stat",
        "--no-textconv",
        "--no-ext-diff",
        &format!("{}...{}", w.base, w.branch),
    ]);
    // The work tree is read in a sandbox: `git status` runs clean filters,
    // which no `-c` flag can turn off.
    let dirty = match worktree_status(env, &w.path) {
        Ok(Some(lines)) => lines.join("\n"),
        Ok(None) => String::new(),
        Err(e) => format!("paddock cannot read the worktree state: {e:#}"),
    };
    let mut t = format!(
        "paddock: the sandbox worked in {} on branch {}.\n",
        w.path.display(),
        w.branch
    );
    if commits.is_empty() && dirty.is_empty() {
        t.push_str("paddock: it made no changes.");
        return t;
    }
    if !commits.is_empty() {
        t.push_str(&format!("commits:\n{commits}\n{stat}\n"));
    }
    if !dirty.is_empty() {
        t.push_str(&format!("uncommitted changes in the worktree:\n{dirty}\n"));
    }
    t.push_str(&format!(
        "review:  git -C {r} diff {b}...{br}\nmerge:   git -C {r} merge {br}\nremove:  git -C {r} worktree remove --force {p} && git -C {r} branch -D {br}",
        r = w.repo.display(),
        b = &w.base[..w.base.len().min(12)],
        br = w.branch,
        p = w.path.display()
    ));
    t
}

/// The state of a worktree that a sandbox wrote. `Ok(None)` means this host
/// cannot run the check in a sandbox, so the state stays unknown.
pub fn worktree_status(env: &HostEnv, path: &Path) -> Result<Option<Vec<String>>> {
    let Some(layout) = crate::hostgit::Layout::discover(path)? else {
        return Ok(Some(vec![]));
    };
    layout.status(env, "normal")
}

/// One line for a notification: how many commits and changed files. The
/// change count is None when the worktree state could not be read.
pub fn worktree_counts(env: &HostEnv, w: &Worktree) -> (usize, Option<usize>) {
    let commits = crate::hostgit::Layout::discover(&w.repo)
        .ok()
        .flatten()
        .and_then(|l| l.try_query(&["rev-list", &format!("{}..{}", w.base, w.branch)]))
        .map(|s| s.lines().count())
        .unwrap_or(0);
    let changed = worktree_status(env, &w.path)
        .ok()
        .flatten()
        .map(|lines| lines.iter().filter(|l| !l.trim().is_empty()).count());
    (commits, changed)
}

/// Today's date in UTC as YYYYMMDD, for branch names.
pub(super) fn today() -> String {
    ymd(state::now())
}

pub(crate) fn ymd(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}
