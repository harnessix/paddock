//! The host hooks of an adoption: live host sessions.
//!
//! `paddock horse adopt` adds paddock's hook entries to the host
//! configuration of the harness, so each turn of a host session reaches
//! the store at its end (hookcmd.rs). The rules:
//!
//! - Before the first change, paddock copies the file into its state
//!   folder (the backup). A file that does not exist gets no backup.
//! - Each entry that paddock adds runs `paddock horse hook ... --mark
//!   paddock-horse`. The marker and the exact command identify the entry.
//! - paddock appends its entries after the user's entries of the same
//!   event, so the position of each user entry stays the same (Codex keys
//!   its trust of an entry by its position).
//! - paddock refuses a file that is a symlink, that is not a JSON object,
//!   or whose hooks are not in the form that paddock knows. Nothing changes
//!   then.
//! - `--off` takes out exactly the entries that paddock added, and the
//!   lists and objects that paddock made for them when they are empty. A
//!   file that paddock made goes when only paddock's entries were in it.
//!
//! paddock never writes the file of the user again in its own layout.
//! Each change is an insertion of text at one place (jsontext.rs), in the
//! layout around it, and each removal cuts that text out again. The order
//! of the keys, the spaces and the line breaks of the user stay. Before it
//! writes, paddock checks that the new text without its insertions is the
//! old text, byte for byte, and that every value of the user stays.

use super::hookfile::{parse, read, write, File};
use super::jsontext;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The marker argument of each command that paddock adds.
pub const MARK: &str = "paddock-horse";

/// The name of paddock's entry in a file of the named style (agy).
const NAMED: &str = "paddock-horse";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// `{"hooks": {EVENT: [{"matcher"?, "hooks": [handler]}]}}`.
    Groups,
    /// `{NAME: {EVENT: [handler]}}`.
    Named,
}

/// Where the hooks of a harness are on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The file, relative to the home.
    pub file: &'static str,
    pub style: Style,
    /// The events that get paddock's entry.
    pub events: &'static [&'static str],
}

/// The hook file and events of each harness that has hooks.
pub fn spec(harness: &str) -> Option<Spec> {
    Some(match harness {
        "claude" => Spec {
            file: ".claude/settings.json",
            style: Style::Groups,
            events: &["SessionStart", "UserPromptSubmit", "Stop"],
        },
        "codex" => Spec {
            file: ".codex/hooks.json",
            style: Style::Groups,
            events: &["SessionStart", "Stop"],
        },
        // A file of paddock's own: Grok Build reads every file there.
        "grok" => Spec {
            file: ".grok/hooks/paddock-horse.json",
            style: Style::Groups,
            events: &["Stop"],
        },
        "agy" => Spec {
            file: ".gemini/config/hooks.json",
            style: Style::Named,
            events: &["Stop"],
        },
        _ => return None,
    })
}

/// One entry that paddock added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Added {
    pub event: String,
    pub command: String,
}

/// What adoption changed in a hook file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The hook file, relative to the home.
    pub file: PathBuf,
    pub style: Style,
    /// paddock made the file: it was not there.
    pub created: bool,
    /// The copy of the file before the first change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// The sha256 of the file before the first change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    pub entries: Vec<Added>,
    /// The texts that paddock inserted, in their order: `--off` cuts each
    /// one that is still in the file as it was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserts: Vec<String>,
    /// The members that paddock made for its entries: `hooks`, or
    /// `hooks.EVENT`. `--off` takes out each one that is empty then. None:
    /// a record from before paddock kept them; every empty event list of
    /// paddock's events goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made: Option<Vec<String>>,
    /// The paddock program of the entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
}

impl Installed {
    /// The program of the entries is gone (an upgrade moved it): each turn
    /// of the harness then shows a hook error. `paddock horse adopt
    /// HARNESS` puts the entries in again with the program of now.
    pub fn program_missing(&self) -> bool {
        self.program
            .as_ref()
            .is_some_and(|p| std::fs::symlink_metadata(p).is_err())
    }
}

/// The paddock program for the hook entries: a name on `PATH` that leads
/// to the running program (`/usr/bin/paddock`, `~/.cargo/bin/paddock`, a
/// Homebrew link), so an upgrade that moves the versioned file keeps the
/// entries working. Else the running program itself.
pub fn stable_program(exe: &Path, path: &[PathBuf]) -> PathBuf {
    let Ok(real) = std::fs::canonicalize(exe) else {
        return exe.to_path_buf();
    };
    let name = exe.file_name().unwrap_or(std::ffi::OsStr::new("paddock"));
    for dir in path.iter().filter(|d| d.is_absolute()) {
        let c = dir.join(name);
        if std::fs::canonicalize(&c).is_ok_and(|r| r == real) {
            return c;
        }
    }
    exe.to_path_buf()
}

