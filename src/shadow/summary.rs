//! What a shadow run changed: the baseline at the start and the summary at
//! the end.
//!
//! - An overlay run (Linux) reads its upper folder: everything there is a
//!   write of the run, but for paddock's own writes before the start.
//! - A clone run (macOS) has one folder. The baseline holds the size and
//!   the change time of each file after paddock's own writes, and the end
//!   compares the clone with it. A file that the run rewrote with the same
//!   size and change time is not seen.

use super::{Plan, SnapKind, MAX_DEPTH};
use crate::safefs::{Dir, Kind, Root, Stat};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How many changed paths the summary names.
const SUMMARY_PATHS: usize = 20;

/// Folders of the clone that paddock makes for the run itself, such as the
/// private temporary folder of a Seatbelt run (seatbelt/run.rs
/// `private_tmp`). Their files are not news.
const OWN_DIRS: &[&str] = &[".paddock-tmp"];

/// The files of the sandbox home before the harness starts.
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    /// The size and change time of each non-folder entry.
    pub files: BTreeMap<PathBuf, (u64, (i64, i64))>,
    /// The folders.
    pub dirs: BTreeSet<PathBuf>,
}

impl Baseline {
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl Plan {
    /// The files in the sandbox home now: the upper folder of an overlay,
    /// the whole clone of a clone run. Taken after paddock's own writes, so
    /// they stay out of the summary.
    pub fn baseline(&self) -> Baseline {
        let mut out = Baseline::default();
        if let Ok(root) = Root::open(&self.home) {
            if let Ok(d) = root.dir(Path::new(""), false) {
                let _ = scan_upper(
                    &d,
                    Path::new(""),
                    &mut |rel, st, _| {
                        if st.kind == Kind::Dir {
                            out.dirs.insert(rel.to_path_buf());
                        } else {
                            out.files.insert(rel.to_path_buf(), (st.size, st.mtime));
                        }
                    },
                    0,
                );
            }
        }
        out
    }

    /// The summary of the kind of the run: an overlay reads its upper
    /// folder, a clone compares itself with the baseline.
    pub fn end_summary(&self, base: &Baseline, skip: &[PathBuf]) -> Summary {
        match self.kind {
            SnapKind::Overlay => self.summary(base, skip),
            SnapKind::Clone => self.summary_clone(base),
        }
    }

    /// What the run changed in the upper folder, without paddock's own
    /// writes from before the start (`base`) and the mount points that
    /// bwrap made for the mounts at `skip` (relative to the home).
    pub fn summary(&self, base: &Baseline, skip: &[PathBuf]) -> Summary {
        let Some(lower) = &self.lower else {
            return Summary::default();
        };
        let mut all = Vec::new();
        self.scan_home(&mut |rel, st, whiteout| {
            if st.kind == Kind::Dir
                || base.files.get(rel) == Some(&(st.size, st.mtime))
                || (st.size == 0 && skip.iter().any(|m| m == rel))
            {
                return;
            }
            let what = if whiteout {
                Change::Deleted
            } else if lower.join(rel).symlink_metadata().is_ok() {
                Change::Changed
            } else {
                Change::Added
            };
            all.push((what, rel.to_path_buf()));
        });
        self.summarize(all)
    }

    /// What the run changed in the clone: a path that the baseline lacks is
    /// added, one with another size or change time is changed, and a
    /// baseline path that is gone is deleted. An empty folder of the
    /// baseline that is gone counts as deleted too.
    pub fn summary_clone(&self, base: &Baseline) -> Summary {
        let own = |rel: &Path| OWN_DIRS.iter().any(|d| rel.starts_with(d));
        let mut all = Vec::new();
        let mut seen_files = BTreeSet::new();
        let mut seen_dirs = BTreeSet::new();
        self.scan_home(&mut |rel, st, _| {
            if own(rel) {
                return;
            }
            if st.kind == Kind::Dir {
                seen_dirs.insert(rel.to_path_buf());
                return;
            }
            seen_files.insert(rel.to_path_buf());
            match base.files.get(rel) {
                None => all.push((Change::Added, rel.to_path_buf())),
                Some(b) if *b != (st.size, st.mtime) => {
                    all.push((Change::Changed, rel.to_path_buf()))
                }
                Some(_) => {}
            }
        });
        for rel in base.files.keys() {
            if !own(rel) && !seen_files.contains(rel) {
                all.push((Change::Deleted, rel.clone()));
            }
        }
        for rel in &base.dirs {
            let empty = !base.files.keys().any(|f| f.starts_with(rel))
                && !base.dirs.iter().any(|d| d != rel && d.starts_with(rel));
            if empty && !own(rel) && !seen_dirs.contains(rel) {
                all.push((Change::Deleted, rel.clone()));
            }
        }
        self.summarize(all)
    }

    /// Walk the sandbox home: `f(rel, stat, whiteout)` for each entry.
    fn scan_home(&self, f: &mut dyn FnMut(&Path, &Stat, bool)) {
        let Ok(root) = Root::open(&self.home) else {
            return;
        };
        let Ok(d) = root.dir(Path::new(""), false) else {
            return;
        };
        let whiteouts = self.kind == SnapKind::Overlay;
        let _ = scan_upper(
            &d,
            Path::new(""),
            &mut |rel, st, w| f(rel, st, w && whiteouts),
            0,
        );
    }

    fn summarize(&self, mut all: Vec<(Change, PathBuf)>) -> Summary {
        let mut s = Summary::default();
        all.sort_by(|a, b| a.1.cmp(&b.1));
        for (what, rel) in &all {
            match what {
                Change::Added => s.added += 1,
                Change::Changed => s.changed += 1,
                Change::Deleted => s.deleted += 1,
            }
            if self.root_of(rel).is_none() {
                s.outside += 1;
            }
        }
        s.paths = all.into_iter().take(SUMMARY_PATHS).collect();
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Changed,
    Deleted,
}

/// What a run changed in its private snapshot.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub added: usize,
    pub changed: usize,
    pub deleted: usize,
    /// Changes outside the data roots.
    pub outside: usize,
    /// The first changed paths, relative to the home.
    pub paths: Vec<(Change, PathBuf)>,
}

impl Summary {
    pub fn total(&self) -> usize {
        self.added + self.changed + self.deleted
    }

