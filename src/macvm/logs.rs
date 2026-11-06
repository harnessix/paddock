//! The logs of a profile and their rotation.
//!
//! | Log | File | Rotation |
//! | --- | --- | --- |
//! | shim | `logs/shim.log` | 5 files of 1 MiB: the parent rotates before it opens the file for a new session, the shim every 10 s with [`reopen_if_large`] |
//! | console | `logs/console.log` | 5 files of 1 MiB, rotated by the shim before the boot |
//! | agent | `logs/agent.log` | fetched from the guest at the quit; the file of the last session becomes `.1` |
//! | events | `logs/events.jsonl` | 2 MiB cap in `events.rs` |
//!
//! The rotation decision is a pure function with a test. The shim writes
//! its log through the stderr that the parent gave it, so the rotation
//! in the shim renames the file and `dup2`s a new one onto the fds.

use super::session::Profile;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// How a log rotates: the size past which it rotates and the files kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rotation {
    pub max: u64,
    pub keep: usize,
}

/// 5 files of 1 MiB: the shim log, the console log and the agent log.
pub const FIVE_BY_MIB: Rotation = Rotation {
    max: 1 << 20,
    keep: 5,
};

/// The logs of the profile that `paddock app log` and the report read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Shim,
    Events,
    Console,
    Agent,
    Boundary,
}

impl Which {
    pub const ALL: [Which; 5] = [
        Which::Shim,
        Which::Events,
        Which::Console,
        Which::Agent,
        Which::Boundary,
    ];

    pub fn path(self, profile: &Profile) -> PathBuf {
        match self {
            Which::Shim => profile.shim_log(),
            Which::Events => profile.events_log(),
            Which::Console => profile.console_log(),
            Which::Agent => profile.agent_log(),
            Which::Boundary => profile.boundary_log(),
        }
    }
}

/// Whether a file of `len` bytes rotates before `add` more bytes go in.
/// An empty file never rotates, so one line longer than `max` still
/// lands somewhere.
pub fn needs_rotation(len: u64, add: u64, r: Rotation) -> bool {
    len > 0 && len + add > r.max
}

/// The rotated name of a log: `shim.log.1` is the newest rotated file.
pub fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{n}"));
    path.with_file_name(name)
}

/// The rotated files that exist, newest first.
pub fn rotated(path: &Path, r: Rotation) -> Vec<PathBuf> {
    (1..r.keep)
        .map(|n| numbered(path, n))
        .filter(|p| p.exists())
        .collect()
}

