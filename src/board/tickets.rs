//! Tickets: the files `.paddock/tickets/*.md` of a project's notes.
//!
//! - The ID is the file name before the first `-` or `.`: `KV2` for
//!   `KV2-worker.md`.
//! - The title is the first heading, without a leading `ID:`.
//! - A line that starts with `Status:` makes the file a ticket even when
//!   no run has it as its prompt. Other files show only with a run.

use super::origin::Origin;
use std::path::{Path, PathBuf};

/// The folder of the tickets in the notes folder.
pub const DIR: &str = "tickets";
/// paddock reads at most this much of a ticket file.
const MAX_READ: u64 = 1 << 20;
/// At most this many ticket files per project.
const MAX_FILES: usize = 2000;
/// The longest title.
const MAX_TITLE: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketFile {
    pub id: String,
    pub title: String,
    /// `.paddock/tickets/NAME.md`.
    pub file: String,
    /// The file name, `NAME.md`.
    pub name: String,
    /// The text after `Status:`, on one line.
    pub status: Option<String>,
    /// SHA-256 of the file, hex.
    pub sha256: String,
}

/// The ID of a ticket file name.
pub fn id_of(name: &str) -> String {
    let stem = name.strip_suffix(".md").unwrap_or(name);
    match stem.split(['-', '.']).next() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => stem.to_string(),
    }
}

/// Read one ticket from its text.
pub fn parse(name: &str, bytes: &[u8]) -> TicketFile {
    let text = String::from_utf8_lossy(bytes);
    // A file name can hold terminal escapes: the ID shows on a terminal.
    let id = match super::rules::one_line(&id_of(name)) {
        id if id.is_empty() => "?".to_string(),
        id => id,
    };
    let heading = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim());
    let title = heading.map_or_else(
        || name.strip_suffix(".md").unwrap_or(name).to_string(),
        // The ID is cleaned, so the heading is cleaned before it loses
        // its `ID:`.
        |h| strip_id(&super::rules::one_line(h), &id).to_string(),
    );
    let status = text
        .lines()
        .find_map(|l| l.trim_end().strip_prefix("Status:"))
        .map(|s| super::rules::one_line(s.trim()));
    TicketFile {
        title: clip(&super::rules::one_line(&title), MAX_TITLE),
        // The notes folder of the project. A ticket that is only in the
        // loft has this path in the loft's `tree/`, not in the project.
        file: format!("{}/{DIR}/{name}", crate::loft::NOTES),
        name: name.to_string(),
        status,
        sha256: super::origin::sha256_hex(bytes),
        id,
    }
}

/// `KV2: named kv namespaces` → `named kv namespaces`.
fn strip_id<'a>(heading: &'a str, id: &str) -> &'a str {
    let Some(head) = heading.get(..id.len()) else {
        return heading;
    };
    if !head.eq_ignore_ascii_case(id) {
        return heading;
    }
    let rest = &heading[id.len()..];
    for sep in [":", " -", " —", " –"] {
        if let Some(r) = rest.strip_prefix(sep) {
            let r = r.trim();
            if !r.is_empty() {
                return r;
            }
        }
    }
    heading
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max - 1).collect::<String>() + "…"
}

/// The ticket folders of a repository: the loft's notes, then the
/// project's own `.paddock/` (the same folder when it links to the loft).
pub fn dirs(env: &crate::env::HostEnv, repo: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen = Vec::new();
    for d in [
        crate::loft::Loft::of(env, repo).notes().join(DIR),
        repo.join(crate::loft::NOTES).join(DIR),
    ] {
        let real = std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone());
        if d.is_dir() && !seen.contains(&real) {
            seen.push(real);
            out.push(d);
        }
    }
    out
}

/// The tickets of a repository, by file name. A file in the loft wins
/// over a file of the same name in the project.
pub fn load(env: &crate::env::HostEnv, repo: &Path) -> Vec<TicketFile> {
    let mut out: Vec<TicketFile> = Vec::new();
    for d in dirs(env, repo) {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.ends_with(".md") && !n.starts_with('.'))
            .collect();
        names.sort();
        for name in names.into_iter().take(MAX_FILES) {
            if out.iter().any(|t| t.name == name) {
                continue;
            }
            let Some(bytes) = read_head(&d.join(&name)) else {
                continue;
            };
            out.push(parse(&name, &bytes));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_head(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(p).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_READ).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// The file name of a prompt path inside a tickets folder: the last
/// `.paddock/tickets/NAME` of the path. Relative paths count, and so do
/// the paths of a worktree or of the loft.
pub fn ticket_name(path: &str) -> Option<&str> {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let n = parts.len();
    if n >= 3 && parts[n - 3] == crate::loft::NOTES && parts[n - 2] == DIR {
        return Some(parts[n - 1]);
    }
    None
}

/// The ticket that a run belongs to: its prompt file, else a ticket with
/// the same text as its prompt.
pub fn owner(o: &Origin, files: &[TicketFile]) -> Option<usize> {
    if let Some(name) = o.file.as_deref().and_then(ticket_name) {
        if let Some(i) = files.iter().position(|t| t.name == name) {
            return Some(i);
        }
    }
    if o.sha256.is_empty() {
        return None;
    }
    files.iter().position(|t| t.sha256 == o.sha256)
}
