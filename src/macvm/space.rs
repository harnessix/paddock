//! The disk space rules of a profile (the full disk row of the failure
//! matrix): before a clone, before a save, and during the session. Pure
//! over a `free(path) -> u64` closure, so the tests pass numbers and the
//! shim passes `session::free_bytes` (which honors
//! `PADDOCK_MACVM_FAKE_FREE`).

use super::session::{Refusal, CLONE_NEEDS, SAVE_EXTRA};
use super::text;
use std::path::Path;

/// The free space of the volume of a path.
pub type Free<'a> = &'a dyn Fn(&Path) -> u64;

/// Free space at which the badge warns.
pub const WARN: u64 = 2 << 30;
/// Free space at which the shim tells the agent `disk.low` and the next
/// quit skips the save.
pub const LOW: u64 = 1 << 30;

/// The two levels of a session's disk watch, the lower space the greater.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warn2G,
    Low1G,
}

/// Before a clone of the golden: 8 GiB, or the refusal with both sizes.
pub fn check_clone(free: Free, dir: &Path) -> Result<u64, Refusal> {
    let have = free(dir);
    if have < CLONE_NEEDS {
        return Err(Refusal::Space {
            needs: CLONE_NEEDS,
            free: have,
        });
    }
    Ok(have)
}

/// Before a save: the RAM size plus 1 GiB, or the line that names the
/// cold boot.
pub fn check_save(free: Free, dir: &Path, ram: u64) -> Result<u64, String> {
    let have = free(dir);
    let needs = ram + SAVE_EXTRA;
    if have < needs {
        return Err(text::save_skipped(needs, have));
    }
    Ok(have)
}

/// During the session, every 10 s: the level of the free space now.
pub fn watch(free: Free, dir: &Path) -> Option<Level> {
    level(free(dir))
}

/// The level of a free size.
pub fn level(free: u64) -> Option<Level> {
    if free < LOW {
        Some(Level::Low1G)
    } else if free < WARN {
        Some(Level::Warn2G)
    } else {
        None
    }
}

/// The badge of a level.
pub fn badge(level: Level, free: u64) -> String {
    match level {
        Level::Warn2G | Level::Low1G => text::disk_warn(free),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn free_of(n: u64) -> impl Fn(&Path) -> u64 {
        move |_| n
    }

    #[test]
    fn clone_and_save_thresholds() {
        let d = Path::new("/p");
        let e = check_clone(&free_of(79 * GIB / 10), d).unwrap_err();
        assert_eq!(
            e,
            Refusal::Space {
                needs: 8 * GIB,
                free: 79 * GIB / 10
            }
        );
        let line = e.to_string();
        assert!(line.contains("8 GiB") && line.contains("7.9 GiB"), "{line}");
        assert_eq!(check_clone(&free_of(8 * GIB), d).unwrap(), 8 * GIB);
        let ram = 4 * GIB;
        let e = check_save(&free_of(ram + 9 * GIB / 10), d, ram).unwrap_err();
        assert!(e.contains("cold boot") && e.contains("5 GiB"), "{e}");
        assert!(check_save(&free_of(ram + GIB), d, ram).is_ok());
    }

    #[test]
    fn watch_levels() {
        let d = Path::new("/p");
        assert_eq!(watch(&free_of(21 * GIB / 10), d), None);
        assert_eq!(watch(&free_of(19 * GIB / 10), d), Some(Level::Warn2G));
        assert_eq!(watch(&free_of(11 * GIB / 10), d), Some(Level::Warn2G));
        assert_eq!(watch(&free_of(9 * GIB / 10), d), Some(Level::Low1G));
        assert!(badge(Level::Low1G, 9 * GIB / 10).contains("0.9 GiB"));
    }
}