    /// The lines for stderr.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!(
            "profile default: the run added {}, changed {} and deleted {} file(s) in its private snapshot, {} of them outside the data roots. paddock removes the snapshot now, and nothing goes back to the host",
            self.added, self.changed, self.deleted, self.outside
        )];
        for (c, p) in &self.paths {
            let mark = match c {
                Change::Added => '+',
                Change::Changed => '~',
                Change::Deleted => '-',
            };
            v.push(format!("  {mark} ~/{}", p.display()));
        }
        if self.total() > self.paths.len() {
            v.push(format!("  and {} more", self.total() - self.paths.len()));
        }
        v
    }
}

/// Every entry below `dir`, folders before their contents:
/// `f(rel, stat, whiteout)`.
pub(super) fn scan_upper(
    dir: &Dir,
    rel: &Path,
    f: &mut dyn FnMut(&Path, &Stat, bool),
    depth: usize,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let mut stats = dir.stats()?;
    stats.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, st) in stats {
        let r = rel.join(&name);
        match st.kind {
            Kind::Dir => {
                f(&r, &st, false);
                if let Ok(sub) = dir.subdir(&name, false) {
                    scan_upper(&sub, &r, f, depth + 1)?;
                }
            }
            // In an overlay upper folder, a character device 0/0 is a
            // whiteout: the run deleted the file.
            Kind::Other => f(&r, &st, true),
            _ => f(&r, &st, false),
        }
    }
    Ok(())
}
