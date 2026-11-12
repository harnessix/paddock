//! Host session fixtures for tests: fake homes with Claude Code and Codex
//! transcripts. Nothing here reads or writes a real home.

use crate::horse::{claude, codex};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const CLAUDE_A: &str = "01934b20-1111-7777-8888-0000000000a1";
pub const CLAUDE_B: &str = "01934b20-1111-7777-8888-0000000000b2";
pub const CODEX_A: &str = "01934b20-1111-7777-8888-0000000000c3";

/// A Claude Code chain row of session `id`.
pub fn claude_row(id: &str, n: usize, text: &str) -> String {
    let uuid = format!("00000000-0000-4000-8000-{n:012}");
    let parent = match n {
        0 => "null".to_string(),
        _ => format!("\"00000000-0000-4000-8000-{:012}\"", n - 1),
    };
    let (ty, role) = if n.is_multiple_of(2) {
        ("user", "user")
    } else {
        ("assistant", "assistant")
    };
    format!(
        "{{\"type\":\"{ty}\",\"uuid\":\"{uuid}\",\"parentUuid\":{parent},\"sessionId\":\"{id}\",\"version\":\"{}\",\"message\":{{\"role\":\"{role}\",\"content\":\"{text}\"}}}}",
        claude::VERSION
    )
}

/// The rows `0..n` of a Claude Code session.
pub fn claude_rows(id: &str, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| claude_row(id, i, &format!("row {i}")))
        .collect()
}

/// The transcript of `id` under `home`, relative to the home.
pub fn claude_rel(project: &str, id: &str) -> PathBuf {
    PathBuf::from(format!(".claude/projects/{project}/{id}.jsonl"))
}

/// Write a Claude Code transcript; returns its path.
pub fn write_claude(home: &Path, project: &str, id: &str, rows: &[String]) -> PathBuf {
    let p = home.join(claude_rel(project, id));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut b = rows.join("\n");
    b.push('\n');
    std::fs::write(&p, b).unwrap();
    p
}

pub fn codex_rel(id: &str) -> PathBuf {
    PathBuf::from(format!(
        ".codex/sessions/2026/11/04/rollout-2026-11-04T00-00-00-{id}.jsonl"
    ))
}

/// The rows of a Codex rollout: the metadata row and `n` messages.
pub fn codex_rows(id: &str, n: usize) -> Vec<String> {
    let mut v = vec![format!(
        r#"{{"timestamp":"2026-11-04T00:00:00Z","type":"session_meta","payload":{{"id":"{id}","session_id":"{id}","cli_version":"{}","cwd":"/home/demo/project"}}}}"#,
        codex::VERSION
    )];
    for i in 0..n {
        v.push(format!(
            r#"{{"timestamp":"2026-11-04T00:00:0{}Z","type":"response_item","payload":{{"type":"reasoning","id":"rs_{i}","encrypted_content":"x{i}=="}}}}"#,
            i % 10
        ));
    }
    v
}

pub fn write_codex(home: &Path, id: &str, rows: &[String]) -> PathBuf {
    let p = home.join(codex_rel(id));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut b = rows.join("\n");
    b.push('\n');
    std::fs::write(&p, b).unwrap();
    p
}

/// Every entry below `dir`: its kind, its mode, its mtime and the sha256
/// of a file's bytes. Two equal values mean that nothing below changed.
pub fn tree(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let m = std::fs::symlink_metadata(&p).unwrap();
        let rel = p.strip_prefix(root).unwrap().to_path_buf();
        let stamp = format!("{:o} {}.{}", m.mode(), m.mtime(), m.mtime_nsec());
        if m.is_dir() {
            out.insert(rel, format!("dir {stamp}"));
            walk(root, &p, out);
        } else if m.is_file() {
            let h = Sha256::digest(std::fs::read(&p).unwrap());
            out.insert(rel, format!("file {stamp} {h:x}"));
        } else {
            out.insert(rel, format!("other {stamp}"));
        }
    }
}
