//! The event log of a profile: `logs/events.jsonl`, one JSON line per state
//! change of a session. Time, kind, text and the numbers. 2 MiB cap: the
//! oldest lines go. The reader side (`read`, `tail`, `since`, `follow`)
//! serves `paddock app log --events` and the report; a line of a kind this
//! paddock does not know is kept as [`Kind::Other`].

use super::session::Profile;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The cap of the file.
pub const CAP: u64 = 2 << 20;
/// The size after a trim: the newest lines that fit.
const KEEP: u64 = CAP * 3 / 4;

/// One event line.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    /// RFC 3339, UTC, whole seconds.
    pub t: String,
    /// The millisecond part of the time, for the bench.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ms: u16,
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub n: serde_json::Map<String, serde_json::Value>,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

/// The writer of the event log.
pub struct Events {
    path: PathBuf,
}

impl Events {
    /// The log of `profile`. The `logs/` folder is made when missing.
    pub fn open(profile: &Profile) -> Self {
        let _ = std::fs::create_dir_all(profile.logs());
        Self {
            path: profile.events_log(),
        }
    }

    pub fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one line. `numbers` are the measured values of the event
    /// (`secs`, `bytes`). A write error is logged and never fails the
    /// session.
    pub fn record(&self, kind: &str, text: &str, numbers: &[(&str, f64)]) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        self.record_ms(
            now.as_secs(),
            now.subsec_millis() as u16,
            kind,
            text,
            numbers,
        );
    }

    pub fn record_at(&self, secs: u64, kind: &str, text: &str, numbers: &[(&str, f64)]) {
        self.record_ms(secs, 0, kind, text, numbers);
    }

    pub fn record_ms(&self, secs: u64, ms: u16, kind: &str, text: &str, numbers: &[(&str, f64)]) {
        let mut n = serde_json::Map::new();
        for (k, v) in numbers {
            let v = if v.fract() == 0.0 && v.abs() < 1e15 {
                serde_json::json!(*v as i64)
            } else {
                serde_json::json!((v * 1000.0).round() / 1000.0)
            };
            n.insert((*k).to_string(), v);
        }
        let ev = Event {
            t: crate::waiting::format_rfc3339(secs),
            ms,
            kind: kind.to_string(),
            text: text.to_string(),
            n,
        };
        log::info!("macvm event {kind}: {text}");
        if let Err(e) = self.append(&ev) {
            log::warn!("macvm: cannot write {}: {e}", self.path.display());
        }
    }

    fn append(&self, ev: &Event) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(ev)?;
        line.push(b'\n');
        let len = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if len + line.len() as u64 > CAP {
            self.trim(KEEP.saturating_sub(line.len() as u64))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)?;
        f.write_all(&line)
    }

    /// Keep the newest whole lines that fit in `keep` bytes.
    fn trim(&self, keep: u64) -> std::io::Result<()> {
        let bytes = std::fs::read(&self.path)?;
        let kept = newest_lines(&bytes, keep as usize);
        crate::privfs::write_atomic(&self.path, kept)
    }
}

/// The newest whole lines of `bytes` that fit in `keep` bytes.
pub fn newest_lines(bytes: &[u8], keep: usize) -> &[u8] {
    if bytes.len() <= keep {
        return bytes;
    }
    let from = bytes.len() - keep;
    match bytes[from..].iter().position(|b| *b == b'\n') {
        Some(nl) => &bytes[from + nl + 1..],
        None => &[],
    }
}

pub use super::logs::rotate;

