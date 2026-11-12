//! `paddock horse import`: read chosen host sessions into the store.
//!
//! Each session is copied into a private staging folder first, and the
//! codec reads the copy (host.rs). The host file is opened read-only and
//! only for that copy. A session that is in the store already gets the new
//! rows of the host file; the rows that the store has stay, also when the
//! harness rewrote the file.

use super::{Found, HostOrigin, Staging};
use crate::horse::Store;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Which host sessions to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Select {
    /// Every session under the transcript folder of the harness.
    All,
    /// Every session under these folders or files, relative to the home.
    Under(Vec<PathBuf>),
    /// The sessions with these native ids.
    Ids(Vec<String>),
}

/// One session that came in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub session: String,
    pub horse: String,
    pub native_id: String,
    pub rel: PathBuf,
    /// The session was not in the store before.
    pub new: bool,
    /// The number of rows of the session in the store now.
    pub rows: usize,
}

#[derive(Debug, Default)]
pub struct Report {
    pub imported: Vec<Imported>,
    /// The sessions that did not come in, and why.
    pub failed: Vec<(PathBuf, String)>,
    /// Native ids that were asked for and not found.
    pub missing: Vec<String>,
}

/// The sessions that `select` chooses under the home.
pub fn choose(home: &Path, harness: &str, select: &Select) -> Result<(Vec<Found>, Vec<String>)> {
    let mut missing = Vec::new();
    let found = match select {
        Select::All => super::list(home, harness, None)?,
        Select::Under(paths) => {
            let mut out: Vec<Found> = Vec::new();
            for p in paths {
                for f in under(home, harness, p)? {
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
            out
        }
        Select::Ids(ids) => {
            let all = super::list(home, harness, None)?;
            let mut out = Vec::new();
            for id in ids {
                let hits: Vec<&Found> = all.iter().filter(|f| names(harness, f, id)).collect();
                if hits.is_empty() {
                    missing.push(id.clone());
                }
                out.extend(hits.into_iter().map(|f| Found {
                    want: Some(id.clone()),
                    ..f.clone()
                }));
            }
            out
        }
    };
    Ok((found, missing))
}

/// The sessions at or below `p`: a session file or folder, or a folder
/// with sessions. `p` is relative to the home, or absolute inside it.
fn under(home: &Path, harness: &str, p: &Path) -> Result<Vec<Found>> {
    // An absolute path and the home can name the same folder by another
    // path (a symlink): compare the resolved paths, as adopt does.
    let rel = if p.is_absolute() {
        let real = super::canon_home(home);
        match super::canon_home(p).strip_prefix(&real) {
            Ok(r) => r.to_path_buf(),
            Err(_) => bail!("{} is not inside the home {}", p.display(), real.display()),
        }
    } else {
        p.to_path_buf()
    };
    super::check_rel(&rel)?;
    let root = Path::new(super::root(harness)?);
    if !rel.starts_with(root) {
        bail!(
            "{} is not inside the {harness} transcript folder ~/{}",
            rel.display(),
            root.display()
        );
    }
    // Only the folder of `p` is listed, not the whole transcript folder.
    let start = match std::fs::symlink_metadata(home.join(&rel)) {
        Ok(m) if m.is_dir() => rel.clone(),
        _ => rel
            .parent()
            .filter(|d| d.starts_with(root))
            .unwrap_or(root)
            .to_path_buf(),
    };
    let all = super::list(home, harness, Some(&start))?;
    Ok(all
        .into_iter()
        .filter(|f| f.rel.starts_with(&rel) || f.files.iter().any(|x| x == &rel))
        .collect())
}

/// Whether the name of a session (its file, or its Grok folder) names the
/// native id `id`: the file stem, the end of a Codex rollout name, or the
/// session folder (Grok, dsh). A folder above the session never matches, so a project
/// folder of Claude Code is no id. The read checks the real id of the
/// copy after ([`Found::want`]).
fn names(harness: &str, f: &Found, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let Some(name) = f.rel.file_name().map(|n| n.to_string_lossy()) else {
        return false;
    };
    match harness {
        // `.dsh/sessions/ID/session.v4.jsonl`: the folder of the file.
        super::dsh::HARNESS => f
            .rel
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n.to_string_lossy() == id),
        super::grok::HARNESS => name == id,
        super::agy::HARNESS => name.strip_suffix(".db") == Some(id),
        super::codex::HARNESS => name
            .strip_suffix(".jsonl")
            .is_some_and(|s| s.ends_with(&format!("-{id}"))),
        _ => name.strip_suffix(".jsonl") == Some(id),
    }
}

/// Read the chosen sessions. `horse`: the horse of the new host sessions;
/// None: one new horse for all of them. `via`: `import` or `adopt`.
pub fn import(
    store: &Store,
    staging: &Path,
    harness: &str,
    home: &Path,
    found: &[Found],
    horse: Option<&str>,
    via: &str,
) -> Result<Report> {
    if !super::HARNESSES.contains(&harness) {
        bail!(
            "paddock cannot read host sessions of {harness}; it reads {}",
            super::HARNESSES.join(", ")
        );
    }
    if let Some(h) = horse {
        store.horse(h)?;
    }
    let mut report = Report::default();
    let mut target: Option<String> = horse.map(str::to_string);
    for f in found {
        match one(store, staging, harness, home, f, &mut target, via) {
            Ok(i) => report.imported.push(i),
            Err(e) => report.failed.push((f.rel.clone(), format!("{e:#}"))),
        }
    }
    Ok(report)
}

fn one(
    store: &Store,
    staging: &Path,
    harness: &str,
    home: &Path,
    f: &Found,
    target: &mut Option<String>,
    via: &str,
) -> Result<Imported> {
    let stage = Staging::new(staging)?;
    let copy = stage.copy(home, f)?;
    let native = super::native_id(harness, &copy)?;
    if let Some(want) = &f.want {
        anyhow::ensure!(
            *want == native,
            "{} holds the session {native}, not {want}",
            f.rel.display()
        );
    }
    let version = super::reader_version(harness, &copy)?;
    let home = super::canon_home(home);
    let home = home.as_path();
    let locked = store.lock()?;
    let known = store.host_session_at(harness, &native, home)?;
    let horse = match (&known, target.as_ref()) {
        (Some(s), _) => s.horse.clone(),
        (None, Some(h)) => h.clone(),
        (None, None) => {
            let h = crate::horse::dna::new_default_horse_locked(&locked)?;
            *target = Some(h.id.clone());
            h.id
        }
    };
    let origin = HostOrigin {
        via: known
            .as_ref()
            .and_then(|s| s.host.as_ref())
            .map_or_else(|| via.to_string(), |h| h.via.clone()),
        home: home.to_path_buf(),
        path: f.rel.clone(),
        read_ms: crate::horse::now_ms(),
    };
    let id = super::ingest_locked(&locked, harness, &copy, &version, &horse, &origin)?;
    let s = store.session(&id)?;
    let rows = store.session_nodes(&s)?.len();
    if known.is_none() {
        let body = serde_json::json!({"type": "host.import", "session": id, "via": via,
            "native_id": native, "path": f.rel, "at_ms": crate::horse::now_ms()});
        crate::horse::run::append_horse_event_locked(&locked, &s.horse, Some(harness), &body)?;
    }
    Ok(Imported {
        session: id,
        horse: s.horse,
        native_id: native,
        rel: f.rel.clone(),
        new: known.is_none(),
        rows,
    })
}
