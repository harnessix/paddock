//! `paddock tack import`: move the values of named variables out of plain
//! files into the store, and put references in their place.
//!
//! The order makes a crash at any moment safe:
//!
//! 1. Read every file and find the lines that set each variable.
//! 2. Write the value into the store, then read it back and compare. A
//!    file changes only after the store holds the same bytes.
//! 3. For each file: a private backup (0600) under the paddock state
//!    folder, then the new file through a temporary file and a rename, so
//!    that the file is the old one or the new one, never half of each.
//!
//! A crash after step 2 leaves the value in the store and in the file; a
//! new run finds the same value in the store and only rewrites the lines.
//! Without `--yes` nothing is written: the command prints the plan.
//! No output ever holds a value.

use super::config::{SecretCfg, Tack};
use super::lines::{self, Found, Syntax};
use super::reference::{valid_name, Reference};
use super::store::{Store, StoreError};
use crate::credentials::secret::Secret;
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::ops::Range;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The largest file that import reads.
const MAX_FILE: u64 = 4 << 20;

pub struct Options {
    pub names: Vec<String>,
    /// Files to read besides the shell startup files and environment.d.
    pub files: Vec<PathBuf>,
    /// Write. Without it, only print the plan.
    pub write: bool,
    /// Overwrite a different value that the store holds already.
    pub replace: bool,
}

/// The shell startup files and environment.d files of the user that exist.
pub fn default_files(env: &HostEnv) -> Vec<PathBuf> {
    let h = &env.home;
    let mut out: Vec<PathBuf> = [
        ".profile",
        ".bash_profile",
        ".bash_login",
        ".bashrc",
        ".bash_aliases",
    ]
    .iter()
    .map(|f| h.join(f))
    .collect();
    let zdot = env
        .var("ZDOTDIR")
        .filter(|d| Path::new(d).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(|| h.clone());
    for f in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        out.push(zdot.join(f));
    }
    out.push(env.config_dir.join("fish/config.fish"));
    for (dir, ext) in [("fish/conf.d", "fish"), ("environment.d", "conf")] {
        let mut more: Vec<PathBuf> = std::fs::read_dir(env.config_dir.join(dir))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == ext))
            .collect();
        more.sort();
        out.extend(more);
    }
    out.retain(|p| p.is_file());
    out.dedup();
    out
}

/// One line that sets a variable.
#[derive(Debug)]
pub struct Site {
    pub file: PathBuf,
    /// 1-based.
    pub line: usize,
    pub kind: SiteKind,
}