/// The command of paddock's entry for `event` of `harness`.
pub fn command(exe: &Path, harness: &str, event: &str) -> String {
    format!(
        "{} horse hook {harness} {event} --mark {MARK}",
        shell_quote(&exe.display().to_string())
    )
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn handler(command: &str) -> Value {
    json!({"type": "command", "command": command, "timeout": 30})
}

fn pretty(v: &Value) -> Result<Vec<u8>> {
    Ok((serde_json::to_string_pretty(v)? + "\n").into_bytes())
}

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn is_ours(entries: &[Added], h: &Value, event: &str) -> bool {
    entries
        .iter()
        .any(|a| a.event == event && h.get("command").and_then(Value::as_str) == Some(&a.command))
}

/// Whether paddock made the member `key` (`hooks` or `hooks.EVENT`) and
/// may take it out when it is empty.
fn made_by_paddock(made: Option<&[String]>, entries: &[Added], key: &str) -> bool {
    match made {
        Some(m) => m.iter().any(|x| x == key),
        None => key == "hooks" || entries.iter().any(|a| format!("hooks.{}", a.event) == key),
    }
}

/// The document without the handlers of `entries`, and without the
/// members that paddock made for them when they are empty.
fn strip(doc: &Value, style: Style, entries: &[Added], made: Option<&[String]>) -> Value {
    let mut doc = doc.clone();
    match style {
        Style::Groups => {
            let Some(section) = doc.get_mut("hooks").and_then(Value::as_object_mut) else {
                return doc;
            };
            for a in entries {
                let Some(groups) = section.get_mut(&a.event).and_then(Value::as_array_mut) else {
                    continue;
                };
                // A group that paddock added holds only `hooks`, with only
                // its handlers.
                groups.retain(|g| {
                    let hs = g.get("hooks").and_then(Value::as_array);
                    !(g.as_object().is_some_and(|o| o.len() == 1)
                        && hs.is_some_and(|hs| {
                            !hs.is_empty() && hs.iter().all(|h| is_ours(entries, h, &a.event))
                        }))
                });
                for g in groups.iter_mut() {
                    if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                        hs.retain(|h| !is_ours(entries, h, &a.event));
                    }
                }
                if groups.is_empty()
                    && made_by_paddock(made, entries, &format!("hooks.{}", a.event))
                {
                    section.remove(&a.event);
                }
            }
            if section.is_empty() && made_by_paddock(made, entries, "hooks") {
                doc.as_object_mut().map(|o| o.remove("hooks"));
            }
        }
        Style::Named => {
            let Some(o) = doc.as_object_mut() else {
                return doc;
            };
            if let Some(mine) = o.get_mut(NAMED).and_then(Value::as_object_mut) {
                for a in entries {
                    if let Some(hs) = mine.get_mut(&a.event).and_then(Value::as_array_mut) {
                        hs.retain(|h| !is_ours(entries, h, &a.event));
                        if hs.is_empty() {
                            mine.remove(&a.event);
                        }
                    }
                }
                if mine.is_empty() {
                    o.remove(NAMED);
                }
            }
        }
    }
    doc
}

/// Whether a hook command is an entry of a host adoption (the marker).
pub fn is_entry(command: &str) -> bool {
    command.contains(" horse hook ") && command.trim_end().ends_with(&format!("--mark {MARK}"))
}

/// Whether `doc` has a handler that paddock wrote (the marker).
fn has_ours(v: &Value) -> bool {
    match v {
        Value::String(s) => is_entry(s),
        Value::Array(a) => a.iter().any(has_ours),
        Value::Object(o) => o.values().any(has_ours),
        _ => false,
    }
}

/// The handlers of `event` in `doc`, in the layout of `style`.
fn handlers<'a>(doc: &'a Value, style: Style, event: &str) -> Vec<&'a Value> {
    match style {
        Style::Groups => doc
            .get("hooks")
            .and_then(|s| s.get(event))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|g| g.get("hooks").and_then(Value::as_array))
            .flatten()
            .collect(),
        Style::Named => doc
            .get(NAMED)
            .and_then(|m| m.get(event))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect(),
    }
}

