//! The texts of the files and the clipboard of a session: the reason of a
//! refused share, the secrets warning, the drop and clipboard badges, the
//! refusals of the `guest-files` port and the downloads folder line.

use super::{fit, gib, shown};
use std::path::Path;
use std::time::Duration;

/// Why a folder is never shared with the guest (checklist item 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareWhy {
    /// `/`.
    Root,
    /// The home folder of the user.
    Home,
    /// A parent of the home folder.
    ContainsHome,
    /// `/Users`.
    AllHomes,
    /// `/Volumes`.
    Volumes,
    /// `/private`, `/tmp`, `/var`, `/etc`.
    SystemFiles,
    /// `/Library`, `/System`, `/Applications`.
    SystemApps,
    /// `~/.ssh`, `~/Library`, the keyring and cloud credential folders.
    Protected,
    /// Not an absolute path.
    Relative,
}

/// The reason of a refused share, the second half of `share_refused`.
pub fn share_reason(why: ShareWhy) -> String {
    match why {
        ShareWhy::Root => "it is the whole disk",
        ShareWhy::Home => "it is your home folder, with every file of yours",
        ShareWhy::ContainsHome => "it contains your home folder",
        ShareWhy::AllHomes => "it holds the home folders of every user of this Mac",
        ShareWhy::Volumes => "it holds every disk of this Mac, and the shares of the guest",
        ShareWhy::SystemFiles => "it is a system folder",
        ShareWhy::SystemApps => "it holds the system and its apps",
        ShareWhy::Protected => "it holds your keys, logins or app data",
        ShareWhy::Relative => "it is not an absolute path",
    }
    .into()
}

/// The warning of a secrets file in a shared folder when
/// `unprotected_workspaces` is on.
pub fn env_file_visible(name: &str, path: &Path) -> String {
    format!(
        "unprotected_workspaces is on for {name}: the guest can read {}",
        shown(path)
    )
}

/// `paddock app files NAME`: the downloads folder of the profile.
pub fn downloads_folder(name: &str, path: &Path) -> String {
    format!(
        "the downloads of {name} are in {}. Every download of the app in the guest lands there",
        shown(path)
    )
}

// ---- Drops --------------------------------------------------------------

/// A drop above the 2 GiB cap, as a badge.
pub fn drop_too_large(size: u64) -> String {
    format!("drop too large: {} (max 2 GiB)", gib(size))
}

pub fn drop_failed() -> String {
    "drop failed".into()
}

pub fn dropped_as_open() -> String {
    "dropped as open".into()
}

/// The event line of a staged drop.
pub fn drop_staged(count: usize, dir: &Path) -> String {
    format!("{count} item(s) dropped into {}", shown(dir))
}

// ---- The clipboard ------------------------------------------------------

pub fn image_too_large() -> String {
    "image too large for the clipboard (max 16 MiB)".into()
}

/// A clipboard item set above 16 MiB that is not an image, as a badge.
pub fn clipboard_too_large(size: u64) -> String {
    format!("clipboard too large: {} (max 16 MiB)", fit(&gib(size), 13))
}

// ---- The guest-files port (checklist item 14) ---------------------------

/// Why a `guest-files` transfer was refused or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferWhy {
    TooLarge { size: u64 },
    TooMany,
    Idle(Duration),
    BadToken,
    BadName,
}

/// The log line of a refused transfer.
pub fn transfer_refused(name: &str, why: TransferWhy) -> String {
    let why = match why {
        TransferWhy::TooLarge { size } => {
            format!("{} is more than the 16 MiB of one transfer", gib(size))
        }
        TransferWhy::TooMany => "more than 64 transfers in a minute".to_string(),
        TransferWhy::Idle(d) => format!("no byte for {} s", d.as_secs()),
        TransferWhy::BadToken => "wrong token".to_string(),
        TransferWhy::BadName => "the name is not a plain file name".to_string(),
    };
    format!("the guest-files transfer {name} was refused: {why}")
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
