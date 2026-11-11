//! The agent log: `~/Library/Logs/paddock-guest.log`, five files of 1 MiB.
//! `log.fetch` sends the current file over the `guest-files` port. No line
//! holds the token, the password or any clipboard text: the agent logs
//! `kind()` of each message, never the message.

use log::{LevelFilter, Log, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A file rotates when it would grow past this.
pub const MAX_BYTES: u64 = 1 << 20;
/// The current file plus four rotated ones.
pub const KEEP: usize = 5;
/// The file name.
pub const NAME: &str = "paddock-guest.log";

/// The log path of the logged-in user: `~/Library/Logs/paddock-guest.log`.
pub fn default_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library").join("Logs").join(NAME)
}

/// The rotated names of a log: `NAME.1` is the newest rotated file.
fn rotated(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// Rotate: `.{keep-1}` goes, each `.n` becomes `.n+1`, the file becomes
/// `.1`. A missing file is fine.
pub fn rotate(path: &Path, keep: usize) -> std::io::Result<()> {
    if keep < 2 {
        return std::fs::remove_file(path).or_else(not_found_ok);
    }
    std::fs::remove_file(rotated(path, keep - 1)).or_else(not_found_ok)?;
    for n in (1..keep - 1).rev() {
        std::fs::rename(rotated(path, n), rotated(path, n + 1)).or_else(not_found_ok)?;
    }
    std::fs::rename(path, rotated(path, 1)).or_else(not_found_ok)
}

fn not_found_ok(e: std::io::Error) -> std::io::Result<()> {
    if e.kind() == std::io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(e)
    }
}

/// The files of a log that exist, newest first.
pub fn files(path: &Path, keep: usize) -> Vec<PathBuf> {
    std::iter::once(path.to_path_buf())
        .chain((1..keep).map(|n| rotated(path, n)))
        .filter(|p| p.is_file())
        .collect()
}

/// A log sink in one file with rotation.
pub struct FileLog {
    path: PathBuf,
    level: LevelFilter,
    file: Mutex<Option<File>>,
}

impl FileLog {
    pub fn new(path: PathBuf, level: LevelFilter) -> Self {
        FileLog {
            path,
            level,
            file: Mutex::new(None),
        }
    }

    /// Append one line, with the rotation before it when the file would
    /// grow past [`MAX_BYTES`]. When the path names another file than the
    /// open one (`data.prepare` moved the home to the data disk, and the
    /// path now leads there), the path opens again, so the log that
    /// `log.fetch` sends has the lines after the move.
    pub fn write_line(&self, line: &str) -> std::io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let at_path = std::fs::metadata(&self.path).ok();
        if let Some(f) = guard.as_ref() {
            let same = match (f.metadata(), &at_path) {
                (Ok(open), Some(p)) => open.dev() == p.dev() && open.ino() == p.ino(),
                _ => false,
            };
            if !same {
                *guard = None;
            }
        }
        let len = at_path.map(|m| m.len()).unwrap_or(0);
        if len + line.len() as u64 + 1 > MAX_BYTES {
            *guard = None;
            rotate(&self.path, KEEP)?;
        }
        if guard.is_none() {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            *guard = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        if let Some(f) = guard.as_mut() {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
        }
        Ok(())
    }
}

impl Log for FileLog {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= self.level
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!(
            // not user text
            "{} {:<5} {}: {}",
            crate::waiting::format_rfc3339(now),
            r.level(),
            r.target().trim_start_matches("paddock::"),
            r.args()
        )
        .replace(['\n', '\r'], " ");
        let _ = self.write_line(&line);
    }

    fn flush(&self) {}
}

/// Install the file log as the `log` sink. `PADDOCK_LOG` names the level,
/// info by default.
pub fn init(path: PathBuf) -> Result<(), log::SetLoggerError> {
    let level = std::env::var("PADDOCK_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LevelFilter::Info);
    log::set_max_level(level);
    log::set_boxed_logger(Box::new(FileLog::new(path, level)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_keeps_five() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(NAME);
        let log = FileLog::new(path.clone(), LevelFilter::Info);
        let big = "x".repeat(1000);
        for round in 1..=8u32 {
            // Fill the file to the limit, then one more line rotates.
            // The file grows to just under the limit (the lines so far
            // are small), then one more line rotates.
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(&vec![b'y'; (MAX_BYTES - 500 - len) as usize])
                .unwrap();
            log.write_line(&format!("round {round} {big}")).unwrap(); // not user text
            let fs = files(&path, KEEP);
            assert!(fs.len() <= KEEP, "round {round}: {fs:?}");
            assert_eq!(fs.len(), (round as usize + 1).min(KEEP));
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.starts_with(&format!("round {round} ")), // not user text
                "the new file holds the new line"
            );
        }
        // The oldest rotated file is round 4 (.4): rounds 1 to 3 are gone.
        let head = |n: usize| {
            let t = std::fs::read_to_string(rotated(&path, n)).unwrap();
            t.lines()
                .next()
                .unwrap()
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            (head(1), head(4)),
            ("round 7".to_string(), "round 4".to_string())
        );
        assert!(!rotated(&path, 5).exists());
    }

    #[test]
    fn a_moved_home_opens_the_log_again() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let path = home.join("Logs").join(NAME);
        let log = FileLog::new(path.clone(), LevelFilter::Info);
        log.write_line("before the move").unwrap();
        // The home moves to the data disk: a copy, and the old one goes.
        let moved = t.path().join("home.moved");
        let new = t.path().join("data-home");
        std::fs::rename(&home, &moved).unwrap();
        std::fs::create_dir_all(new.join("Logs")).unwrap();
        std::fs::copy(moved.join("Logs").join(NAME), new.join("Logs").join(NAME)).unwrap();
        std::os::unix::fs::symlink(&new, &home).unwrap();
        log.write_line("after the move").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "before the move\nafter the move\n");
        // A removed file is made again.
        std::fs::remove_file(&path).unwrap();
        log.write_line("again").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "again\n");
    }

    #[test]
    fn rotate_without_files_is_fine() {
        let d = tempfile::tempdir().unwrap();
        rotate(&d.path().join("none.log"), KEEP).unwrap();
        assert!(files(&d.path().join("none.log"), KEEP).is_empty());
    }

    #[test]
    fn lines_carry_no_token_through_the_log_path() {
        use crate::macvm::proto::{Call, Event, FileHeader, FileKind, Request, Version};
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(NAME);
        let log = FileLog::new(path.clone(), LevelFilter::Trace);
        let token = "plantedtoken0123plantedtoken0123";
        let msgs = [
            Call {
                req: 1,
                request: Request::Token {
                    token: token.into(),
                    v: Version { major: 1, minor: 0 },
                    bench: false,
                    now_ms: None,
                },
            }
            .kind(),
            Call {
                req: 2,
                request: Request::DataPrepare {
                    password: token.into(),
                    data_id: "d".into(),
                },
            }
            .kind(),
            Call {
                req: 3,
                request: Request::ClockSet {
                    unix_ms: 1,
                    password: token.into(),
                },
            }
            .kind(),
            Event::AxValue {
                window: 1,
                text: token.into(),
            }
            .kind(),
            FileHeader {
                token: token.into(),
                kind: FileKind::Log,
                name: NAME.into(),
                size: 1,
            }
            .kind(),
        ];
        for m in msgs {
            log.write_line(&m).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 5);
        assert!(!text.contains(token), "{text}");
    }
}
