//! The connection log of a session: `logs/connections.jsonl` of the
//! profile, next to `boundary.jsonl`. One line per decision: the time, the
//! app, the team, the host, the port, the verdict (`allow`, `deny` or
//! `ask`), the rule (a rule id, or `ask`, `once`, `timeout`, `stale`,
//! `mode`, `egress`, `approver`), and for a closed connection the bytes up
//! and down and the seconds. Never the content.
//!
//! The same merge and cap as the boundary log (`src/boundary.rs`): a
//! line for the same app, host, port, verdict and rule within a minute
//! raises the count and adds the bytes; the file keeps the last 5000
//! lines. One writer at a time, through `connections.lock`.

use super::text;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::Path;

pub use super::status::CONNECTIONS_FILE as FILE;
pub const MERGE_SECS: u64 = 60;
const MAX_LINES: usize = 5000;
const MAX_TEXT: usize = 300;

/// The verdict words.
pub const ALLOW: &str = "allow";
pub const DENY: &str = "deny";
pub const ASK: &str = "ask";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds of the first decision on this line.
    pub t: u64,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub host: String,
    pub port: u16,
    pub verdict: String,
    pub rule: String,
    /// Bytes from the app to the host, over every connection of the line.
    #[serde(default)]
    pub up: u64,
    #[serde(default)]
    pub down: u64,
    /// The seconds the connections of the line were open, added up.
    #[serde(default)]
    pub secs: u64,
    #[serde(default = "one")]
    pub count: u64,
    /// Unix seconds of the last decision on this line.
    #[serde(default)]
    pub last: u64,
}

fn one() -> u64 {
    1
}

fn cut(s: &str) -> String {
    crate::broker::clean_text(s)
        .chars()
        .take(MAX_TEXT)
        .collect()
}

impl Entry {
    pub fn new(
        now: u64,
        app: &str,
        team: Option<&str>,
        host: &str,
        port: u16,
        verdict: &str,
        rule: &str,
    ) -> Self {
        Self {
            t: now,
            app: cut(app),
            team: team.map(cut),
            host: cut(host),
            port,
            verdict: verdict.to_string(),
            rule: cut(rule),
            up: 0,
            down: 0,
            secs: 0,
            count: 1,
            last: now,
        }
    }

    /// The same entry with the bytes and the length of a closed connection.
    pub fn closed(mut self, up: u64, down: u64, secs: u64) -> Self {
        self.up = up;
        self.down = down;
        self.secs = secs;
        self
    }

    fn same(&self, o: &Entry) -> bool {
        self.app == o.app
            && self.team == o.team
            && self.host == o.host
            && self.port == o.port
            && self.verdict == o.verdict
            && self.rule == o.rule
    }

    /// The line of `paddock app log NAME`.
    pub fn render(&self) -> String {
        let time = crate::waiting::format_rfc3339(self.t);
        let bytes = (self.verdict == ALLOW && (self.up > 0 || self.down > 0 || self.secs > 0))
            .then_some((self.up, self.down, self.secs));
        text::connection_line(
            &time,
            &self.verdict,
            &self.app,
            &self.host,
            self.port,
            &self.rule,
            self.count,
            bytes,
        )
    }
}

fn line(e: &Entry) -> String {
    serde_json::to_string(e).unwrap_or_default()
}

/// Add `e` to the log at `path`: raise the count of a line for the same
/// decision that started less than a minute ago, or append a new line.
pub fn record_at(path: &Path, e: Entry) -> std::io::Result<()> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    crate::privfs::dir(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("connections.lock"))?;
    use std::os::fd::AsRawFd as _;
    // SAFETY: flock on an open file descriptor; the lock goes with the file.
    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
    let mut lines = read_at(path);
    if let Some(l) = lines
        .iter_mut()
        .rev()
        .find(|l| l.same(&e) && e.t.saturating_sub(l.t) < MERGE_SECS)
    {
        l.count += e.count;
        l.up += e.up;
        l.down += e.down;
        l.secs += e.secs;
        l.last = l.last.max(e.last);
        return write_all(path, &lines);
    }
    if lines.len() >= MAX_LINES {
        lines.drain(..=lines.len() - MAX_LINES);
        lines.push(e);
        return write_all(path, &lines);
    }
    let mut f = crate::privfs::append(path)?;
    writeln!(f, "{}", line(&e))
}

fn write_all(path: &Path, lines: &[Entry]) -> std::io::Result<()> {
    let mut s = String::new();
    for l in lines {
        s.push_str(&line(l));
        s.push('\n');
    }
    crate::privfs::write_atomic(path, s)
}