/// The kinds of event that the shim and the parent write. The list is
/// the contract of the Mac check scripts, which `grep` for the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// The start decision: `fresh: ...`, `warm: ...`, `cold: ...`.
    Start,
    /// The system part of the profile is in place after a clone.
    Prepare,
    /// The restore of the saved state, or its failure.
    Restore,
    /// The VM runs (booting or resumed).
    Running,
    /// The guest agent dialed in.
    Hello,
    /// The agent: update, rollback, the connection closed.
    Agent,
    /// `health` lost and back.
    Health,
    /// The host slept and woke.
    Sleep,
    Wake,
    /// The app exited.
    App,
    /// `app.launch` went out.
    Launch,
    /// The app copy into the guest.
    Copy,
    /// The quit path: asked, forced, the second signal.
    Quit,
    /// The save of the machine state, or why there is none.
    Save,
    /// The end of the session: `quit with save`, `crashed`, ...
    Stop,
    /// The shim process ends, with the exit code.
    End,
    /// Another paddock saw the dead pid: `session ended without a save`.
    Crashed,
    Error,
    Timeout,
    Network,
    Disk,
    Data,
    Backup,
    Window,
    Drop,
    Clipboard,
    Files,
    Badge,
    Bench,
    Guest,
    /// A kind of another paddock version, kept as its word.
    Other(String),
}

impl Kind {
    /// The kinds this paddock writes, in the order of a session.
    pub const KNOWN: [Kind; 30] = [
        Kind::Start,
        Kind::Prepare,
        Kind::Restore,
        Kind::Running,
        Kind::Hello,
        Kind::Agent,
        Kind::Health,
        Kind::Sleep,
        Kind::Wake,
        Kind::App,
        Kind::Launch,
        Kind::Copy,
        Kind::Quit,
        Kind::Save,
        Kind::Stop,
        Kind::End,
        Kind::Crashed,
        Kind::Error,
        Kind::Timeout,
        Kind::Network,
        Kind::Disk,
        Kind::Data,
        Kind::Backup,
        Kind::Window,
        Kind::Drop,
        Kind::Clipboard,
        Kind::Files,
        Kind::Badge,
        Kind::Bench,
        Kind::Guest,
    ];

    /// The word of the log line.
    pub fn word(&self) -> &str {
        match self {
            Kind::Start => "start",
            Kind::Prepare => "prepare",
            Kind::Restore => "restore",
            Kind::Running => "running",
            Kind::Hello => "hello",
            Kind::Agent => "agent",
            Kind::Health => "health",
            Kind::Sleep => "sleep",
            Kind::Wake => "wake",
            Kind::App => "app",
            Kind::Launch => "launch",
            Kind::Copy => "copy",
            Kind::Quit => "quit",
            Kind::Save => "save",
            Kind::Stop => "stop",
            Kind::End => "end",
            Kind::Crashed => "crashed",
            Kind::Error => "error",
            Kind::Timeout => "timeout",
            Kind::Network => "network",
            Kind::Disk => "disk",
            Kind::Data => "data",
            Kind::Backup => "backup",
            Kind::Window => "window",
            Kind::Drop => "drop",
            Kind::Clipboard => "clipboard",
            Kind::Files => "files",
            Kind::Badge => "badge",
            Kind::Bench => "bench",
            Kind::Guest => "guest",
            Kind::Other(w) => w,
        }
    }

    /// The kind of a word. An unknown word is `Other`.
    pub fn parse(word: &str) -> Kind {
        Kind::KNOWN
            .iter()
            .find(|k| k.word() == word)
            .cloned()
            .unwrap_or_else(|| Kind::Other(word.to_string()))
    }

    /// Whether the word is one this paddock writes.
    pub fn known(&self) -> bool {
        !matches!(self, Kind::Other(_))
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.word())
    }
}

impl Event {
    pub fn kind(&self) -> Kind {
        Kind::parse(&self.kind)
    }

    /// Seconds since the epoch of `t`, or 0 for a time that does not parse.
    pub fn secs(&self) -> u64 {
        crate::waiting::parse_rfc3339(&self.t).unwrap_or(0)
    }

    /// The line of `paddock app log --events`.
    pub fn render(&self) -> String {
        let numbers: Vec<(String, String)> = self
            .n
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect();
        super::text::event_line(&self.t, &self.kind, &self.text, &numbers)
    }
}

