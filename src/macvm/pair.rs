//! The disk pair rule: a formatted data disk mounts only under the system
//! disk that made it, so the host reads the first sectors of `data.img` to
//! tell a formatted disk from a blank one, and a start that finds a
//! formatted data disk without its system disk is refused. `session`
//! re-exports everything here.

use super::session::{Now, Stamp};
use super::text;
use std::fmt;
use std::path::{Path, PathBuf};

/// The start cannot happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The volume has too little space for the clone.
    Space { needs: u64, free: u64 },
    /// The data disk is formatted and its system disk is gone: nothing can
    /// read it. `backup` is the newest backup of the pair, when there is
    /// one.
    DataLocked {
        profile: PathBuf,
        backup: Option<PathBuf>,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Space { needs, free } => f.write_str(&text::no_space(
                *needs,
                *free,
                "a cold start of the profile",
            )),
            Refusal::DataLocked { profile, backup } => {
                f.write_str(&text::data_locked(profile, backup.as_deref()))
            }
        }
    }
}

impl std::error::Error for Refusal {}

/// Whether the guest partitioned the data disk: the protective MBR at byte
/// 510 or the GPT signature at byte 512. A never prepared disk is a sparse
/// file of zeros. A disk that cannot be read counts as formatted: the safe
/// side.
pub fn data_formatted(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return std::fs::symlink_metadata(path).is_ok();
    };
    let mut head = [0u8; 1024];
    if f.seek(SeekFrom::Start(0)).is_err() {
        return true;
    }
    let mut got = 0;
    while got < head.len() {
        match f.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => return true,
        }
    }
    got >= 520 && (head[510..512] == [0x55, 0xAA] || &head[512..520] == b"EFI PART")
}

/// The line that says the golden moved on while this profile keeps its own
/// system disk, or None when they agree.
pub fn golden_note(name: &str, stamp: &Stamp, now: &Now) -> Option<String> {
    (stamp.golden != now.golden && !stamp.golden.is_empty())
        .then(|| text::golden_newer(name, &stamp.golden, &now.golden))
}