#[derive(Debug)]
pub enum SiteKind {
    /// The line holds a plain value, and becomes `new_line`.
    Rewrite {
        span: Range<usize>,
        new_line: String,
    },
    Reference,
    Unsupported(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Write the value into the store, then rewrite the lines.
    Store {
        replace: bool,
    },
    /// The store holds the same value: only rewrite the lines.
    Rewrite,
    /// Every line holds a reference already.
    Done,
    NotFound,
    Refused(String),
}

pub struct NamePlan {
    pub name: String,
    pub configured: bool,
    pub store_name: String,
    pub action: Action,
    pub sites: Vec<Site>,
    /// What else the user needs to know, without a value.
    pub notes: Vec<String>,
    value: Option<Secret>,
    store: Option<Box<dyn Store>>,
}

struct FileScan {
    /// As found or named.
    path: PathBuf,
    /// The file that is written: the target of a link.
    real: PathBuf,
    content: Secret,
    syntax: Syntax,
}

pub struct Plan {
    pub names: Vec<NamePlan>,
    files: Vec<FileScan>,
    /// Files that could not be read, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
}

/// The outcome of writing one file.
#[derive(Debug)]
pub struct Written {
    pub file: PathBuf,
    pub result: Result<PathBuf, String>,
}

pub type Opener<'a> = &'a dyn Fn(&str, &SecretCfg) -> Result<Box<dyn Store>, StoreError>;

/// Read the files and the store, and decide what to do for each name.
pub fn plan(env: &HostEnv, cfg: &Tack, opts: &Options, open: Opener) -> Result<Plan> {
    if opts.names.is_empty() {
        bail!("name at least one variable, such as: paddock tack import GH_TOKEN");
    }
    for n in &opts.names {
        if !valid_name(n) {
            bail!("'{n}' is not an environment variable name: use letters, digits and '_', not starting with a digit");
        }
    }
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut paths = default_files(env);
    for f in &opts.files {
        let p = std::path::absolute(f).with_context(|| format!("{}", f.display()))?;
        if !p.is_file() {
            bail!("{} is not a file", p.display());
        }
        paths.push(p);
    }
    let mut seen = Vec::new();
    for path in paths {
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.contains(&real) {
            continue;
        }
        seen.push(real.clone());
        match read(&real) {
            Ok(content) => files.push(FileScan {
                syntax: Syntax::of(&path),
                path,
                real,
                content,
            }),
            Err(e) => skipped.push((path, e)),
        }
    }
    let mut names = Vec::new();
    let mut done: Vec<&String> = Vec::new();
    for name in &opts.names {
        if done.contains(&name) {
            continue;
        }
        done.push(name);
        names.push(plan_name(env, cfg, opts, open, name, &files));
    }
    Ok(Plan {
        names,
        files,
        skipped,
    })
}

fn read(p: &Path) -> Result<Secret, String> {
    let m = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if m.len() > MAX_FILE {
        return Err(format!("it is larger than {} MiB", MAX_FILE >> 20));
    }
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    let s = Secret::new(bytes);
    if std::str::from_utf8(s.as_bytes()).is_err() {
        return Err("it is not UTF-8 text".into());
    }
    Ok(s)
}

fn plan_name(
    env: &HostEnv,
    cfg: &Tack,
    opts: &Options,
    open: Opener,
    name: &str,
    files: &[FileScan],
) -> NamePlan {
    let configured = cfg.secrets.get(name);
    let scfg = configured.cloned().unwrap_or_default();
    let reference = Reference::new(name).expect("checked").to_string();
    let mut sites = Vec::new();
    let mut values: Vec<Secret> = Vec::new();
    let mut origin: Vec<usize> = Vec::new();
    for f in files {
        for (i, line) in f.content.as_str().split('\n').enumerate() {
            let Some(found) = lines::find(line, name, f.syntax) else {
                continue;
            };
            let kind = match found {
                Found::Value { span, value } => {
                    if !values.iter().any(|v| v.as_bytes() == value.as_bytes()) {
                        values.push(value);
                    }
                    origin.push(sites.len());
                    SiteKind::Rewrite {
                        new_line: lines::rewrite(line, &span, &reference),
                        span,
                    }
                }
                Found::Reference => SiteKind::Reference,
                Found::Unsupported(why) => SiteKind::Unsupported(why),
                Found::Empty => continue,
            };
            sites.push(Site {
                file: f.path.clone(),
                line: i + 1,
                kind,
            });
        }
    }
    let mut p = NamePlan {
        name: name.to_string(),
        configured: configured.is_some(),
        store_name: scfg.source(),
        action: Action::NotFound,
        sites,
        notes: Vec::new(),
        value: None,
        store: None,
    };
    let at = |p: &NamePlan| -> String {
        p.sites
            .iter()
            .filter(|s| matches!(s.kind, SiteKind::Rewrite { .. }))
            .map(|s| format!("{}:{}", super::show(env, &s.file), s.line))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if p.sites
        .iter()
        .any(|s| matches!(s.kind, SiteKind::Unsupported(_)))
    {
        p.notes.push("some lines set the variable in a form that paddock does not change: move their value yourself, or make them plain (export NAME='value')".into());
    }
    if values.is_empty() {
        let refs = p
            .sites
            .iter()
            .any(|s| matches!(s.kind, SiteKind::Reference));
        p.action = if refs { Action::Done } else { Action::NotFound };
        if env.var(name).is_some_and(|v| !Reference::looks_like(v)) {
            p.notes.push(format!("{name} has a plain value in this environment, but no file that paddock read sets it. Name the file with --file, or check where your session sets it"));
        }
        return p;
    }
    if values.len() > 1 {
        p.action = Action::Refused(format!(
            "the files hold {} different values for {name} (at {}). Keep one value, or remove the old lines, then run the command again",
            values.len(),
            at(&p)
        ));
        return p;
    }
    let value = values.pop().expect("one");
    let store = match open(name, &scfg) {
        Ok(s) => s,
        Err(e) => {
            p.action = Action::Refused(e.to_string());
            return p;
        }
    };
    p.store_name = store.describe();
    p.action = match store.get(name) {
        Ok(v) if v.as_bytes() == value.as_bytes() => Action::Rewrite,
        Ok(_) if store.writable() && opts.replace => Action::Store { replace: true },
        Ok(_) if store.writable() => Action::Refused(format!(
            "the store holds another value for {name}. Run with --replace to overwrite it with the value of the files"
        )),
        Ok(_) => Action::Refused(format!(
            "{} prints another value than the files hold. Update the password manager first, so that it prints the value you use",
            store.describe()
        )),
        Err(StoreError::Missing) if store.writable() => Action::Store { replace: false },
        Err(StoreError::Missing) => Action::Refused(format!(
            "{} prints no value. Put the value into the password manager first",
            store.describe()
        )),
        Err(e) => Action::Refused(e.to_string()),
    };
    p.value = Some(value);
    p.store = Some(store);
    p
}

impl NamePlan {
    fn proceeds(&self) -> bool {
        matches!(self.action, Action::Store { .. } | Action::Rewrite)
    }
}

impl Plan {
    /// Whether every name has a good outcome.
    pub fn ok(&self) -> bool {
        self.names.iter().all(|n| {
            matches!(
                n.action,
                Action::Store { .. } | Action::Rewrite | Action::Done
            )
        })
    }

    /// The plan, for the user. No line holds a value.
    pub fn describe(&self, env: &HostEnv) -> Vec<String> {
        let mut out = Vec::new();
        for (f, why) in &self.skipped {
            out.push(format!("skipped  {}: {why}", super::show(env, f)));
        }
        for n in &self.names {
            let head = match &n.action {
                Action::Store { replace: false } => {
                    format!("store    {} into {}", n.name, n.store_name)
                }
                Action::Store { replace: true } => {
                    format!("replace  {} in {}", n.name, n.store_name)
                }
                Action::Rewrite => format!("ok       {} is in {} already", n.name, n.store_name),
                Action::Done => format!(
                    "done     {}: every line holds tack://{} already",
                    n.name, n.name
                ),
                Action::NotFound => {
                    format!("absent   {}: no file that paddock read sets it", n.name)
                }
                Action::Refused(why) => format!("refused  {}: {why}", n.name),
            };
            out.push(head);
            for s in &n.sites {
                let at = format!("{}:{}", super::show(env, &s.file), s.line);
                out.push(match &s.kind {
                    SiteKind::Rewrite { new_line, .. } if n.proceeds() => {
                        format!("           {at}  becomes  {}", new_line.trim())
                    }
                    SiteKind::Rewrite { .. } => {
                        format!("           {at}  plain value, not changed")
                    }
                    SiteKind::Reference => format!("           {at}  holds the reference"),
                    SiteKind::Unsupported(why) => format!("           {at}  not changed: {why}"),
                });
            }
            if n.proceeds() && !n.configured {
                out.push(format!("           {} has no [tack.secrets.{}] table yet: no program receives it until you add one with an allowlist", n.name, n.name));
            }
            for note in &n.notes {
                out.push(format!("           note: {note}"));
            }
        }
        out
    }

    /// Store the values, then rewrite the files. Returns the outcome of
    /// each changed file; the names whose store write failed are marked
    /// refused, and their lines stay.
    pub fn apply(&mut self, env: &HostEnv) -> Vec<Written> {
        for n in &mut self.names {
            if !matches!(n.action, Action::Store { .. }) {
                continue;
            }
            let (Some(store), Some(value)) = (&n.store, &n.value) else {
                continue;
            };
            let kept = store
                .put(&n.name, value)
                .and_then(|()| match store.get(&n.name) {
                    Ok(v) if v.as_bytes() == value.as_bytes() => Ok(()),
                    Ok(_) => Err(StoreError::Failed(
                        "the value read back differs from the one written".into(),
                    )),
                    Err(e) => Err(e),
                });
            match kept {
                Ok(()) => super::log_event(
                    env,
                    serde_json::json!({
                        "event": "store", "name": n.name, "store": n.store_name,
                    }),
                ),
                Err(e) => {
                    n.action = Action::Refused(format!(
                        "the store did not keep the value, so no file changed: {e}"
                    ));
                }
            }
        }
        let stamp = format!("{}-{}", crate::state::now(), std::process::id());
        let mut out = Vec::new();
        for f in &self.files {
            // line index -> (span, reference), from the names that go on.
            let mut edits: Edits = BTreeMap::new();
            for n in self.names.iter().filter(|n| n.proceeds()) {
                for s in n.sites.iter().filter(|s| s.file == f.path) {
                    if let SiteKind::Rewrite { span, .. } = &s.kind {
                        let r = Reference::new(&n.name).expect("checked").to_string();
                        edits
                            .entry(s.line - 1)
                            .or_default()
                            .push((span.clone(), r, &n.name));
                    }
                }
            }
            if edits.is_empty() {
                continue;
            }
            let result = write_file(env, f, &edits, &stamp);
            if result.is_ok() {
                for (line, e) in &edits {
                    for (_, _, name) in e {
                        super::log_event(
                            env,
                            serde_json::json!({
                                "event": "import", "name": name,
                                "file": f.path, "line": line + 1,
                            }),
                        );
                    }
                }
            }
            out.push(Written {
                file: f.path.clone(),
                result,
            });
        }
        out
    }
}

type Edits<'a> = BTreeMap<usize, Vec<(Range<usize>, String, &'a str)>>;

fn write_file(env: &HostEnv, f: &FileScan, edits: &Edits, stamp: &str) -> Result<PathBuf, String> {
    let now = read(&f.real)?;
    if now.as_bytes() != f.content.as_bytes() {
        return Err("the file changed after paddock read it; run the command again".into());
    }
    let mut new = Vec::with_capacity(now.len());
    for (i, line) in now.as_str().split('\n').enumerate() {
        if i > 0 {
            new.push(b'\n');
        }
        match edits.get(&i) {
            Some(e) => {
                // Spans of one line do not overlap: one variable each.
                let mut e: Vec<_> = e.iter().collect();
                e.sort_by_key(|(s, _, _)| std::cmp::Reverse(s.start));
                let mut l = line.to_string();
                for (span, r, _) in e {
                    l = lines::rewrite(&l, span, r);
                }
                new.extend_from_slice(l.as_bytes());
            }
            None => new.extend_from_slice(line.as_bytes()),
        }
    }
    let new = Secret::new(new);
    let flat = f
        .real
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "%");
    let backup = super::backup_dir(env).join(stamp).join(flat);
    crate::privfs::write(&backup, now.as_bytes())
        .and_then(|()| std::fs::File::open(&backup)?.sync_all())
        .map_err(|e| {
            format!(
                "cannot write the backup {}: {e}; the file did not change",
                backup.display()
            )
        })?;
    replace(&f.real, new.as_bytes()).map_err(|e| {
        format!(
            "cannot write the file: {e}; it did not change, and its backup is {}",
            backup.display()
        )
    })?;
    Ok(backup)
}

/// Replace a file through a temporary file in its folder and a rename. The
/// new file keeps the mode of the old one.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mode = std::fs::metadata(path)?.permissions().mode() & 0o7777;
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".paddock-tack-{}", std::process::id()));
    let tmp = dir.join(name);
    let _ = std::fs::remove_file(&tmp);
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        drop(f);
        if tests_fail_rename() {
            return Err(std::io::Error::other("injected failure"));
        }
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn tests_fail_rename() -> bool {
    #[cfg(test)]
    return FAIL_RENAME.with(|f| f.get());
    #[cfg(not(test))]
    false
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