/// The recorded entries of `i` that the hook file under `home` does not
/// have now: a crash between the save of the record and the write of the
/// file, or an edit of the user, took them out. The file is read, not the
/// record.
pub fn missing(home: &Path, i: &Installed) -> Result<Vec<Added>> {
    let f = read(home, &i.file)?;
    let doc = parse(&f, &i.file)?;
    Ok(i.entries
        .iter()
        .filter(|a| {
            !handlers(&doc, i.style, &a.event)
                .into_iter()
                .any(|h| h.get("command").and_then(Value::as_str) == Some(&a.command))
        })
        .cloned()
        .collect())
}

/// The group of paddock's handler, and its own `paddock-horse` entry.
fn group(command: &str) -> Value {
    json!({"hooks": [handler(command)]})
}

/// The insertions that add paddock's entries to `text` (a JSON object), in
/// their order, with the members that paddock makes for them. Each one
/// applies to the text that the ones before it made.
fn insertions(
    text: &str,
    style: Style,
    entries: &[Added],
    rel: &Path,
) -> Result<(Vec<jsontext::Insert>, Vec<String>, String)> {
    let refuse = |what: &str| {
        format!(
            "{what} of ~/{}; paddock does not change them",
            rel.display()
        )
    };
    let mut cur = text.to_string();
    let mut out = Vec::new();
    let mut made = Vec::new();
    let mut push = |cur: &mut String, i: jsontext::Insert| {
        *cur = i.apply(cur);
        out.push(i);
    };
    let root = jsontext::parse(&cur)?;
    match style {
        Style::Groups => match root.get("hooks") {
            None => {
                let mut section = Map::new();
                for a in entries {
                    section.insert(a.event.clone(), json!([group(&a.command)]));
                    made.push(format!("hooks.{}", a.event));
                }
                made.insert(0, "hooks".into());
                let i = jsontext::append_member(&cur, &root, "hooks", &Value::Object(section))?;
                push(&mut cur, i);
            }
            Some(section) if section.members().is_none() => {
                bail!(refuse("the hooks are not a JSON object"))
            }
            Some(_) => {
                for a in entries {
                    let root = jsontext::parse(&cur)?;
                    let section = root.get("hooks").context("the hooks went")?;
                    let i = match section.get(&a.event) {
                        None => {
                            made.push(format!("hooks.{}", a.event));
                            jsontext::append_member(
                                &cur,
                                section,
                                &a.event,
                                &json!([group(&a.command)]),
                            )?
                        }
                        Some(list) if list.items().is_some() => {
                            jsontext::append_item(&cur, list, &group(&a.command))?
                        }
                        Some(_) => bail!(refuse(&format!("the {} hooks are not a list", a.event))),
                    };
                    push(&mut cur, i);
                }
            }
        },
        Style::Named => {
            if root.get(NAMED).is_some() {
                bail!(
                    "~/{} has an entry {NAMED} already; paddock does not change it",
                    rel.display()
                );
            }
            let mut mine = Map::new();
            for a in entries {
                mine.insert(a.event.clone(), json!([handler(&a.command)]));
            }
            let i = jsontext::append_member(&cur, &root, NAMED, &Value::Object(mine))?;
            push(&mut cur, i);
        }
    }
    Ok((out, made, cur))
}

/// The change of a hook file that adoption will make: every check done,
/// nothing written yet. [`Plan::apply`] writes it.
pub struct Plan {
    file: File,
    new: Vec<u8>,
    old: Option<Vec<u8>>,
    installed: Installed,
}

impl Plan {
    /// What the hook file will have, for the adoption state: the state is
    /// saved before the file changes, so a crash between the two leaves a
    /// record that `--off` can use.
    pub fn installed(&self) -> &Installed {
        &self.installed
    }

    /// Write the backup, then the hook file.
    pub fn apply(self) -> Result<Installed> {
        if let (Some(b), Some(p)) = (&self.old, &self.installed.backup) {
            crate::privfs::dir(p.parent().context("a backup has a folder")?)?;
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(p)?;
            out.write_all(b)?;
            out.sync_all()?;
        }
        write(&self.file, &self.new)?;
        Ok(self.installed)
    }
}

