//! The texts of a failed file or command: `cannot read X`, `cannot move
//! X to Y`. Every module of the route uses them as the context of an
//! error, so the user sees the path and the verb in one line.

use super::shown;
use std::path::Path;

pub fn cannot_read(path: &Path) -> String {
    format!("cannot read {}", shown(path))
}

pub fn cannot_write(path: &Path) -> String {
    format!("cannot write {}", shown(path))
}

pub fn cannot_make(path: &Path) -> String {
    format!("cannot make {}", shown(path))
}

pub fn cannot_open(path: &Path) -> String {
    format!("cannot open {}", shown(path))
}

pub fn cannot_remove(path: &Path) -> String {
    format!("cannot remove {}", shown(path))
}

pub fn cannot_copy(from: &Path, to: &Path) -> String {
    format!("cannot copy {} to {}", shown(from), shown(to))
}

pub fn cannot_move(from: &Path, to: &Path) -> String {
    format!("cannot move {} to {}", shown(from), shown(to))
}

/// A `.partial` file or folder did not take its final name.
pub fn cannot_commit(path: &Path) -> String {
    format!("cannot commit {}", shown(path))
}

pub fn cannot_size(path: &Path) -> String {
    format!("cannot size {}", shown(path))
}

/// The `uchg` flag or the mode of a sealed image did not come off.
pub fn cannot_unseal(path: &Path) -> String {
    format!("cannot unseal {}", shown(path))
}

/// A file of the golden or of a profile does not parse.
pub fn damaged(path: &Path) -> String {
    format!("{} is damaged", shown(path))
}

/// A command of the host did not start.
pub fn cannot_run(command: &str) -> String {
    format!("cannot run {command}")
}

/// The free space of a volume could not be read.
pub fn free_space_unreadable(path: &Path) -> String {
    format!("cannot read the free space of {}", shown(path))
}

/// No parent of the path exists, so there is no volume to measure.
pub fn no_folder_exists(path: &Path) -> String {
    format!("no folder of {} exists", shown(path))
}

/// A path that the framework cannot take.
pub fn not_utf8(path: &Path) -> String {
    format!("the path {} is not UTF-8", shown(path))
}

/// `commit` got a name without the `.partial` suffix: a bug of the caller.
pub fn not_partial_name(path: &Path) -> String {
    format!("{} is not a .partial name", shown(path))
}

/// The random source of the password did not open or read.
pub fn urandom_failed(what: &str) -> String {
    format!("cannot {what} /dev/urandom")
}

#[cfg(test)]
#[path = "io_tests.rs"]
mod tests;
