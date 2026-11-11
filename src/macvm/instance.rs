//! The instances of one app profile: `paddock app run NAME --instance N`
//! runs another profile of the same app, with its own disks, data,
//! keychain and firewall rules, cloned from the golden like the first.
//!
//! Instance 1 is the folder `profiles.noindex/NAME/default`, as before.
//! Instance N is the folder `profiles.noindex/NAME/instance-N`. The key
//! of an instance (`NAME`, or `NAME--N`) names its session record, its
//! control socket, its running id and its Dock face. Plain Rust: the
//! tests run on every system.

use super::host::DEFAULT_HOME;
use super::layout::Layout;
use super::text;

/// The first instance: the folder `default`.
pub const FIRST: u32 = 1;
/// The highest instance number.
pub const MAX: u32 = 99;
/// The folder prefix of instance 2 and later.
pub const PREFIX: &str = "instance-";
/// The separator of an instance key: `sample-notes--2`.
pub const KEY_SEP: &str = "--";

/// The folder of instance `n` under `profiles.noindex/NAME/`.
pub fn folder(n: u32) -> String {
    if n <= FIRST {
        DEFAULT_HOME.to_string()
    } else {
        format!("{PREFIX}{n}") // not user text
    }
}

/// The instance of a profile folder name: `default` is 1, `instance-N`
/// is N. Any other name is not an instance.
pub fn number(folder: &str) -> Option<u32> {
    if folder == DEFAULT_HOME {
        return Some(FIRST);
    }
    let n: u32 = folder.strip_prefix(PREFIX)?.parse().ok()?;
    (n > FIRST && n <= MAX && folder == self::folder(n)).then_some(n)
}

/// The key of an instance: the app name for instance 1, `NAME--N` after.
pub fn key(app: &str, n: u32) -> String {
    if n <= FIRST {
        app.to_string()
    } else {
        format!("{app}{KEY_SEP}{n}") // not user text
    }
}

/// The name of an instance for the user: `Sample Notes`, `Sample Notes 2`.
pub fn label(name: &str, n: u32) -> String {
    if n <= FIRST {
        name.to_string()
    } else {
        format!("{name} {n}") // not user text
    }
}

/// The name of an instance in a command: `sample-notes`, `sample-notes --instance 2`.
pub fn arg(app: &str, n: u32) -> String {
    if n <= FIRST {
        app.to_string()
    } else {
        format!("{app} --instance {n}") // not user text
    }
}

/// The instance of a profile folder: its folder name, 1 for any other.
pub fn of_profile(dir: &std::path::Path) -> u32 {
    dir.file_name()
        .and_then(|n| n.to_str())
        .and_then(number)
        .unwrap_or(FIRST)
}

/// The value of `--instance`: a number from 1 to [`MAX`].
pub fn parse(s: &str) -> Result<u32, String> {
    match s.trim().parse::<u32>() {
        Ok(n) if (FIRST..=MAX).contains(&n) => Ok(n),
        _ => Err(text::instance_invalid(s, MAX)),
    }
}

/// The instances of an app that have a profile folder, sorted.
pub fn existing(layout: &Layout, app: &str) -> Vec<u32> {
    let mut v: Vec<u32> = layout
        .profile_dirs()
        .into_iter()
        .filter(|(a, _, _)| a == app)
        .filter_map(|(_, home, _)| number(&home))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The number that `--new` picks: the lowest that has no profile folder
/// yet. None when all [`MAX`] exist.
pub fn next_free(existing: &[u32]) -> Option<u32> {
    (FIRST..=MAX).find(|n| !existing.contains(n))
}

/// The instance of a run: `--instance N`, `--new`, or the first.
pub fn choose(asked: Option<u32>, new: bool, existing: &[u32]) -> Result<u32, String> {
    match (asked, new) {
        (Some(_), true) => Err(text::instance_and_new()),
        (Some(n), false) => Ok(n),
        (None, true) => next_free(existing).ok_or_else(|| text::instances_full(MAX)),
        (None, false) => Ok(FIRST),
    }
}

#[cfg(test)]
#[path = "instance_tests.rs"]
mod tests;
