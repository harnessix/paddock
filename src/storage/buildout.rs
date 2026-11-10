//! Build output in a worktree that the automatic pass may drop.
//!
//! Only a folder that says it is a cache counts: one with a valid
//! `CACHEDIR.TAG` (<https://bford.info/cachedir/>). cargo writes one into
//! `target/`, and pytest, mypy and ruff write one into their caches. The
//! folder must also be ignored by git and hold no tracked file. paddock never
//! deletes a tracked file, or an untracked file that git does not ignore.
//!
//! The worktree was written by a sandbox: the walk never follows a symlink,
//! and the git calls are the hardened ones with the configuration check of
//! hostgit.rs. `ls-files` and `check-ignore` read the index and the ignore
//! files, never file content, so no filter runs.

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const TAG: &str = "CACHEDIR.TAG";
const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
/// How deep the walk looks for tagged folders below the worktree.
const MAX_DEPTH: usize = 4;
/// Folders the walk opens at most, so a huge tree cannot stall the pass.
const MAX_DIRS: usize = 20_000;

/// A `CACHEDIR.TAG` file, not a link, that starts with the signature.
pub fn has_tag(dir: &Path) -> bool {
    let f = dir.join(TAG);
    if !std::fs::symlink_metadata(&f).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&f)
    else {
        return false;
    };
    let mut head = Vec::with_capacity(SIGNATURE.len());
    file.take(SIGNATURE.len() as u64)
        .read_to_end(&mut head)
        .is_ok()
        && head == SIGNATURE
}

/// Tagged folders below `root`, relative to it. The walk skips `.git`,
/// never follows a symlink and does not enter a tagged folder.
pub fn tagged_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(PathBuf::new(), 0usize)];
    let mut opened = 0;
    while let Some((rel, depth)) = stack.pop() {
        opened += 1;
        if opened > MAX_DIRS {
            break;
        }
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in rd.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_dir()) || e.file_name() == ".git" {
                continue;
            }
            let child = rel.join(e.file_name());
            if has_tag(&root.join(&child)) {
                out.push(child);
            } else if depth + 1 < MAX_DEPTH {
                stack.push((child, depth + 1));
            }
        }
    }
    out.sort();
    out
}

/// The tagged folders of a worktree that git ignores and that hold no
/// tracked file, relative to the worktree. Empty when git cannot tell.
pub fn droppable(worktree: &Path) -> Vec<PathBuf> {
    let candidates = tagged_dirs(worktree);
    if candidates.is_empty() {
        return vec![];
    }
    let Ok(co) = crate::hostgit::Checkout::open(worktree) else {
        return vec![];
    };
    if co.layout.work_tree != worktree || !co.layout.linked {
        return vec![];
    }
    candidates
        .into_iter()
        .filter(|rel| {
            let r = rel.to_string_lossy();
            let tracked = co.run(&["ls-files", "-z", "--", &r]);
            // `check-ignore` exits 1 for a path that is not ignored, which
            // `run` turns into an error.
            let ignored = co.run(&["check-ignore", "-q", "--", &r]).is_ok();
            matches!(tracked, Ok(t) if t.is_empty()) && ignored
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::merged::tests::git;

    fn tag(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(TAG),
            "Signature: 8a477f597d28d172789f06886806bc55\n# cargo\n",
        )
        .unwrap();
    }

    #[test]
    fn only_a_valid_tag_that_is_no_link_counts() {
        let t = tempfile::tempdir().unwrap();
        let ok = t.path().join("ok");
        tag(&ok);
        assert!(has_tag(&ok));
        let bad = t.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(TAG), "Signature: something else\n").unwrap();
        assert!(!has_tag(&bad));
        let link = t.path().join("link");
        std::fs::create_dir_all(&link).unwrap();
        std::os::unix::fs::symlink(ok.join(TAG), link.join(TAG)).unwrap();
        assert!(!has_tag(&link));
    }

    #[test]
    fn the_walk_finds_tagged_folders_and_skips_links_and_git() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        tag(&r.join("target"));
        tag(&r.join("target/debug/inner"));
        tag(&r.join("py/.pytest_cache"));
        tag(&r.join(".git/x"));
        tag(&r.join("a/b/c/d/e/deep"));
        let outside = tempfile::tempdir().unwrap();
        tag(&outside.path().join("victim"));
        std::os::unix::fs::symlink(outside.path(), r.join("out")).unwrap();
        assert_eq!(
            tagged_dirs(r),
            vec![PathBuf::from("py/.pytest_cache"), PathBuf::from("target")]
        );
    }

    #[test]
    fn only_ignored_folders_without_tracked_files_are_droppable() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), "/target\n/tracked-cache/\n").unwrap();
        // A cache folder with a tracked file inside: kept.
        tag(&repo.join("tracked-cache"));
        std::fs::write(repo.join("tracked-cache/keep.txt"), "x\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(&repo, &["add", "-f", "tracked-cache/keep.txt"]);
        git(&repo, &["commit", "-q", "-m", "a"]);
        let wt = t.path().join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "w",
                &wt.display().to_string(),
            ],
        );
        tag(&wt.join("target"));
        std::fs::write(wt.join("target/out.bin"), "x").unwrap();
        tag(&wt.join("tracked-cache"));
        // A tagged folder that git does not ignore: untracked, kept.
        tag(&wt.join("notes"));
        std::fs::write(wt.join("notes/mine.txt"), "mine\n").unwrap();
        assert_eq!(droppable(&wt), vec![PathBuf::from("target")]);
        // The main checkout is not a linked worktree: nothing.
        tag(&repo.join("target"));
        assert!(droppable(&repo).is_empty());
    }
}
