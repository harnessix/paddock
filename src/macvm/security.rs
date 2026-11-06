//! The security checklist of the macOS guest: the hooks that its tests need
//! and that no other module owns, and the tests themselves, one per row of
//! the table (`security_tests.rs`, `s01_` to `s21_`). The Mac side of items
//! 5, 10, 11 and 16 is `packaging/macos/checks/security.sh`.
//!
//! Item 5 and item 16 feed the `macvm` block of `paddock doctor`
//! (`doctor::macvm`): the modes of each profile folder and the mode of the
//! slots folder. Item 13 is the gate of the control port and the token
//! line of a forwarded port, which the macOS listener (`vz_mac_agent`)
//! calls. Everything here is pure and runs on Linux.

use super::session::Profile;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The longest token line of a guest connection on a forwarded port.
pub const TOKEN_LINE_MAX: usize = 128;

/// The mode of the slots folder: sticky and writable by every user, so
/// that the two slots are shared by every user of the Mac.
pub const SLOTS_DIR_MODE: u32 = 0o1777;

/// Item 13: one control connection per session. The listener calls
/// [`ControlGate::open`] for each connection on the control port and
/// refuses the connection when it says no; the reader of the open
/// connection calls [`ControlGate::close`] when the connection ends.
#[derive(Debug, Clone, Default)]
pub struct ControlGate(Arc<AtomicBool>);

impl ControlGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the control connection. False when one is open already: the
    /// second connection is refused and the guest agent retries later.
    pub fn open(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// The open connection ended: the next one may come.
    pub fn close(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub fn is_open(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Item 13: a guest connection on a forwarded port starts with the
/// session token on its own line. True when the first line is the token.
/// A longer line, a closed connection, a read error or another text is
/// false, and the caller closes the connection without a byte in reply.
pub fn token_line_ok(r: &mut impl Read, token: &str) -> bool {
    matches!(super::proto::read_raw_line(r, TOKEN_LINE_MAX), Ok(Some(line)) if line == token)
}

/// Item 5: the files of a profile whose mode is not the one of the table
/// (the folder 0700, `state.vzs`, `data.img` and every backup file 0600,
/// every backup folder 0700), as `<name> <mode> (expected <mode>)`. A
/// file that is not there is not an issue. Empty when every mode is
/// right.
pub fn mode_issues(profile: &Profile) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |path: &Path, want: u32| {
        let Ok(m) = std::fs::symlink_metadata(path) else {
            return;
        };
        let mode = m.permissions().mode() & 0o7777;
        if mode != want {
            let name = path
                .strip_prefix(&profile.dir)
                .unwrap_or(path)
                .display()
                .to_string();
            out.push(format!("{name} {mode:04o} (expected {want:04o})")); // not user text
        }
    };
    check(&profile.dir, 0o700);
    check(&profile.state(), 0o600);
    check(&profile.data(), 0o600);
    if let Ok(backups) = std::fs::read_dir(profile.backups()) {
        let mut dirs: Vec<_> = backups.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            check(&dir, 0o700);
            if let Ok(files) = std::fs::read_dir(&dir) {
                let mut files: Vec<_> = files.flatten().map(|e| e.path()).collect();
                files.sort();
                for f in files {
                    if f.is_file() {
                        check(&f, 0o600);
                    }
                }
            }
        }
    }
    out
}

/// Item 16: the mode bits of the slots folder, `None` when it is not a
/// folder yet (the first start makes it with [`SLOTS_DIR_MODE`]).
pub fn slots_dir_mode(dir: &Path) -> Option<u32> {
    let m = std::fs::symlink_metadata(dir).ok()?;
    m.is_dir().then(|| m.permissions().mode() & 0o7777)
}

/// The mode as `ls -l` prints it for a folder: `drwxrwxrwt` for 1777.
pub fn ls_mode(mode: u32) -> String {
    let mut s = String::from("d");
    for (shift, special, letter) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let x = bits & 1 != 0;
        s.push(match (x, mode & special != 0) {
            (true, true) => letter,
            (false, true) => letter.to_ascii_uppercase(),
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    s
}

#[cfg(test)]
#[path = "security_tests.rs"]
mod tests;