/// Every event of the file, in order. A line that does not parse is
/// skipped, a missing file is empty.
pub fn read(path: &Path) -> Vec<Event> {
    parse_lines(&std::fs::read(path).unwrap_or_default())
}

/// The events of a byte range of the file.
pub fn parse_lines(bytes: &[u8]) -> Vec<Event> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|l| serde_json::from_str::<Event>(l).ok())
        .collect()
}

/// The last `n` events.
pub fn tail(path: &Path, n: usize) -> Vec<Event> {
    let mut all = read(path);
    let from = all.len().saturating_sub(n);
    all.drain(..from);
    all
}

/// The events at or after `secs` (seconds since the epoch).
pub fn since(path: &Path, secs: u64) -> Vec<Event> {
    read(path)
        .into_iter()
        .filter(|e| e.secs() >= secs)
        .collect()
}

/// The `--follow` loop: every new event of the file goes to `sink`, each
/// `poll`, until `stop()` says so after the file stopped growing. A file
/// that shrinks (the cap trimmed it) is read from its new start. Returns
/// the events seen.
pub fn follow(
    path: &Path,
    from: u64,
    poll: std::time::Duration,
    sink: &mut dyn FnMut(&Event),
    stop: &dyn Fn() -> bool,
) -> u64 {
    let mut at = from;
    loop {
        let bytes = std::fs::read(path).unwrap_or_default();
        if (bytes.len() as u64) < at {
            at = 0;
        }
        let new = &bytes[at as usize..];
        // Only whole lines: a line in the middle of a write waits.
        let whole = match new.iter().rposition(|b| *b == b'\n') {
            Some(i) => &new[..=i],
            None => &[][..],
        };
        for e in parse_lines(whole) {
            sink(&e);
        }
        at += whole.len() as u64;
        if stop() {
            return at;
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_since_unknown() {
        let t = tempfile::tempdir().unwrap();
        let p = Profile::new(t.path().join("p"));
        let ev = Events::open(&p);
        for (i, kind) in ["start", "hello", "teleport", "quit", "stop"]
            .iter()
            .enumerate()
        {
            ev.record_at(1_700_000_000 + i as u64 * 10, kind, "x", &[("i", i as f64)]);
        }
        let all = read(ev.path());
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].kind(), Kind::Start);
        // An unknown kind is kept as its word.
        assert_eq!(all[2].kind(), Kind::Other("teleport".into()));
        assert!(!all[2].kind().known());
        assert_eq!(all[2].kind().to_string(), "teleport");
        assert_eq!(Kind::parse("stop"), Kind::Stop);
        for k in Kind::KNOWN {
            assert_eq!(Kind::parse(k.word()), k);
        }
        let last = tail(ev.path(), 2);
        assert_eq!(
            last.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["quit", "stop"]
        );
        assert_eq!(tail(ev.path(), 50).len(), 5);
        let late = since(ev.path(), 1_700_000_020);
        assert_eq!(late.len(), 3);
        assert_eq!(late[0].kind, "teleport");
        assert_eq!(all[1].secs(), 1_700_000_010);
        assert!(all[1]
            .render()
            .starts_with("2023-11-14T22:13:30Z  hello    x  [i=1]"));
        // A missing file and a broken line.
        assert!(read(&t.path().join("none")).is_empty());
        assert_eq!(
            parse_lines(b"{bad\n{\"t\":\"T\",\"kind\":\"end\",\"text\":\"\"}\n").len(),
            1
        );
    }

    #[test]
    fn follow_sees_new_whole_lines_only() {
        let t = tempfile::tempdir().unwrap();
        let p = Profile::new(t.path().join("p"));
        let ev = Events::open(&p);
        ev.record_at(1, "start", "a", &[]);
        let len = std::fs::metadata(ev.path()).unwrap().len();
        // A new line, then a half line: one event.
        ev.record_at(2, "hello", "b", &[]);
        std::fs::OpenOptions::new()
            .append(true)
            .open(ev.path())
            .unwrap()
            .write_all(b"{\"t\":\"T\",\"kind\":\"quit")
            .unwrap();
        let mut seen = Vec::new();
        let at = follow(
            ev.path(),
            len,
            std::time::Duration::from_millis(1),
            &mut |e| seen.push(e.kind.clone()),
            &|| true,
        );
        assert_eq!(seen, ["hello"]);
        assert!(at < std::fs::metadata(ev.path()).unwrap().len());
        // The file shrank (the cap trimmed it): read from the new start.
        std::fs::write(ev.path(), "{\"t\":\"T\",\"kind\":\"end\",\"text\":\"\"}\n").unwrap();
        let mut seen = Vec::new();
        follow(
            ev.path(),
            at,
            std::time::Duration::from_millis(1),
            &mut |e| seen.push(e.kind.clone()),
            &|| true,
        );
        assert_eq!(seen, ["end"]);
    }

    #[test]
    fn cap_keeps_newest() {
        let t = tempfile::tempdir().unwrap();
        let p = Profile::new(t.path().join("p"));
        let ev = Events::open(&p);
        assert!(p.logs().is_dir());
        let text = "x".repeat(1000);
        // About 2.2 MiB of lines, numbered.
        for i in 0..2200 {
            ev.record_at(1_700_000_000 + i, "test", &text, &[("i", i as f64)]);
        }
        let len = std::fs::metadata(ev.path()).unwrap().len();
        assert!(len <= CAP, "{len}");
        assert!(len > CAP / 2, "{len}");
        let content = std::fs::read_to_string(ev.path()).unwrap();
        let lines: Vec<Event> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // Whole lines only, the newest last.
        assert_eq!(lines.last().unwrap().n["i"], 2199);
        assert!(lines.first().unwrap().n["i"].as_i64().unwrap() > 0);
        let is: Vec<i64> = lines.iter().map(|l| l.n["i"].as_i64().unwrap()).collect();
        assert!(is.windows(2).all(|w| w[1] == w[0] + 1));
        assert_eq!(
            lines[0].t,
            crate::waiting::format_rfc3339(1_700_000_000 + is[0] as u64)
        );
        // Fractions are rounded to three places, whole numbers stay whole.
        ev.record_at(1, "k", "text", &[("secs", 4.12345), ("bytes", 7.0)]);
        let last = std::fs::read_to_string(ev.path()).unwrap();
        let last: Event = serde_json::from_str(last.lines().last().unwrap()).unwrap();
        assert_eq!(last.n["secs"], 4.123);
        assert_eq!(last.n["bytes"], 7);
        assert_eq!(last.t, "1970-01-01T00:00:01Z");
        assert_eq!(newest_lines(b"ab\ncd\nef", 5), b"ef");
        assert_eq!(newest_lines(b"ab\ncd\nef", 2), b"");
        assert_eq!(newest_lines(b"ab\ncd\nef", 8), b"ab\ncd\nef");
    }

    #[test]
    fn rotate_keeps_five_files() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("console.log");
        for i in 0..7 {
            std::fs::write(&log, format!("boot {i}").repeat(100)).unwrap(); // not user text
            rotate(&log, 5, 100);
        }
        // The newest is rotated away: boot 6 became console.log.1.
        assert!(!log.exists());
        let names: Vec<String> = (1..=5)
            .filter(|n| t.path().join(format!("console.log.{n}")).exists())
            .map(|n| {
                std::fs::read_to_string(t.path().join(format!("console.log.{n}"))).unwrap()[..6]
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["boot 6", "boot 5", "boot 4", "boot 3"]);
        assert!(
            !t.path().join("console.log.5").exists(),
            "the fifth slot stays free for the next rotation"
        );
        // A short file is left alone.
        std::fs::write(&log, "short").unwrap();
        rotate(&log, 5, 100);
        assert!(log.exists());
    }
}
