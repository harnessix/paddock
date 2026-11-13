//! What a run changed: the working copy against the original, with the
//! state at the start of the run as the base.
//!
//! Per path, with B the base, C the copy and O the original:
//!
//! - C = B and O = B: nothing.
//! - only C changed: a change that an apply writes.
//! - only O changed: the user worked in the original. The copy is behind,
//!   and an apply does not touch the path.
//! - both changed to the same content: nothing to do.
//! - both changed: a conflict. paddock never overwrites the original then.
//!
//! "Changed" is decided without a read where size, time and mode agree.
//! Otherwise the content decides, so a `touch` or a rewrite with the same
//! bytes is no change.
//!
//! After an apply, the base of a path is the applied content (run.rs).

use super::backup::Backup;
use super::run::{Run, Synced};
use super::tree::{self, Entry, Side};
use crate::snapshot::Skip;
use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs::File;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    New,
    Changed,
    Deleted,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::New => "new",
            Kind::Changed => "changed",
            Kind::Deleted => "deleted",
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Kind::New => '+',
            Kind::Changed => '~',
            Kind::Deleted => '-',
        }
    }
}

/// How both sides changed one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conflict {
    /// Both sides changed the file.
    Both,
    /// The copy deleted the file, and the original changed it.
    CopyDeleted,
    /// The original deleted the file, and the copy changed it.
    OriginalDeleted,
    /// Both sides made a new file with this name.
    BothNew,
}

impl Conflict {
    pub fn text(self) -> &'static str {
        match self {
            Conflict::Both => "both sides changed it",
            Conflict::CopyDeleted => "the copy deleted it, and your folder changed it",
            Conflict::OriginalDeleted => "your folder deleted it, and the copy changed it",
            Conflict::BothNew => "both sides made it",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// What an apply does to the original.
    pub kind: Kind,
    pub conflict: Option<Conflict>,
    /// The user keeps it in the copy: an apply of everything leaves it out.
    pub kept: bool,
    pub copy: Option<Entry>,
    pub original: Option<Entry>,
    /// The agent wrote the file a moment ago.
    pub writing: bool,
}

/// A path that only the original changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Behind {
    pub path: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    /// The working copy is there.
    pub tree: bool,
    pub changes: Vec<Change>,
    /// Applied paths that neither side changed since.
    pub applied: Vec<String>,
    pub behind: Vec<Behind>,
    /// Sockets, FIFOs, devices and names that are not UTF-8 in the copy.
    pub special: Vec<String>,
    /// A fingerprint of the list. An apply with this token stops when the
    /// list is another one by then.
    pub token: String,
}

impl Status {
    pub fn conflicts(&self) -> usize {
        self.changes.iter().filter(|c| c.conflict.is_some()).count()
    }

    pub fn count(&self, k: Kind) -> usize {
        self.changes.iter().filter(|c| c.kind == k).count()
    }

    /// Changes that an apply of everything writes.
    pub fn ready(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.conflict.is_none() && !c.kept)
            .count()
    }

    /// "4 changed, 2 new, 1 deleted", then the conflicts.
    pub fn counts_line(&self) -> String {
        let mut s = format!(
            "{} changed, {} new, {} deleted",
            self.count(Kind::Changed),
            self.count(Kind::New),
            self.count(Kind::Deleted)
        );
        match self.conflicts() {
            0 => {}
            1 => s.push_str(", 1 conflict"),
            n => s.push_str(&format!(", {n} conflicts")),
        }
        s
    }
}

/// Where the base content of one path is.
pub enum BaseRef<'a> {
    /// In the backup from the start of the run.
    Backup(&'a Backup),
    /// The content that an apply wrote, by hash. None: no file.
    Synced(Option<&'a str>),
}

/// The base of `path`: the entry on each side, and the content.
pub fn base_of<'a>(
    run: &'a Run,
    base: &'a Backup,
    manifest: &'a tree::Manifest,
    path: &str,
) -> (Option<&'a Entry>, Option<&'a Entry>, BaseRef<'a>) {
    match run.rec.synced.get(path) {
        Some(Synced {
            hash,
            copy,
            original,
        }) => (
            copy.as_ref(),
            original.as_ref(),
            BaseRef::Synced(hash.as_deref()),
        ),
        None => {
            let e = manifest.get(path);
            (e, e, BaseRef::Backup(base))
        }
    }
}