/// Shift the files: `.keep-1` goes, each `.n` becomes `.n+1`, the file
/// becomes `.1`. A missing file is fine. The size is not looked at: the
/// caller decided.
pub fn shift(path: &Path, r: Rotation) {
    if r.keep == 0 {
        return;
    }
    let _ = std::fs::remove_file(numbered(path, r.keep - 1));
    for n in (1..r.keep - 1).rev() {
        let _ = std::fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    if r.keep > 1 {
        let _ = std::fs::rename(path, numbered(path, 1));
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Rotate a log when it is longer than `max`: the file becomes `.1`, the
/// older files move up, `keep` files stay. For the console log and the
/// shim log (5 files of 1 MiB).
pub fn rotate(path: &Path, keep: usize, max: u64) {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len <= max || keep == 0 {
        return;
    }
    shift(path, Rotation { max, keep });
}

/// Open the shim log of a profile for a new session, in append mode,
/// after a rotation when the file is over its size. The `logs/` folder
/// is made when missing.
pub fn open_shim_log(profile: &Profile) -> io::Result<File> {
    std::fs::create_dir_all(profile.logs())?;
    let path = profile.shim_log();
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if needs_rotation(len, 1, FIVE_BY_MIB) {
        shift(&path, FIVE_BY_MIB);
    }
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
}

/// The shim side of the shim log rotation: when `path` is over the size,
/// rotate it, open a new file and put it onto each of `fds` (1 and 2 in
/// the shim). `Ok(true)` when it rotated.
pub fn reopen_if_large(path: &Path, r: Rotation, fds: &[i32]) -> io::Result<bool> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if !needs_rotation(len, 1, r) {
        return Ok(false);
    }
    shift(path, r);
    let f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    for fd in fds {
        // SAFETY: dup2 onto an fd that this process owns; the old file
        // stays open in the parent that gave it.
        if unsafe { libc::dup2(f.as_raw_fd(), *fd) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(true)
}

/// The agent log of the guest arrived through the files port: move it to
/// `logs/agent.log`. The file of the last session becomes `agent.log.1`,
/// five stay. Returns the final path.
pub fn place_agent_log(profile: &Profile, fetched: &Path) -> io::Result<PathBuf> {
    let to = profile.agent_log();
    std::fs::create_dir_all(profile.logs())?;
    if to.exists() {
        shift(&to, FIVE_BY_MIB);
    }
    match std::fs::rename(fetched, &to) {
        Ok(()) => {}
        Err(_) => {
            std::fs::copy(fetched, &to)?;
            let _ = std::fs::remove_file(fetched);
        }
    }
    let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o600));
    Ok(to)
}

use std::os::unix::fs::PermissionsExt;

/// Hand the items of `rx` to `take` for at most `wait`, until `take` says
/// that what was awaited came (true). False when the time ran out or the
/// channel closed first. The shim waits so for the agent log at its exit.
pub fn wait_for<T>(
    rx: &std::sync::mpsc::Receiver<T>,
    wait: std::time::Duration,
    mut take: impl FnMut(T) -> bool,
) -> bool {
    let end = std::time::Instant::now() + wait;
    loop {
        let left = end.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(item) => {
                if take(item) {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }
}

/// The last `n` lines of a text file. `None` when the file is missing.
/// Bytes that are not UTF-8 are replaced, never an error.
pub fn tail(path: &Path, n: usize) -> Option<Vec<String>> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    Some(
        lines[lines.len().saturating_sub(n)..]
            .iter()
            .map(|l| l.to_string())
            .collect(),
    )
}

/// The size of each log of the profile, for `paddock app status`.
pub fn sizes(profile: &Profile) -> Vec<(Which, u64)> {
    Which::ALL
        .iter()
        .map(|w| {
            (
                *w,
                std::fs::metadata(w.path(profile))
                    .map(|m| m.len())
                    .unwrap_or(0),
            )
        })
        .collect()
}

/// `paddock app log NAME [--events | --shim | --guest | --console]
/// [--follow]`. Without a flag: the connection log of the profile
/// (`logs/connections.jsonl`), rendered one decision per line, or the
/// boundary log while there is none yet. `--follow` polls the file until
/// the session is gone.
pub fn cmd(
    env: &crate::env::HostEnv,
    name: &str,
    which: Option<Which>,
    follow: bool,
) -> anyhow::Result<i32> {
    use super::host::{self, DEFAULT_HOME};
    use super::layout::Layout;
    use super::text;
    if !host::valid_name(name) {
        anyhow::bail!(text::bad_app_name(name));
    }
    let layout = Layout::new(env);
    let profile = layout.profile(name, DEFAULT_HOME);
    if !profile.dir.is_dir() {
        println!("{}", text::status_no_profile(name));
        return Ok(1);
    }
    let alive = || host::running(&layout, name, DEFAULT_HOME).is_some();
    let poll = std::time::Duration::from_millis(250);
    let path = match which {
        Some(w) => w.path(&profile),
        None => {
            let connections = profile.connections_log();
            if connections.exists() {
                connections
            } else {
                println!("{}", text::connections_fallback(&connections));
                profile.boundary_log()
            }
        }
    };
    if which == Some(Which::Events) {
        let mut printed = 0u64;
        let events = super::events::read(&path);
        for e in &events {
            println!("{}", e.render());
        }
        if events.is_empty() && !path.exists() && !follow {
            println!("{}", text::log_missing(&path));
        }
        if !follow {
            return Ok(0);
        }
        printed += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        super::events::follow(
            &path,
            printed,
            poll,
            &mut |e| println!("{}", e.render()),
            &|| !alive(),
        );
        println!("{}", text::log_follow_ended(name));
        return Ok(0);
    }
    // The connection log is one JSON object per line: show it as lines
    // that read.
    let rendered = which.is_none() && path == profile.connections_log();
    let show = |bytes: &[u8]| {
        if rendered {
            for l in super::connlog::render_lines(bytes) {
                println!("{l}");
            }
        } else {
            print!("{}", String::from_utf8_lossy(bytes));
        }
    };
    let mut at = match std::fs::read(&path) {
        Ok(bytes) => {
            show(&bytes);
            bytes.len() as u64
        }
        Err(_) => {
            if !follow {
                println!("{}", text::log_missing(&path));
                return Ok(0);
            }
            0
        }
    };
    if !follow {
        return Ok(0);
    }
    loop {
        let bytes = std::fs::read(&path).unwrap_or_default();
        if (bytes.len() as u64) < at {
            at = 0;
        }
        if (bytes.len() as u64) > at {
            show(&bytes[at as usize..]);
            at = bytes.len() as u64;
        }
        if !alive() {
            println!("{}", text::log_follow_ended(name));
            return Ok(0);
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_command_prints_each_log() {
        let t = tempfile::tempdir().unwrap();
        let env = crate::env::HostEnv::from_parts(
            t.path().join("home"),
            "u".into(),
            501,
            20,
            [(
                "XDG_DATA_HOME".to_string(),
                t.path().join("data").display().to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert!(cmd(&env, "../x", None, false).is_err());
        assert_eq!(cmd(&env, "vscode", None, false).unwrap(), 1);
        let layout = super::super::layout::Layout::new(&env);
        let p = layout.profile("vscode", "default");
        std::fs::create_dir_all(p.logs()).unwrap();
        for w in [
            None,
            Some(Which::Events),
            Some(Which::Shim),
            Some(Which::Agent),
        ] {
            assert_eq!(cmd(&env, "vscode", w, false).unwrap(), 0);
        }
        std::fs::write(p.shim_log(), "line\n").unwrap();
        super::super::events::Events::open(&p).record("start", "fresh", &[]);
        for w in [
            Some(Which::Events),
            Some(Which::Shim),
            Some(Which::Boundary),
        ] {
            assert_eq!(cmd(&env, "vscode", w, false).unwrap(), 0);
        }
        // --follow with no session ends at once.
        assert_eq!(cmd(&env, "vscode", Some(Which::Events), true).unwrap(), 0);
        assert_eq!(cmd(&env, "vscode", Some(Which::Shim), true).unwrap(), 0);
        assert_eq!(cmd(&env, "vscode", Some(Which::Console), true).unwrap(), 0);
    }

    #[test]
    fn rotation_decision() {
        let r = FIVE_BY_MIB;
        // Under the size: no rotation, also with the line that fills it.
        assert!(!needs_rotation(0, 100, r));
        assert!(!needs_rotation((1 << 20) - 100, 100, r));
        // One byte over the size with the next line: rotation.
        assert!(needs_rotation((1 << 20) - 99, 100, r));
        assert!(needs_rotation(1 << 20, 1, r));
        // An empty file never rotates, however long the line.
        assert!(!needs_rotation(0, 5 << 20, r));

        // The sixth file: five stay, the oldest goes.
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("shim.log");
        for i in 0..6 {
            std::fs::write(&log, format!("session {i}")).unwrap(); // not user text
            shift(&log, r);
        }
        assert!(!log.exists());
        let kept = rotated(&log, r);
        assert_eq!(kept.len(), 4, "{kept:?}");
        assert_eq!(
            std::fs::read_to_string(numbered(&log, 1)).unwrap(),
            "session 5"
        );
        assert_eq!(
            std::fs::read_to_string(numbered(&log, 4)).unwrap(),
            "session 2"
        );
        assert!(!numbered(&log, 5).exists(), "the fifth slot stays free");
        // `rotate` with the size: a short file stays.
        std::fs::write(&log, "short").unwrap();
        rotate(&log, 5, 100);
        assert!(log.exists());
        rotate(&log, 5, 2);
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(numbered(&log, 1)).unwrap(), "short");
    }

    #[test]
    fn shim_log_rotates_at_open_and_in_the_shim() {
        use std::io::Write;
        let t = tempfile::tempdir().unwrap();
        let p = Profile::new(t.path().join("p"));
        // The parent: a log over the size rotates before the open.
        {
            let mut f = open_shim_log(&p).unwrap();
            f.write_all(&vec![b'x'; (1 << 20) + 5]).unwrap();
        }
        let f = open_shim_log(&p).unwrap();
        drop(f);
        assert_eq!(std::fs::metadata(p.shim_log()).unwrap().len(), 0);
        assert_eq!(
            std::fs::metadata(numbered(&p.shim_log(), 1)).unwrap().len(),
            (1 << 20) + 5
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(p.shim_log())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        // The shim: a file that it writes through a fixed fd. The fd is a
        // scratch file of this test, in place of stderr.
        let scratch = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(t.path().join("scratch"))
            .unwrap();
        let fd = scratch.as_raw_fd();
        let r = Rotation { max: 100, keep: 3 };
        assert!(!reopen_if_large(&p.shim_log(), r, &[fd]).unwrap());
        std::fs::write(p.shim_log(), vec![b'y'; 150]).unwrap();
        assert!(reopen_if_large(&p.shim_log(), r, &[fd]).unwrap());
        // The fd now writes into the new shim.log.
        let mut through = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(libc::dup(fd)) };
        through.write_all(b"after\n").unwrap();
        drop(through);
        assert_eq!(std::fs::read_to_string(p.shim_log()).unwrap(), "after\n");
        assert_eq!(
            std::fs::metadata(numbered(&p.shim_log(), 1)).unwrap().len(),
            150
        );
        drop(scratch);
    }

    #[test]
    fn agent_log_is_placed_and_the_old_one_rotates() {
        let t = tempfile::tempdir().unwrap();
        let p = Profile::new(t.path().join("p"));
        let guest = p.logs().join("guest");
        std::fs::create_dir_all(&guest).unwrap();
        let fetched = guest.join("paddock-guest.log");
        std::fs::write(&fetched, "first\n").unwrap();
        assert_eq!(place_agent_log(&p, &fetched).unwrap(), p.agent_log());
        assert!(!fetched.exists());
        std::fs::write(&fetched, "second\n").unwrap();
        place_agent_log(&p, &fetched).unwrap();
        assert_eq!(std::fs::read_to_string(p.agent_log()).unwrap(), "second\n");
        assert_eq!(
            std::fs::read_to_string(numbered(&p.agent_log(), 1)).unwrap(),
            "first\n"
        );
        assert_eq!(tail(&p.agent_log(), 5).unwrap(), vec!["second"]);
        assert!(tail(&p.logs().join("none.log"), 5).is_none());
        let sizes = sizes(&p);
        assert_eq!(sizes.len(), 5);
        assert!(sizes.iter().any(|(w, n)| *w == Which::Agent && *n == 7));
        assert_eq!(Which::Console.path(&p), p.console_log());
    }

    #[test]
    fn wait_for_hands_on_the_items_until_the_awaited_one() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let (tx, rx) = mpsc::channel();
        for i in [1, 2, 7, 9] {
            tx.send(i).unwrap();
        }
        let mut seen = Vec::new();
        assert!(wait_for(&rx, Duration::from_secs(5), |i| {
            seen.push(i);
            i == 7
        }));
        assert_eq!(seen, [1, 2, 7], "the rest stays in the channel");
        // Nothing comes: false after the wait, not before.
        let t = Instant::now();
        assert!(!wait_for(&rx, Duration::from_millis(200), |i| i == 100));
        assert!(t.elapsed() >= Duration::from_millis(200));
        // An item from a thread in time.
        let late = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            late.send(5).unwrap();
        });
        assert!(wait_for(&rx, Duration::from_secs(5), |i| i == 5));
        drop(tx);
        assert!(!wait_for(&rx, Duration::from_secs(5), |_| true), "closed");
    }
}
