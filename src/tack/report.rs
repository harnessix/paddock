//! What tack shows: `paddock tack list`, the lines of `paddock explain`
//! and the block of `paddock doctor`. No line ever holds a value.

use super::config::{self, Backend, SecretCfg, Tack};
use super::import;
use super::lines::{self, Found, Syntax};
use super::program::{self, State};
use super::reference::Reference;
use super::store::{self, Store, StoreError};
use crate::env::HostEnv;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// How long doctor waits for the keyring.
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(10);

/// What every process sees in phase 1, said once in each report.
pub const PHASE_NOTE: &str = "this paddock does not give the values to programs yet: every process sees the reference, such as tack://NAME (README \"tack\")";

/// One secret for `paddock tack list`.
#[derive(Debug, Serialize)]
pub struct Row {
    pub name: String,
    pub reference: String,
    /// False: the import log names it, the configuration does not.
    pub configured: bool,
    pub store: String,
    pub programs: Vec<String>,
    pub callers: usize,
    pub hosts: Vec<String>,
    pub last_import: Option<u64>,
}

/// The names that the import log has, with the time of the last import.
fn imported(env: &HostEnv) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for e in super::events(env) {
        if e["event"] == "import" || e["event"] == "store" {
            if let (Some(n), Some(t)) = (e["name"].as_str(), e["time"].as_u64()) {
                out.insert(n.to_string(), t);
            }
        }
    }
    out
}

pub fn rows(env: &HostEnv, cfg: &Tack) -> Vec<Row> {
    let log = imported(env);
    let mut names: Vec<&String> = cfg.secrets.keys().chain(log.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| Reference::new(n).map(|r| (n, r)))
        .map(|(n, r)| {
            let s = cfg.secrets.get(n);
            let d = SecretCfg::default();
            let c = s.unwrap_or(&d);
            Row {
                name: n.clone(),
                reference: r.to_string(),
                configured: s.is_some(),
                store: c.source(),
                programs: c
                    .allow
                    .iter()
                    .map(|a| a.program.display().to_string())
                    .collect(),
                callers: c.callers.len(),
                hosts: c.hosts.clone(),
                last_import: log.get(n).copied(),
            }
        })
        .collect()
}

/// The lines of `paddock explain` for one sandbox environment. Empty when
/// tack has no secrets and the environment has no reference.
pub fn explain(env: &HostEnv, vars: &BTreeMap<String, crate::spec::EnvVar>) -> Vec<String> {
    let cfg = match config::load(env) {
        Ok(c) => c,
        Err(e) => return vec![format!("  error    {e:#}")],
    };
    let mut names: Vec<&String> = cfg.secrets.keys().collect();
    names.extend(
        vars.iter()
            .filter(|(_, v)| Reference::looks_like(&v.value))
            .map(|(k, _)| k),
    );
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for n in names {
        let r = Reference::new(n).map(|r| r.to_string()).unwrap_or_default();
        let line = match vars.get(n.as_str()) {
            Some(v) if v.value == r => {
                format!("  {n}  the sandbox sees {r}\n         {}", v.origin)
            }
            Some(v) if Reference::looks_like(&v.value) => format!(
                "  {n}  holds a reference to another name or an unknown form; programs see it as it is\n         {}",
                v.origin
            ),
            Some(v) => format!(
                "  {n}  enters the sandbox as a plain value: run paddock tack import {n} on the host\n         {}",
                v.origin
            ),
            None => format!("  {n}  not in the sandbox environment"),
        };
        out.push(line);
    }
    if !out.is_empty() {
        out.push(format!("  note     {PHASE_NOTE}"));
    }
    out
}

/// The `paddock doctor` block, with the host stores.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    let open = |_: &str, c: &SecretCfg| -> Result<Box<dyn Store>, StoreError> {
        match c.backend {
            Backend::Keyring => Ok(Box::new(store::Keyring::find(env)?.timeout(DOCTOR_TIMEOUT))),
            Backend::Command => store::open(env, c),
        }
    };
    doctor_with(env, &open)
}