/// The hash that a sync record holds for a symlink.
pub fn link_hash(target: &str) -> String {
    format!("link:{target}")
}

/// The base content of `path`, when it is a file.
pub fn base_file(run: &Run, r: &BaseRef, path: &str) -> Option<File> {
    match r {
        BaseRef::Backup(b) => b.file(path),
        BaseRef::Synced(Some(h)) => run.stashed(h),
        BaseRef::Synced(None) => None,
    }
}

/// `now` on `side` differs from the base entry `was`.
fn differs(
    run: &Run,
    side: &Side,
    path: &str,
    now: Option<&Entry>,
    was: Option<&Entry>,
    r: &BaseRef,
) -> Result<bool> {
    let (a, b) = match (now, was) {
        (None, None) => return Ok(false),
        (Some(a), Some(b)) => (a, b),
        _ => return Ok(true),
    };
    if a.same_fp(b) {
        return Ok(false);
    }
    if a.link.is_some() || b.link.is_some() || a.size != b.size || a.mode != b.mode {
        return Ok(true);
    }
    let Some(mut f) = side.file(path) else {
        return Ok(true);
    };
    match r {
        BaseRef::Synced(Some(h)) => Ok(tree::hash(&mut f)? != *h),
        _ => match base_file(run, r, path) {
            Some(mut g) => Ok(!tree::same_bytes(&mut f, &mut g)?),
            None => Ok(true),
        },
    }
}

/// Both sides hold the same thing at `path`.
fn equal(
    copy: &Side,
    original: &Side,
    path: &str,
    c: Option<&Entry>,
    o: Option<&Entry>,
) -> Result<bool> {
    let (c, o) = match (c, o) {
        (None, None) => return Ok(true),
        (Some(c), Some(o)) => (c, o),
        _ => return Ok(false),
    };
    if c.link.is_some() || o.link.is_some() {
        return Ok(c.link == o.link);
    }
    if c.size != o.size || c.mode != o.mode {
        return Ok(false);
    }
    match (copy.file(path), original.file(path)) {
        (Some(mut a), Some(mut b)) => tree::same_bytes(&mut a, &mut b),
        _ => Ok(false),
    }
}

/// The rules of a comparison: those of the start, and what git ignores in
/// the folder now.
fn rules(run: &Run, dir: &std::path::Path) -> Skip {
    let mut skip = run.rec.skip.clone();
    if run.rec.git {
        skip = skip.union(&super::start::rules(dir));
    }
    skip.paths.insert(super::pointer::DIR.to_string());
    skip
}

