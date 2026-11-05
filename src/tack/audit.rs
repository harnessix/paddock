//! tackd's audit log: one JSON line per decision.
//!
//! A line holds the time, the uid and pid that the kernel reported, the
//! program (path, real path, SHA-256), the caller chain summary, the names
//! of the secrets, the verdict and the reason. It never holds a value
//! (tackd does not read values to decide) nor an argument (an argument can
//! hold a value).
//!
//! The log is root's (0600, in a 0700 folder). It is rotated when it
//! reaches `MAX_BYTES`: `tackd.log` becomes `tackd.log.1`, and so on up to
//! `KEEP` old files. A user reads the lines of their own uid through tackd
//! (`paddock tack audit`); root reads the files.
//!
//! A crash can leave a last line without its end. The next line starts on
//! a new line, and readers skip lines that do not parse.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const KEEP: usize = 4;
/// The most lines that one audit request returns.
pub const MAX_LINES: usize = 2000;
/// The most bytes of lines that one answer carries (under the frame limit).
const MAX_ANSWER: usize = super::proto::MAX_FRAME - 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub time: u64,
    pub uid: u32,
    pub pid: u32,
    pub program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub chain: Vec<String>,
    pub secrets: Vec<String>,
    pub decision: String,
    pub reason: String,
}

impl Record {
    pub fn from_decision(
        time: u64,
        peer: super::policy::Peer,
        program: &str,
        d: &super::proto::Decision,
    ) -> Self {
        Record {
            time,
            uid: peer.uid,
            pid: peer.pid,
            program: program.to_string(),
            real: d.program.as_ref().map(|p| p.real.clone()),
            sha256: d.program.as_ref().map(|p| p.sha256.clone()),
            chain: d.chain.clone(),
            secrets: d.secrets.clone(),
            decision: d.verdict.name().to_string(),
            reason: d.reason.clone(),
        }
    }

    /// One line for `paddock tack audit`. The program and the reason can
    /// hold text that a client chose: control characters are shown as
    /// `?`, so a line cannot drive the terminal that reads it.
    pub fn show(&self) -> String {
        let clean = |s: &str| -> String {
            s.chars()
                .map(|c| if c.is_control() { '?' } else { c })
                .collect()
        };
        let r = Record {
            program: clean(&self.program),
            reason: clean(&self.reason),
            chain: self.chain.iter().map(|c| clean(c)).collect(),
            ..self.clone()
        };
        r.show_raw()
    }

    fn show_raw(&self) -> String {
        let secrets = if self.secrets.is_empty() {
            "-".to_string()
        } else {
            self.secrets.join(",")
        };
        let mut s = format!(
            "{}  {:<5}  {}  secrets {}  pid {}",
            crate::storage::date(self.time),
            self.decision,
            self.program,
            secrets,
            self.pid,
        );
        if let Some(h) = &self.sha256 {
            s.push_str(&format!("  sha256 {}", &h[..h.len().min(16)]));
        }
        s.push_str(&format!("\n    {}", self.reason));
        if !self.chain.is_empty() {
            s.push_str(&format!("\n    callers: {}", self.chain.join(" < ")));
        }
        s
    }
}

pub struct Log {
    path: PathBuf,
    max: u64,
    /// The owner that the folder and file must have: root in tackd, the
    /// test user in tests.
    owner: u32,
    lock: Mutex<()>,
}

impl Log {
    pub fn new(path: PathBuf, owner: u32) -> Self {
        Self::with_max(path, owner, MAX_BYTES)
    }