/// The lines of the log at `path`. Lines that do not parse are skipped.
pub fn read_at(path: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The counts by verdict, for the status line and the support bundle.
pub fn counts(path: &Path) -> super::status::Counts {
    super::status::count_connections(&std::fs::read(path).unwrap_or_default())
}

/// A new session starts a new log: the old one becomes `.1`, five stay.
pub fn rotate_for_session(path: &Path) {
    if path.exists() {
        super::logs::shift(path, super::logs::FIVE_BY_MIB);
    }
}

/// The lines of `paddock app log NAME` for the bytes of the file. A line
/// that is no entry is printed as it is.
pub fn render_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match serde_json::from_str::<Entry>(l) {
            Ok(e) => e.render(),
            Err(_) => l.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(t: u64, host: &str, verdict: &str, rule: &str) -> Entry {
        Entry::new(t, "com.openai.chat", Some("T1"), host, 443, verdict, rule)
    }

    #[test]
    fn round_trip_cap_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logs").join(FILE);
        // Two allows of the same target within a minute merge, with the
        // bytes added; a deny is its own line; after a minute a new line.
        record_at(
            &path,
            entry(100, "chatgpt.com", ALLOW, "seed-1").closed(10, 20, 3),
        )
        .unwrap();
        record_at(
            &path,
            entry(130, "chatgpt.com", ALLOW, "seed-1").closed(1, 2, 1),
        )
        .unwrap();
        record_at(&path, entry(131, "t.example", DENY, "timeout")).unwrap();
        record_at(
            &path,
            entry(170, "chatgpt.com", ALLOW, "seed-1").closed(5, 5, 5),
        )
        .unwrap();
        record_at(&path, entry(171, "t.example", ASK, "ask")).unwrap();
        let lines = read_at(&path);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(
            (
                lines[0].count,
                lines[0].up,
                lines[0].down,
                lines[0].secs,
                lines[0].last
            ),
            (2, 11, 22, 4, 130)
        );
        assert_eq!(lines[1].verdict, DENY);
        assert_eq!(lines[2].t, 170);
        assert_eq!(lines[3].verdict, ASK);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // The counts: allow and deny by the count field, the rest by word.
        let c = counts(&path);
        assert_eq!((c.allowed, c.denied), (3, 1));
        assert_eq!(c.other, vec![("ask".to_string(), 1)]);
        assert_eq!(c.last.as_deref(), Some("t.example:443"));
        assert_eq!(c.hosts, ["chatgpt.com", "t.example"]);
        // The rendered lines.
        let shown = render_lines(&std::fs::read(&path).unwrap());
        assert!(
            shown[0].contains("chatgpt.com:443")
                && shown[0].contains("x2")
                && shown[0].contains("22 B down"),
            "{}",
            shown[0]
        );
        assert!(
            shown[1].contains("deny") && shown[1].contains("timeout") && !shown[1].contains(" up,"),
            "{}",
            shown[1]
        );
        assert!(shown[0].starts_with("1970-01-01T00:01:40Z"), "{}", shown[0]);
        // A bad line is kept as text, and skipped by the reader.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        assert_eq!(read_at(&path).len(), 4);
        assert_eq!(
            render_lines(&std::fs::read(&path).unwrap()).last().unwrap(),
            "not json"
        );
        // The cap: the last MAX_LINES stay.
        let big = tmp.path().join("big.jsonl");
        let mut s = String::new();
        for i in 0..MAX_LINES as u64 {
            s.push_str(&line(&entry(
                i * 100,
                &format!("h{i}.example"),
                ALLOW,
                "egress",
            )));
            s.push('\n');
        }
        std::fs::write(&big, s).unwrap();
        record_at(&big, entry(u64::MAX / 2, "new.example", ALLOW, "egress")).unwrap();
        let lines = read_at(&big);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0].host, "h1.example");
        assert_eq!(lines.last().unwrap().host, "new.example");
        // Text from the guest is cleaned and cut.
        let e = Entry::new(1, "a\u{202E}b", None, &"x".repeat(400), 1, DENY, "mode");
        assert_eq!(e.app, "ab");
        assert_eq!(e.host.len(), MAX_TEXT);
        // The session rotation.
        rotate_for_session(&path);
        assert!(!path.exists());
        assert!(super::super::logs::numbered(&path, 1).exists());
        rotate_for_session(&path);
        assert!(
            super::super::logs::numbered(&path, 1).exists(),
            "a missing file rotates nothing"
        );
        assert!(counts(&path).hosts.is_empty());
    }
}
