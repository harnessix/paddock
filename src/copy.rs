//! Agents work on a copy of the project (`--copy`).
//!
//! At the start of a run paddock copies the project into its own data
//! folder, and the agent works there. The original folder changes only
//! when the user applies changes (`paddock apply`). A copy shares its data
//! with the project where the file system can do that (`FICLONE` on btrfs,
//! XFS and bcachefs, `clonefileat` on APFS), else it is a plain copy up to
//! `[copy] max`.
//!
//! Layout, `<data>/copies/<store id>/`:
//!
//! - `store.json`: the id, and the device, inode and last path of the
//!   original folder (store.rs).
//! - `backups/<backup id>/`: `meta.json` and `files/`. A backup is the
//!   state at the start of a run, or the files that an apply or a restore
//!   replaced (backup.rs).
//! - `runs/<run id>/`: `run.json`, the working copy `tree/`, and `sync/`
//!   with the content that an apply wrote (run.rs).
//!
//! The original folder holds only `.paddock/pointer.json` with the store
//! id (pointer.rs). It names no path and holds no project data, so a copy
//! of the folder carries nothing along.
//!
//! The working copy is sandbox-writable. The host reads it through
//! `safefs` and never follows a symlink in it. An apply first puts the
//! content into `sync/`, which only the host writes, so the write into the
//! original is the content that paddock compared.
//!
//! Which files count: the rules of `snapshot.rs`. `.git/` and build output
//! (folders that git ignores, or `target`, `node_modules` and the like
//! outside git) are never part of the change list. They come along into
//! the working copy when that costs no bytes, and `.git/` always does, so
//! git works in the copy.

pub mod apply;
pub mod backup;
pub mod changes;
pub mod config;
pub mod diff;
pub mod disk;
pub mod end;
pub mod json;
pub mod pointer;
pub mod restore;
pub mod run;
pub mod start;
pub mod store;
pub mod tree;

pub use config::Config;
pub use tree::Entry;

/// The format number of `pointer.json`, `store.json`, `run.json` and the
/// backup records.
pub const FORMAT: u32 = 1;

/// What the command line asks for. The default asks for no copy: only
/// `paddock run`, `paddock shell` and `paddock pick` set it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ask {
    /// The command can use a copy at all.
    pub allowed: bool,
    /// `--copy` (true) or `--direct` (false).
    pub choice: Option<bool>,
    /// A run from a launcher entry.
    pub launcher: bool,
    /// `paddock explain`: say what a run does, and make nothing.
    pub plan_only: bool,
}

/// What the spec records of a run on a copy.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CopyOf {
    /// The folder of the user.
    pub original: std::path::PathBuf,
    /// The working copy on the host.
    pub tree: std::path::PathBuf,
    /// The sandbox sees the copy at the path of the original.
    pub in_place: bool,
    pub store: String,
    /// Why the run works on a copy, in words.
    pub why: String,
}

#[cfg(test)]
mod tests;