    pub fn with_max(path: PathBuf, owner: u32, max: u64) -> Self {
        Log {
            path,
            max,
            owner,
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut s = self.path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    }

    /// The folder: created 0700, and refused when someone else owns it or
    /// others can write it (a folder that a user made before setup).
    fn folder(&self) -> io::Result<()> {
        let d = self.path.parent().unwrap_or(Path::new("/"));
        match std::fs::symlink_metadata(d) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new().mode(0o700).create(d)?;
            }
            Err(e) => return Err(e),
            Ok(m) => {
                if !m.is_dir() || m.uid() != self.owner || m.mode() & 0o022 != 0 {
                    return Err(io::Error::other(format!(
                        "{} is not a folder that only uid {} can write",
                        d.display(),
                        self.owner
                    )));
                }
            }
        }
        Ok(())
    }

    fn open(&self) -> io::Result<File> {
        self.folder()?;
        let f = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)?;
        let m = f.metadata()?;
        if !m.is_file() || m.uid() != self.owner {
            return Err(io::Error::other(format!(
                "{} is not a file of uid {}",
                self.path.display(),
                self.owner
            )));
        }
        if m.mode() & 0o077 != 0 {
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(f)
    }

    fn rotate(&self) -> io::Result<()> {
        for n in (1..KEEP).rev() {
            let from = self.rotated(n);
            if from.exists() {
                std::fs::rename(&from, self.rotated(n + 1))?;
            }
        }
        std::fs::rename(&self.path, self.rotated(1))
    }

    /// Write one record and sync it. An error means the decision is not
    /// on record: tackd then does not allow.
    pub fn write(&self, r: &Record) -> io::Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.len() >= self.max) {
            self.rotate()?;
        }
        let mut f = self.open()?;
        let len = f.metadata()?.len();
        let mut line = Vec::with_capacity(512);
        if len > 0 {
            let mut last = [0u8; 1];
            f.seek(SeekFrom::Start(len - 1))?;
            f.read_exact(&mut last)?;
            if last[0] != b'\n' {
                line.push(b'\n');
            }
        }
        serde_json::to_writer(&mut line, r).map_err(io::Error::other)?;
        line.push(b'\n');
        f.write_all(&line)?;
        f.sync_data()
    }

    /// Whether a record could be written now, for status.
    pub fn check(&self) -> io::Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.open().map(|_| ())
    }

    /// The last `n` records of `uid` (all uids when `uid` is None), oldest
    /// first, as JSON lines, within the size of one answer.
    pub fn read(&self, uid: Option<u32>, n: usize) -> Vec<String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let n = n.min(MAX_LINES);
        let mut files: Vec<PathBuf> = (1..=KEEP).rev().map(|i| self.rotated(i)).collect();
        files.push(self.path.clone());
        let mut out: Vec<String> = Vec::new();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            for l in text.lines() {
                let Ok(r) = serde_json::from_str::<Record>(l) else {
                    continue;
                };
                if uid.is_none_or(|u| u == r.uid) {
                    out.push(l.to_string());
                }
            }
        }
        let mut size = 0;
        let mut keep = Vec::new();
        for l in out.into_iter().rev().take(n) {
            size += l.len() + 8;
            if size > MAX_ANSWER {
                break;
            }
            keep.push(l);
        }
        keep.reverse();
        keep
    }

    /// Every file of the log, for `remove`.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut v = vec![self.path.clone()];
        v.extend((1..=KEEP).map(|i| self.rotated(i)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(uid: u32, reason: &str) -> Record {
        Record {
            time: 1_700_000_000,
            uid,
            pid: 42,
            program: "/usr/bin/gh".into(),
            real: Some("/usr/bin/gh".into()),
            sha256: Some("ab".repeat(32)),
            chain: vec!["paddock[42]".into(), "bash[41]".into()],
            secrets: vec!["GH_TOKEN".into()],
            decision: "allow".into(),
            reason: reason.into(),
        }
    }

    fn me() -> u32 {
        unsafe { libc::geteuid() }
    }

    #[test]
    fn a_record_is_one_line_with_the_documented_fields() {
        let t = tempfile::tempdir().unwrap();
        let log = Log::new(t.path().join("audit/tackd.log"), me());
        log.write(&rec(1000, "evil\nline \"quoted\"")).unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(text.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        for k in [
            "time", "uid", "pid", "program", "real", "sha256", "chain", "secrets", "decision",
            "reason",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
        let m = std::fs::metadata(log.path()).unwrap();
        assert_eq!(m.mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(t.path().join("audit")).unwrap().mode() & 0o777,
            0o700
        );
        let mut evil = rec(1000, "r\x1b]0;owned\x07");
        evil.program = "/tmp/\x1b[2Jx".into();
        let shown = evil.show();
        assert!(
            !shown.contains('\x1b') && !shown.contains('\x07'),
            "{shown:?}"
        );
        let shown = rec(1000, "r").show();
        assert!(
            shown.contains("allow")
                && shown.contains("GH_TOKEN")
                && shown.contains("callers: paddock[42] < bash[41]"),
            "{shown}"
        );
    }

    #[test]
    fn a_user_reads_only_their_own_lines() {
        let t = tempfile::tempdir().unwrap();
        let log = Log::new(t.path().join("tackd.log"), me());
        for u in [1000, 1001, 1000] {
            log.write(&rec(u, &format!("for {u}"))).unwrap();
        }
        let mine = log.read(Some(1000), 100);
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|l| l.contains("\"uid\":1000")));
        assert_eq!(log.read(None, 100).len(), 3);
        assert_eq!(log.read(Some(1000), 1).len(), 1);
        assert!(log.read(Some(5), 100).is_empty());
    }

    #[test]
    fn the_log_rotates_and_keeps_a_bounded_number_of_files() {
        let t = tempfile::tempdir().unwrap();
        let log = Log::with_max(t.path().join("tackd.log"), me(), 600);
        for i in 0..40 {
            log.write(&rec(1000, &format!("n{i}"))).unwrap();
        }
        let files: Vec<_> = std::fs::read_dir(t.path()).unwrap().flatten().collect();
        assert_eq!(files.len(), KEEP + 1, "{files:?}");
        let lines = log.read(Some(1000), 1000);
        assert!(lines.last().unwrap().contains("\"n39\""));
        // Oldest first across the files.
        let nums: Vec<u32> = lines
            .iter()
            .map(|l| {
                serde_json::from_str::<Record>(l).unwrap().reason[1..]
                    .parse()
                    .unwrap()
            })
            .collect();
        assert!(nums.windows(2).all(|w| w[0] < w[1]), "{nums:?}");
    }

    #[test]
    fn a_line_cut_by_a_crash_is_skipped_and_the_next_starts_fresh() {
        let t = tempfile::tempdir().unwrap();
        let log = Log::new(t.path().join("tackd.log"), me());
        log.write(&rec(1000, "a")).unwrap();
        let mut f = OpenOptions::new().append(true).open(log.path()).unwrap();
        f.write_all(b"{\"time\":1,\"uid\":1000,\"pro").unwrap();
        log.write(&rec(1000, "b")).unwrap();
        let reasons: Vec<String> = log
            .read(Some(1000), 10)
            .iter()
            .map(|l| serde_json::from_str::<Record>(l).unwrap().reason)
            .collect();
        assert_eq!(reasons, ["a", "b"]);
    }

    #[test]
    fn a_planted_link_or_foreign_folder_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("elsewhere");
        std::fs::write(&target, "").unwrap();
        let p = t.path().join("tackd.log");
        std::os::unix::fs::symlink(&target, &p).unwrap();
        let log = Log::new(p, me());
        assert!(log.write(&rec(1, "x")).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
        // A folder that others can write.
        let d = t.path().join("open");
        std::fs::create_dir(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o777)).unwrap();
        let log = Log::new(d.join("tackd.log"), me());
        let e = log.write(&rec(1, "x")).unwrap_err();
        assert!(e.to_string().contains("only uid"), "{e}");
        // Owned by another uid.
        let log = Log::new(t.path().join("x/tackd.log"), me() + 1);
        assert!(log.write(&rec(1, "x")).is_err());
    }

    #[test]
    fn concurrent_writers_never_interleave_lines() {
        let t = tempfile::tempdir().unwrap();
        let log = std::sync::Arc::new(Log::with_max(t.path().join("tackd.log"), me(), 20_000));
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let log = log.clone();
                std::thread::spawn(move || {
                    for j in 0..50 {
                        log.write(&rec(1000 + i, &format!("{i}-{j}"))).unwrap();
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        for f in log.files() {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            for l in text.lines() {
                serde_json::from_str::<Record>(l).unwrap_or_else(|e| panic!("{e}: {l}"));
            }
        }
    }

    #[test]
    fn an_answer_stays_under_the_frame_limit() {
        let t = tempfile::tempdir().unwrap();
        let log = Log::with_max(t.path().join("tackd.log"), me(), u64::MAX);
        let long = "r".repeat(2000);
        for _ in 0..300 {
            log.write(&rec(1000, &long)).unwrap();
        }
        let lines = log.read(Some(1000), MAX_LINES);
        assert!(lines.len() < 300);
        assert!(lines.iter().map(|l| l.len() + 8).sum::<usize>() <= MAX_ANSWER);
    }
}
