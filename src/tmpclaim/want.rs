//! The profile side of TMP1: a `[[macos.tmp]]` entry of a harness profile
//! (`Want`), its variables and the checked form that a claim takes
//! (`Resolved`). Pure: it reads no file.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// What a `[[macos.tmp]]` entry names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WantKind {
    /// A folder that paddock makes for this run.
    #[default]
    Dir,
    /// A file that paddock makes for this run.
    File,
    /// A Unix socket that the run binds.
    Socket,
    /// A folder of all runs, with entries of this run in it.
    Shared,
}

/// `[[macos.tmp]]` of a harness profile: a fixed name in `/private/tmp`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Want {
    /// The name in `/private/tmp`, one path part. Variables: `{uid}`,
    /// `{home}` (the real sandbox home), `{run}`, `{sha256:TEXT}`.
    pub name: String,
    #[serde(default)]
    pub kind: WantKind,
    /// `shared`: the names in the folder that are this run's own.
    #[serde(default)]
    pub entries: Vec<String>,
    /// Launch flags that the claim makes needless (`--no-daemon`). paddock
    /// drops them when every entry that names them is granted.
    #[serde(default)]
    pub replaces_args: Vec<String>,
    /// Pid files in the sandbox home, relative to it. The end of the run stops
    /// these processes when they run in this sandbox (a daemon that leaves
    /// the process group).
    #[serde(default)]
    pub stop_pid_files: Vec<String>,
    /// Why the harness needs it, for the notes.
    #[serde(default)]
    pub why: String,
}

/// The values of the template variables.
#[derive(Debug, Clone)]
pub struct Vars<'a> {
    pub uid: u32,
    pub home: &'a Path,
    pub run: &'a str,
}

/// Expand the variables of a template. A value is never read as a template.
pub fn expand(t: &str, v: &Vars) -> Result<String> {
    let (out, rest) = expand_until(t, v, false)?;
    if !rest.is_empty() {
        bail!("{t:?}: a '}}' without its '{{'");
    }
    Ok(out)
}

/// Expand up to an unmatched `}` (with `inner`) or the end. Returns the text
/// and the rest after the `}`.
fn expand_until<'t>(t: &'t str, v: &Vars, inner: bool) -> Result<(String, &'t str)> {
    let mut out = String::new();
    let mut rest = t;
    while let Some(c) = rest.chars().next() {
        match c {
            '}' if inner => return Ok((out, &rest[1..])),
            '}' => return Ok((out, rest)),
            '{' => {
                let body = &rest[1..];
                if let Some(text) = body.strip_prefix("sha256:") {
                    let (inside, after) = expand_until(text, v, true)?;
                    if after.len() == text.len() {
                        bail!("{t:?}: {{sha256: has no closing '}}'");
                    }
                    out.push_str(&crate::manifest::hex(&Sha256::digest(inside.as_bytes())));
                    rest = after;
                    continue;
                }
                let Some(end) = body.find('}') else {
                    bail!("{t:?}: a '{{' without its '}}'");
                };
                match &body[..end] {
                    "uid" => out.push_str(&v.uid.to_string()),
                    "home" => out.push_str(&v.home.display().to_string()),
                    "run" => out.push_str(v.run),
                    other => bail!(
                        "{t:?}: unknown variable {{{other}}} (known: uid, home, run, sha256:TEXT)"
                    ),
                }
                rest = &body[end + 1..];
            }
            _ => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if inner {
        // The caller sees that nothing was left to close.
        return Ok((out, t));
    }
    Ok((out, rest))
}

/// A name of one path part that a Seatbelt profile can quote.
pub fn check_name(n: &str) -> Result<()> {
    if n.is_empty() || n == "." || n == ".." || n.contains('/') {
        bail!("{n:?} is not one plain name in /private/tmp");
    }
    if n.chars().any(char::is_control) || n.len() > 255 {
        bail!("{n:?} has a control character or is longer than 255 bytes");
    }
    Ok(())
}

/// A want with its variables expanded and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub kind: WantKind,
    pub entries: Vec<String>,
    pub replaces_args: Vec<String>,
    /// Absolute pid files in the sandbox home.
    pub stop: Vec<PathBuf>,
    pub why: String,
    /// Every entry of a `shared` want is built from `{home}`: no program but
    /// a run of this sandbox home makes such a name. A leftover of an earlier
    /// run of the home can be removed at the next claim.
    pub home_bound: bool,
}

pub fn resolve(w: &Want, v: &Vars) -> Result<Resolved> {
    let name = expand(&w.name, v)?;
    check_name(&name)?;
    let mut entries = Vec::new();
    for e in &w.entries {
        let e = expand(e, v)?;
        check_name(&e)?;
        entries.push(e);
    }
    if (w.kind == WantKind::Shared) == entries.is_empty() {
        bail!("{name}: entries go with kind \"shared\", and kind \"shared\" needs entries");
    }
    let mut stop = Vec::new();
    for f in &w.stop_pid_files {
        let rel = Path::new(f);
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            bail!("{f:?}: a pid file is a plain path relative to the sandbox home");
        }
        stop.push(v.home.join(rel));
    }
    let home_bound = w.kind == WantKind::Shared && w.entries.iter().all(|e| e.contains("{home}"));
    Ok(Resolved {
        name,
        kind: w.kind,
        entries,
        replaces_args: w.replaces_args.clone(),
        stop,
        why: w.why.clone(),
        home_bound,
    })
}