/// Plan the addition of paddock's entries to the hook file of `harness`
/// under `home`. `backups`: the folder for the backup. None: the harness
/// has no hooks.
pub fn plan(home: &Path, backups: &Path, harness: &str, exe: &Path) -> Result<Option<Plan>> {
    let Some(sp) = spec(harness) else {
        return Ok(None);
    };
    let rel = PathBuf::from(sp.file);
    let f = read(home, &rel)?;
    let doc = parse(&f, &rel)?;
    if has_ours(&doc) {
        bail!(
            "~/{} has paddock's hook entries already, from an adoption that paddock does not know; take them out by hand first",
            rel.display()
        );
    }
    let entries: Vec<Added> = sp
        .events
        .iter()
        .map(|e| Added {
            event: (*e).into(),
            command: command(exe, harness, e),
        })
        .collect();
    let old_text = match &f.bytes {
        Some(b) => Some(
            std::str::from_utf8(b)
                .with_context(|| {
                    format!(
                        "~/{} is not UTF-8; paddock does not change it",
                        rel.display()
                    )
                })?
                .to_string(),
        ),
        None => None,
    };
    let (new, inserts, made) = match old_text.as_deref() {
        // No file: paddock writes it, in its own layout.
        None => {
            let (_, made, body) = insertions("{}", sp.style, &entries, &rel)?;
            let v: Value = serde_json::from_str(&body)?;
            (pretty(&v)?, Vec::new(), made)
        }
        // Only spaces: paddock's object goes before them.
        Some(t) if t.trim().is_empty() => {
            let (_, made, body) = insertions("{}", sp.style, &entries, &rel)?;
            let v: Value = serde_json::from_str(&body)?;
            let ins = String::from_utf8(pretty(&v)?)?;
            (format!("{ins}{t}").into_bytes(), vec![ins], made)
        }
        Some(t) => {
            let (ins, made, new) = insertions(t, sp.style, &entries, &rel)?;
            // The new text without the insertions is the old text.
            let mut back = new.clone();
            for i in ins.iter().rev() {
                back.replace_range(i.at..i.at + i.text.len(), "");
            }
            anyhow::ensure!(
                back == t,
                "paddock would change other bytes of ~/{}; nothing was written",
                rel.display()
            );
            (
                new.into_bytes(),
                ins.into_iter().map(|i| i.text).collect(),
                made,
            )
        }
    };
    // Every value of the user stays.
    let new_doc: Value = serde_json::from_slice(&new).with_context(|| {
        format!(
            "paddock would make ~/{} invalid; nothing was written",
            rel.display()
        )
    })?;
    anyhow::ensure!(
        strip(&new_doc, sp.style, &entries, Some(&made)) == doc,
        "paddock would change other entries of ~/{}; nothing was written",
        rel.display()
    );
    let backup = match &f.bytes {
        Some(_) => Some(backups.join(format!(
            "{harness}-{}-{}.json",
            crate::horse::now_ms(),
            crate::horse::hex(&crate::horse::random_bytes::<3>()?)
        ))),
        None => None,
    };
    let installed = Installed {
        file: rel,
        style: sp.style,
        created: f.bytes.is_none(),
        backup,
        before: f.bytes.as_deref().map(sha),
        entries,
        inserts,
        made: Some(made),
        program: Some(exe.to_path_buf()),
    };
    Ok(Some(Plan {
        old: f.bytes.clone(),
        file: f,
        new,
        installed,
    }))
}

/// Add paddock's entries to the hook file of `harness` under `home` now.
pub fn install(
    home: &Path,
    backups: &Path,
    harness: &str,
    exe: &Path,
) -> Result<Option<Installed>> {
    plan(home, backups, harness, exe)?
        .map(Plan::apply)
        .transpose()
}

/// What `remove` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Removed {
    /// The file is the backup again, byte for byte.
    Restored,
    /// paddock's entries went; the other entries stay.
    Stripped,
    /// paddock made the file and only its entries were in it: it went.
    Deleted,
    /// The file has none of paddock's entries (or is not there).
    Nothing,
}

/// The first of paddock's handlers in `text`: the text without it.
fn cut_one(text: &str, i: &Installed) -> Result<Option<String>> {
    let root = jsontext::parse(text)?;
    let ours = |n: &jsontext::Node, event: &str| -> bool {
        let Ok(v) = serde_json::from_str::<Value>(&text[n.start..n.end]) else {
            return false;
        };
        is_ours(&i.entries, &v, event)
    };
    match i.style {
        Style::Groups => {
            let Some(section) = root.get("hooks") else {
                return Ok(None);
            };
            for a in &i.entries {
                let Some(list) = section.get(&a.event) else {
                    continue;
                };
                for (gi, g) in list.items().unwrap_or_default().iter().enumerate() {
                    let Some(hs) = g.get("hooks") else {
                        continue;
                    };
                    let items = hs.items().unwrap_or_default();
                    let Some(hi) = items.iter().position(|h| ours(h, &a.event)) else {
                        continue;
                    };
                    let only_ours = g.members().is_some_and(|m| m.len() == 1)
                        && items.iter().all(|h| ours(h, &a.event));
                    return Ok(Some(if only_ours {
                        jsontext::cut_item(text, list, gi)?
                    } else {
                        jsontext::cut_item(text, hs, hi)?
                    }));
                }
            }
        }
        Style::Named => {
            let Some(mine) = root.get(NAMED) else {
                return Ok(None);
            };
            for a in &i.entries {
                let Some(list) = mine.get(&a.event) else {
                    continue;
                };
                if let Some(hi) = list
                    .items()
                    .unwrap_or_default()
                    .iter()
                    .position(|h| ours(h, &a.event))
                {
                    return Ok(Some(jsontext::cut_item(text, list, hi)?));
                }
            }
        }
    }
    Ok(None)
}

