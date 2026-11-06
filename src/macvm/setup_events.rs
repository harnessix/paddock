//! The event log of a golden build: `events.jsonl` in the golden folder,
//! one JSON line per state start and end and per wait on the person. A
//! line that waits on the person carries `"person":true` and one of the
//! [`PERSON_KINDS`]; `packaging/macos/checks/macvm-first-run.sh` counts
//! them. A write error is logged and never fails the setup.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The person made the guest account in Setup Assistant.
pub const ACCOUNT_CREATED: &str = "account created";
/// The person ran the install command of the seed in the guest.
pub const SEED_INSTALLED: &str = "seed installed";
/// The person turned on Accessibility for paddock-guest.
pub const TRUSTED: &str = "trusted";
/// The kinds that wait on the person, in their order in the guided step.
pub const PERSON_KINDS: [&str; 3] = [ACCOUNT_CREATED, SEED_INSTALLED, TRUSTED];
/// A state starts, ends, stops or fails; the text names it.
pub const STATE: &str = "state";
/// The seed folder is ready to share.
pub const SEED_PREPARED: &str = "seed prepared";
/// `sudo -S` in the guest refused the password at configure; the text is
/// the count. Never the password.
pub const PASSWORD_REFUSED: &str = "password refused";
/// The keyring took the password that the person typed into the panel.
pub const PASSWORD_REPLACED: &str = "password replaced";
/// The offline seed of the disk ended: `done: N files, fsck clean`,
/// `done: ..., fsck skipped: WHY`, `failed: WHY` or `skipped: WHY`. Only a
/// `done` line means that the agent is on the disk.
pub const SEED_OFFLINE: &str = "seed offline";
/// The offline trust ended: `updated`, `inserted`, `already` or
/// `failed: WHY`.
pub const TRUST_OFFLINE: &str = "trust offline";
/// The offline link of `~/Downloads` (`offline_link.rs`) ended: `made`,
/// `already`, `failed: WHY` or `skipped: WHY`. Only `made` and `already`
/// mean that the link is there.
pub const DOWNLOADS_OFFLINE: &str = "downloads offline";
/// The agent did not dial in after the boot of a seeded disk: the guided
/// panel shows the typed command. The text names the wait.
pub const AGENT_LATE: &str = "agent late";

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// RFC 3339, UTC, whole seconds.
    pub t: String,
    pub kind: String,
    #[serde(default)]
    pub person: bool,
    #[serde(default)]
    pub text: String,
}

/// Append one line to `path`.
pub fn record(path: &Path, kind: &str, person: bool, text: &str) {
    let line = Line {
        t: super::golden::now_text(),
        kind: kind.to_string(),
        person,
        text: text.to_string(),
    };
    log::info!("macvm setup event {kind}: {text}");
    if let Err(e) = append(path, &line) {
        log::warn!("macvm: cannot write {}: {e}", path.display());
    }
}

/// Append a person line of `kind` unless the log has one: a guided step
/// that runs again after a stop does not count a wait twice.
pub fn record_person_once(path: &Path, kind: &str, text: &str) {
    if read(path).iter().any(|l| l.person && l.kind == kind) {
        return;
    }
    record(path, kind, true, text);
}

/// Append a line of `kind` with `"person":false` unless the log has a
/// line of that kind: paddock did a wait that the guided step leaves to
/// the person, so the clicks check counts no click for it.
pub fn record_once(path: &Path, kind: &str, text: &str) {
    if read(path).iter().any(|l| l.kind == kind) {
        return;
    }
    record(path, kind, false, text);
}

fn append(path: &Path, line: &Line) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(line)?;
    bytes.push(b'\n');
    // A torn last line of a crash gets its end first, so this line
    // stays whole.
    if std::fs::read(path).is_ok_and(|b| b.last().is_some_and(|c| *c != b'\n')) {
        bytes.insert(0, b'\n');
    }
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(&bytes)
}

/// Every line that parses. A torn last line (a crash in a write) and a
/// line of another shape are skipped.
pub fn read(path: &Path) -> Vec<Line> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn person_lines_carry_person_true_and_the_kind_as_the_script_reads_them() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("events.jsonl");
        record(&p, STATE, false, "seed starts");
        for k in PERSON_KINDS {
            record_person_once(&p, k, "by the agent");
            record_person_once(&p, k, "again");
        }
        let text = std::fs::read_to_string(&p).unwrap();
        // The extraction of macvm-first-run.sh: grep, then sed.
        let kinds: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("\"person\":true"))
            .map(|l| {
                let at = l.find("\"kind\":\"").unwrap() + 8;
                let rest = &l[at..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        assert_eq!(kinds, PERSON_KINDS.to_vec());
        assert_eq!(read(&p).len(), 4);
    }

    /// The clicks check of the Mac script sorts the kinds and compares
    /// them with its `expect` lines: each must hold every person kind.
    #[test]
    fn first_run_script_expects_the_person_kinds() {
        let script = include_str!("../../packaging/macos/checks/macvm-first-run.sh");
        let expects: Vec<&str> = script
            .lines()
            .filter_map(|l| l.split_once("expect=\"").map(|(_, r)| r))
            .filter(|r| r.contains("account created"))
            .map(|r| r.split('"').next().unwrap())
            .collect();
        assert_eq!(expects.len(), 2, "{expects:?}");
        let mut sorted = PERSON_KINDS.to_vec();
        sorted.sort();
        assert_eq!(expects[0], format!("{},", sorted.join(",")));
        let mut with_pasteboard = sorted.clone();
        with_pasteboard.push("pasteboard allowed");
        with_pasteboard.sort();
        assert_eq!(expects[1], format!("{},", with_pasteboard.join(",")));
        assert!(script.contains("for n in 2 3 4 5 6;"));
        assert!(script.contains("\" 2 3 4 5 6\""));
    }

    #[test]
    fn record_once_writes_a_wait_without_the_person_and_only_once() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("events.jsonl");
        record_once(&p, ACCOUNT_CREATED, "paddock");
        record_once(&p, ACCOUNT_CREATED, "again");
        record_person_once(&p, SEED_INSTALLED, "the Continue button");
        record_once(&p, SEED_INSTALLED, "paddock");
        let lines = read(&p);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].person && lines[1].person);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.matches("\"person\":true").count(), 1);
    }

    #[test]
    fn a_torn_line_is_skipped() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("events.jsonl");
        record(&p, STATE, false, "a");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"{\"t\":\"2026")
            .unwrap();
        assert_eq!(read(&p).len(), 1);
        record(&p, STATE, false, "b");
        assert_eq!(read(&p).len(), 2, "the next line stays whole");
        assert!(read(&t.path().join("none")).is_empty());
    }
}