/// What the run `run` changed, as of now.
pub fn status(run: &Run) -> Result<Status> {
    if !run.has_tree() {
        return Ok(Status::default());
    }
    let base = run.base()?;
    let manifest = base.manifest();
    let project = &run.rec.project;
    if !project.is_dir() {
        bail!(
            "the folder {} is not there. If you moved it, run `paddock pointer status` in its new place",
            project.display()
        );
    }
    let tree_dir = run.tree();
    let skip = rules(run, &tree_dir).union(&rules(run, project));
    let c = tree::scan(&tree_dir, &skip).context("cannot read the copy")?;
    let o =
        tree::scan(project, &skip).with_context(|| format!("cannot read {}", project.display()))?;
    let copy = Side::open(&tree_dir)?;
    let original = Side::open(project)?;
    let live = run.rec.ended.is_none();
    let now = crate::state::now();
    let paths: BTreeSet<&String> = manifest
        .keys()
        .chain(c.files.keys())
        .chain(o.files.keys())
        .chain(run.rec.synced.keys())
        .collect();
    let mut st = Status {
        tree: true,
        special: c.special,
        ..Default::default()
    };
    for p in paths {
        if skip.covers(p) {
            continue;
        }
        let (bc, bo, r) = base_of(run, &base, &manifest, p);
        let (ce, oe) = (c.files.get(p), o.files.get(p));
        let cc = differs(run, &copy, p, ce, bc, &r)?;
        let oc = differs(run, &original, p, oe, bo, &r)?;
        let was = bo.is_some();
        match (cc, oc) {
            (false, false) => {
                if run.rec.synced.contains_key(p) {
                    st.applied.push(p.clone());
                }
                continue;
            }
            (false, true) => {
                st.behind.push(Behind {
                    path: p.clone(),
                    kind: match (was, oe.is_some()) {
                        (false, _) => Kind::New,
                        (true, false) => Kind::Deleted,
                        (true, true) => Kind::Changed,
                    },
                });
                continue;
            }
            _ => {}
        }
        let (kind, conflict) = if !oc {
            let k = match (oe.is_some(), ce.is_some()) {
                (false, _) => Kind::New,
                (true, false) => Kind::Deleted,
                (true, true) => Kind::Changed,
            };
            (k, None)
        } else if equal(&copy, &original, p, ce, oe)? {
            continue;
        } else {
            match (ce.is_some(), oe.is_some(), was) {
                (false, _, _) => (Kind::Deleted, Some(Conflict::CopyDeleted)),
                (true, false, _) => (Kind::New, Some(Conflict::OriginalDeleted)),
                (true, true, false) => (Kind::Changed, Some(Conflict::BothNew)),
                (true, true, true) => (Kind::Changed, Some(Conflict::Both)),
            }
        };
        st.changes.push(Change {
            path: p.clone(),
            kind,
            conflict,
            kept: run.rec.kept.contains(p),
            writing: live && ce.is_some_and(|e| e.link.is_none() && e.secs() + 2 > now),
            copy: ce.cloned(),
            original: oe.cloned(),
        });
    }
    st.token = token(&st.changes);
    Ok(st)
}

fn token(changes: &[Change]) -> String {
    let mut h = blake3::Hasher::new();
    let fp = |e: &Option<Entry>| match e {
        None => "-".to_string(),
        Some(e) => format!("{},{},{},{:?}", e.size, e.mtime, e.mode, e.link),
    };
    for c in changes {
        h.update(
            format!(
                "{}\0{}\0{:?}\0{}\0{}\n",
                c.path,
                c.kind.word(),
                c.conflict,
                fp(&c.copy),
                fp(&c.original)
            )
            .as_bytes(),
        );
    }
    h.finalize().to_hex()[..16].to_string()
}

/// A path as the user typed it: without `./` at the start and `/` at the
/// end. A path that leaves the folder is refused.
pub fn clean_path(p: &str) -> Result<String> {
    let mut s = p.trim();
    while let Some(r) = s.strip_prefix("./") {
        s = r;
    }
    let s = s.trim_end_matches('/');
    if s.is_empty() || s == "." {
        return Ok(String::new());
    }
    if s.starts_with('/') || s.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
        bail!("{p} is not a path inside the project");
    }
    Ok(s.to_string())
}

/// `path` is `sel`, or below the folder `sel`. An empty `sel` is the
/// whole project.
pub fn covers(sel: &str, path: &str) -> bool {
    sel.is_empty()
        || path == sel
        || path
            .strip_prefix(sel)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The changes that `paths` name: a file, or a folder with all below it.
/// Each path must name at least one change.
pub fn select<'a>(st: &'a Status, paths: &[String]) -> Result<Vec<&'a Change>> {
    let mut sels = Vec::new();
    for p in paths {
        let s = clean_path(p)?;
        if !st.changes.iter().any(|c| covers(&s, &c.path)) {
            bail!("{p} is no change of this run. `paddock changes` lists the changes");
        }
        sels.push(s);
    }
    Ok(st
        .changes
        .iter()
        .filter(|c| sels.iter().any(|s| covers(s, &c.path)))
        .collect())
}