/// The first member that paddock made and that is empty now: the text
/// without it.
fn cut_empty(text: &str, i: &Installed) -> Result<Option<String>> {
    let root = jsontext::parse(text)?;
    let empty = |n: &jsontext::Node| {
        n.items().is_some_and(<[_]>::is_empty) || n.members().is_some_and(<[_]>::is_empty)
    };
    let made = i.made.as_deref();
    match i.style {
        Style::Groups => {
            let Some(section) = root.get("hooks") else {
                return Ok(None);
            };
            for a in &i.entries {
                if section.get(&a.event).is_some_and(empty)
                    && made_by_paddock(made, &i.entries, &format!("hooks.{}", a.event))
                {
                    return Ok(Some(jsontext::cut_member(text, section, &a.event)?));
                }
            }
            if empty(section) && made_by_paddock(made, &i.entries, "hooks") {
                return Ok(Some(jsontext::cut_member(text, &root, "hooks")?));
            }
        }
        Style::Named => {
            let Some(mine) = root.get(NAMED) else {
                return Ok(None);
            };
            for a in &i.entries {
                if mine.get(&a.event).is_some_and(empty) {
                    return Ok(Some(jsontext::cut_member(text, mine, &a.event)?));
                }
            }
            if empty(mine) {
                return Ok(Some(jsontext::cut_member(text, &root, NAMED)?));
            }
        }
    }
    Ok(None)
}

/// Take paddock's entries out of the hook file again. Each text that
/// paddock inserted and that is still there as it was goes; then each
/// handler of paddock that the user moved; then each member that paddock
/// made and that is empty now. Every other byte stays.
pub fn remove(home: &Path, i: &Installed) -> Result<Removed> {
    let f = read(home, &i.file)?;
    let Some(bytes) = &f.bytes else {
        return Ok(Removed::Nothing);
    };
    let doc = parse(&f, &i.file)?;
    if !has_ours(&doc) {
        return Ok(Removed::Nothing);
    }
    let old = std::str::from_utf8(bytes).with_context(|| {
        format!(
            "~/{} is not UTF-8; paddock does not change it",
            i.file.display()
        )
    })?;
    let mut text = old.to_string();
    for ins in i.inserts.iter().rev() {
        if text.matches(ins.as_str()).count() == 1 {
            let cut = text.replacen(ins.as_str(), "", 1);
            if serde_json::from_str::<Value>(&cut).is_ok() {
                text = cut;
            }
        }
    }
    if !text.trim().is_empty() {
        while let Some(t) = cut_one(&text, i)? {
            text = t;
        }
        while let Some(t) = cut_empty(&text, i)? {
            text = t;
        }
    }
    let rest: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text)
            .context("paddock would make the hook file invalid; nothing was written")?
    };
    // Every value of the user stays.
    anyhow::ensure!(
        rest == strip(&doc, i.style, &i.entries, i.made.as_deref()),
        "paddock would change other entries of ~/{}; nothing was written",
        i.file.display()
    );
    if i.created && rest == json!({}) {
        let now = std::fs::symlink_metadata(&f.path)
            .ok()
            .map(|m| (m.len(), m.mtime(), m.mtime_nsec(), m.ino()));
        anyhow::ensure!(
            now == f.stamp,
            "{} changed; run the command again",
            f.path.display()
        );
        std::fs::remove_file(&f.path)?;
        return Ok(Removed::Deleted);
    }
    write(&f, text.as_bytes())?;
    let restored = i.before.as_deref() == Some(sha(text.as_bytes()).as_str());
    Ok(if restored {
        Removed::Restored
    } else {
        Removed::Stripped
    })
}