pub fn doctor_with(env: &HostEnv, open: import::Opener) -> Vec<String> {
    let mut out = Vec::new();
    let cfg = match config::load(env) {
        Ok(c) => c,
        Err(e) => {
            out.push(format!("  error    {e:#}"));
            return out;
        }
    };
    let rows = rows(env, &cfg);
    if rows.is_empty() {
        out.push("  off      no secrets are set up (README \"tack\")".into());
        return out;
    }
    let files = import::default_files(env);
    for r in &rows {
        let d = SecretCfg::default();
        let c = cfg.secrets.get(&r.name).unwrap_or(&d);
        let n = &r.name;
        out.push(store_line(env, n, c, open));
        if !r.configured {
            out.push(format!(
                "  note     {n} has no [tack.secrets.{n}] table in {}: no program can receive it",
                super::show(env, &config::file(env))
            ));
        } else if c.allow.is_empty() {
            out.push(format!(
                "  note     {n} has no allowlist: no program can receive it"
            ));
        }
        for a in &c.allow {
            let p = a.program.display();
            match program::check(&a.program, a.sha256.as_deref()) {
                State::Ok { .. } => {}
                State::Missing => out.push(format!("  missing  {n}  allowlisted program {p} is not there: install it, or remove the entry")),
                State::NotExecutable => out.push(format!("  error    {n}  allowlisted program {p} is not an executable file")),
                State::HashChanged { now } => out.push(format!(
                    "  changed  {n}  {p} changed since its sha256 was written (now {now}): if the update is yours, put the new hash in the configuration"
                )),
                State::Unreadable(e) => out.push(format!("  error    {n}  cannot read {p} to check its hash: {e}")),
            }
            if a.signer.is_some() && !cfg!(target_os = "macos") {
                out.push(format!(
                    "  note     {n}  signer of {p} is only checked on macOS"
                ));
            }
        }
        for f in &files {
            let Ok(text) = std::fs::read_to_string(f) else {
                continue;
            };
            let text = crate::credentials::secret::Secret::from(text);
            for (i, line) in text.as_str().split('\n').enumerate() {
                let at = format!("{}:{}", super::show(env, f), i + 1);
                match lines::find(line, n, Syntax::of(f)) {
                    Some(Found::Value { .. }) => out.push(format!(
                        "  plain    {n}  has a plain value in {at}: paddock tack import {n}"
                    )),
                    Some(Found::Unsupported(why)) => out.push(format!(
                        "  plain?   {n}  is set in {at} in a form that paddock does not change ({why}): move its value yourself"
                    )),
                    _ => {}
                }
            }
        }
        match env.var(n) {
            Some(v) if v == r.reference => {}
            Some(v) if Reference::looks_like(v) => out.push(format!(
                "  error    {n}  in this environment is not a reference that this paddock knows"
            )),
            Some(_) => out.push(format!(
                "  plain    {n}  has a plain value in this environment: after the import, start a new session"
            )),
            None => {}
        }
    }
    let backups = count_files(&super::backup_dir(env));
    if backups > 0 {
        out.push(format!(
            "  note     {backups} backups in {} hold old files with plain values: delete them once the new files work",
            super::show(env, &super::backup_dir(env))
        ));
    }
    out.push(format!("  note     {PHASE_NOTE}"));
    out
}

fn store_line(env: &HostEnv, n: &str, c: &SecretCfg, open: import::Opener) -> String {
    if c.backend == Backend::Command {
        let prog = c.command.first().map(String::as_str).unwrap_or("");
        return match env.which(prog) {
            Some(_) => format!("  ok       {n}  from command {prog} (doctor does not run it)"),
            None => format!("  missing  {n}  the command {prog} is not installed or not on PATH"),
        };
    }
    let s = match open(n, c) {
        Ok(s) => s,
        Err(e) => return format!("  error    {n}  {e}"),
    };
    // The value is read and dropped at once: locked, then wiped.
    match s.get(n) {
        Ok(_) => format!("  ok       {n}  in the {}", s.describe()),
        Err(StoreError::Missing) => format!(
            "  missing  {n}  is not in the {}: paddock tack import {n}",
            s.describe()
        ),
        Err(e) => format!("  error    {n}  {e}"),
    }
}

fn count_files(dir: &std::path::Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => count_files(&e.path()),
            Ok(_) => 1,
            Err(_) => 0,
        })
        .sum()
}

/// The `paddock doctor` block, printed.
pub fn print_doctor(env: &HostEnv) {
    println!("\nSecrets for allowlisted programs (paddock tack):");
    for l in doctor_lines(env) {
        println!("{l}");
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
