//! Whether the work of a paddock worktree is in another branch, so the
//! automatic pass can remove the worktree.
//!
//! A commit counts as merged when a local branch outside `paddock/` has it
//! (a merge or a fast-forward), or has a commit with the same patch id (a
//! cherry-pick or a rebase), as `git cherry` compares them. Branches under
//! `paddock/` do not count: they belong to other worktrees that the same pass
//! may remove, and two of them that point at the same commit would make each
//! other look merged. Remote branches do not count either: a fetch can take
//! them away.
//!
//! Every call is a hardened query on refs and objects (see hostgit.rs).

use crate::hostgit::Layout;
use std::collections::BTreeSet;

/// The branches that `git cherry` compares with, newest first.
const MAX_BRANCHES: usize = 64;

/// The commits of `head` whose change no local branch outside `paddock/`
/// has. Empty: all of it is merged. None when git fails, so the state is
/// unknown.
pub fn left_over(layout: &Layout, head: &str) -> Option<Vec<String>> {
    let out = layout.try_query(&[
        "rev-list",
        head,
        "--not",
        "--exclude=paddock/*",
        "--branches",
        "--",
    ])?;
    let mut left: BTreeSet<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if left.is_empty() {
        return Some(vec![]);
    }
    let refs = layout.try_query(&[
        "for-each-ref",
        "--sort=-committerdate",
        "--format=%(refname)",
        "refs/heads/",
    ])?;
    let others = refs
        .lines()
        .filter(|r| !r.is_empty() && !r.starts_with("refs/heads/paddock/"))
        .take(MAX_BRANCHES);
    for r in others {
        // `-` marks a commit of `head` with an equivalent change in `r`.
        let out = layout.try_query(&["cherry", r, head])?;
        for l in out.lines() {
            if let Some(sha) = l.strip_prefix("- ") {
                left.remove(sha.trim());
            }
        }
        if left.is_empty() {
            break;
        }
    }
    Some(left.into_iter().collect())
}

/// Merged state of a worktree: of its recorded branch and of its HEAD,
/// which a sandbox can move to another branch or detach. Both must be merged.
pub fn worktree_merged(layout: &Layout, branch: Option<&str>, head: Option<&str>) -> Option<bool> {
    let branch = branch.map(|b| format!("refs/heads/{b}"));
    let heads: Vec<&str> = branch.iter().map(String::as_str).chain(head).collect();
    if heads.is_empty() {
        return None;
    }
    for h in heads {
        if !left_over(layout, h)?.is_empty() {
            return Some(false);
        }
    }
    Some(true)
}

/// Commits on `branch` after `base`. None when git fails.
pub fn commits_beyond(layout: &Layout, base: &str, branch: &str) -> Option<usize> {
    layout
        .try_query(&[
            "rev-list",
            "--count",
            &format!("{base}..refs/heads/{branch}"),
            "--",
        ])
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn repo() -> (tempfile::TempDir, Layout) {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().join("repo");
        std::fs::create_dir_all(&r).unwrap();
        git(&r, &["init", "-q", "-b", "main"]);
        std::fs::write(r.join("a"), "a\n").unwrap();
        git(&r, &["add", "a"]);
        git(&r, &["commit", "-q", "-m", "a"]);
        let l = Layout::discover(&r).unwrap().unwrap();
        (t, l)
    }

    fn commit_on(l: &Layout, branch: &str, file: &str) {
        let r = &l.work_tree;
        git(r, &["switch", "-q", branch]);
        std::fs::write(r.join(file), format!("{file}\n")).unwrap();
        git(r, &["add", file]);
        git(r, &["commit", "-q", "-m", file]);
        git(r, &["switch", "-q", "main"]);
    }

    #[test]
    fn a_merged_branch_and_a_fresh_branch_count_as_merged() {
        let (_t, l) = repo();
        let r = l.work_tree.clone();
        git(&r, &["branch", "paddock/fresh"]);
        assert_eq!(worktree_merged(&l, Some("paddock/fresh"), None), Some(true));
        let base = git(&r, &["rev-parse", "HEAD"]);
        assert_eq!(commits_beyond(&l, &base, "paddock/fresh"), Some(0));

        git(&r, &["branch", "paddock/work"]);
        commit_on(&l, "paddock/work", "b");
        assert_eq!(commits_beyond(&l, &base, "paddock/work"), Some(1));
        assert_eq!(worktree_merged(&l, Some("paddock/work"), None), Some(false));
        git(&r, &["merge", "-q", "--no-edit", "paddock/work"]);
        assert_eq!(worktree_merged(&l, Some("paddock/work"), None), Some(true));
    }

    #[test]
    fn a_cherry_picked_commit_counts_as_merged_by_its_patch_id() {
        let (_t, l) = repo();
        let r = l.work_tree.clone();
        git(&r, &["branch", "paddock/pick"]);
        commit_on(&l, "paddock/pick", "b");
        commit_on(&l, "paddock/pick", "c");
        // main moves on, so the picks get new commit ids.
        std::fs::write(r.join("m"), "m\n").unwrap();
        git(&r, &["add", "m"]);
        git(&r, &["commit", "-q", "-m", "m"]);
        let tip = git(&r, &["rev-parse", "paddock/pick"]);
        git(&r, &["cherry-pick", &format!("{tip}~1")]);
        // One of two commits is picked: not merged yet.
        let left = left_over(&l, "refs/heads/paddock/pick").unwrap();
        assert_eq!(left, vec![tip.clone()]);
        assert_eq!(worktree_merged(&l, Some("paddock/pick"), None), Some(false));
        // The second one lands on another local branch, not main.
        git(&r, &["switch", "-q", "-c", "release"]);
        git(&r, &["cherry-pick", &tip]);
        git(&r, &["switch", "-q", "main"]);
        assert!(left_over(&l, "refs/heads/paddock/pick").unwrap().is_empty());
        assert_eq!(worktree_merged(&l, Some("paddock/pick"), None), Some(true));
    }

    #[test]
    fn other_paddock_branches_and_a_moved_head_do_not_count() {
        let (_t, l) = repo();
        let r = l.work_tree.clone();
        git(&r, &["branch", "paddock/a"]);
        commit_on(&l, "paddock/a", "b");
        // A second worktree branch at the same commit: neither is merged.
        git(&r, &["branch", "paddock/b", "paddock/a"]);
        assert_eq!(worktree_merged(&l, Some("paddock/a"), None), Some(false));
        assert_eq!(worktree_merged(&l, Some("paddock/b"), None), Some(false));

        // The recorded branch is merged, but the worktree's HEAD has a commit
        // of its own (a sandbox detached it and committed).
        git(&r, &["branch", "paddock/c"]);
        git(&r, &["switch", "-q", "--detach", "main"]);
        std::fs::write(r.join("d"), "d\n").unwrap();
        git(&r, &["add", "d"]);
        git(&r, &["commit", "-q", "-m", "d"]);
        let head = git(&r, &["rev-parse", "HEAD"]);
        git(&r, &["switch", "-q", "main"]);
        assert_eq!(worktree_merged(&l, Some("paddock/c"), None), Some(true));
        assert_eq!(
            worktree_merged(&l, Some("paddock/c"), Some(&head)),
            Some(false)
        );
        // A branch that git does not know: unknown, never merged.
        assert_eq!(worktree_merged(&l, Some("paddock/none"), None), None);
        assert_eq!(worktree_merged(&l, None, None), None);
    }
}
