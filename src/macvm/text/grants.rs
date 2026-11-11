//! The texts of the live grants: `paddock app share`, `paddock app
//! network`, `paddock app clipboard`, the grants file of the profile, the
//! `grant` lines of the control socket and the open files count of the
//! guest.

use super::shown;
use crate::macvm::grants::{Applies, Clipboard, What};
use crate::macvm::grants_cli::{Listing, Outcome};
use std::fmt::Display;
use std::path::Path;

/// The grants file or its folder cannot be read or made.
pub fn grants_unreadable(path: &Path) -> String {
    format!("paddock cannot read or make {}", shown(path))
}

/// The grants file does not parse.
pub fn grants_damaged(path: &Path, e: &dyn Display) -> String {
    format!(
        "the grants file {} is damaged ({e}). paddock does not change it. Move it away to start again with no extra grants",
        shown(path)
    )
}

/// The grants file is of a newer paddock.
pub fn grants_newer(path: &Path, v: u32) -> String {
    format!(
        "the grants file {} has version {v}, from a newer paddock. Update paddock: brew upgrade paddock",
        shown(path)
    )
}

/// A start with a grants file that does not load.
pub fn grants_skipped(e: &str) -> String {
    format!("{e}. This start uses none of its folders or network mode; both clipboard directions are off")
}

/// The new grants file cannot be written.
pub fn grants_unwritable(path: &Path) -> String {
    format!(
        "paddock cannot write {}. The grants stay as they were",
        shown(path)
    )
}

/// `paddock app share NAME add` of a refused folder.
pub fn grant_share_refused(app: &str, shown_path: &str, reason: &str) -> String {
    format!("paddock does not share {shown_path} with {app}: {reason}. Choose a project folder")
}

/// The secrets check refused a folder: how to share it anyway.
pub fn grant_share_anyway(app: &str, shown_path: &str) -> String {
    format!(
        "To share {shown_path} anyway, with these files visible to the app: paddock app share {app} add '{shown_path}' --unprotected"
    )
}

/// A mode word that is not `ro` or `rw`.
pub fn grant_bad_mode(w: &str) -> String {
    format!("the folder mode {w:?} is not ro or rw")
}

/// A switch word that is not `on` or `off`.
pub fn grant_bad_switch(w: &str) -> String {
    format!("{w:?} is not on or off")
}

/// A `grant` line that does not parse.
pub fn grant_bad_line(rest: &str) -> String {
    format!("unknown grant request {rest:?}")
}

/// `paddock app network NAME` with another word.
pub fn grant_bad_network(w: &str) -> String {
    format!("the network mode {w:?} is not one of ask, allowlist or none")
}

/// The folder is shared already.
pub fn grant_already_shared(app: &str, path: &Path, read_only: bool) -> String {
    let (now, other) = if read_only {
        ("read only", "rw")
    } else {
        ("read and write", "ro")
    };
    format!(
        "{} is shared with {app} already, {now}. To change the mode: paddock app share {app} mode '{}' {other}",
        shown(path),
        shown(path)
    )
}

/// The folder is not shared.
pub fn grant_not_shared(app: &str, path: &Path) -> String {
    format!(
        "{} is not shared with {app}. paddock app share {app} list shows the folders",
        shown(path)
    )
}

/// `lsof` did not start or failed.
pub fn open_files_failed(program: &str, e: &str) -> String {
    format!("cannot count the open files with {program}: {e}")
}

/// `lsof` did not end in time.
pub fn open_files_timeout(program: &str) -> String {
    format!("{program} did not end in time: no count of the open files")
}

/// A grants command for a name that is no app.
pub fn grant_no_profile(name: &str) -> String {
    format!("{name} is not an app of paddock's macOS guest. paddock app list shows the apps")
}

/// The control socket of the running session does not answer.
pub fn grant_session_unreachable(control: &Path) -> String {
    format!(
        "the session of the app does not answer on {}. Nothing changed. If the app still runs, stop it and start it again",
        shown(control)
    )
}

/// The session closed without an answer.
pub fn grant_no_answer(control: &Path) -> String {
    format!(
        "the session on {} gave no answer in time. Nothing was written into the profile. paddock app share NAME list shows what the app sees now",
        shown(control)
    )
}

/// An answer line of the session that does not parse.
pub fn grant_bad_answer(line: &str) -> String {
    format!("the session sent an unknown answer: {line}")
}

fn mode_text(value: &str) -> &'static str {
    if value == "ro" {
        "read only"
    } else {
        "read and write"
    }
}

fn when(o: &Outcome) -> String {
    match o.change.applies {
        Applies::NextStart => " It applies from the next start of the app.".into(),
        Applies::Live => String::new(),
        Applies::ReopenFiles => {
            let n = o.live.as_ref().and_then(|a| a.open_handles).unwrap_or(0);
            format!(
                " The app holds {n} files of it open: it must open them again to see the change."
            )
        }
    }
}

/// The line after a grant change. `what` is the folder with `~`, or the
/// target word.
pub fn grant_done(app: &str, what: &str, o: &Outcome) -> String {
    let c = &o.change;
    let guest = o
        .live
        .as_ref()
        .and_then(|a| a.guest.as_ref())
        .map(|g| format!(" Visible in the app under {}.", g.display()))
        .unwrap_or_default();
    match (c.what, c.action.as_str()) {
        (What::Folder, "add") => format!(
            "Added {what} to {app}, {}.{}{}",
            mode_text(&c.value),
            if c.applies == Applies::NextStart {
                String::new()
            } else {
                guest
            },
            when(o)
        ),
        (What::Folder, "remove") => {
            let open = o
                .live
                .as_ref()
                .and_then(|a| a.open_handles)
                .filter(|n| *n > 0)
                .map(|n| format!(" The app had {n} files open in it."))
                .unwrap_or_default();
            let tail = if c.applies == Applies::NextStart {
                " The next start leaves it out."
            } else {
                " The app does not see it now."
            };
            format!("Removed {what} from {app}.{tail}{open}")
        }
        (What::Folder, _) => format!(
            "{what} is now {} for {app}.{}",
            mode_text(&c.value),
            when(o)
        ),
        (What::Network, _) => {
            let closed = o
                .live
                .as_ref()
                .and_then(|a| a.closed)
                .filter(|n| *n > 0)
                .map(|n| format!(" {n} open connections closed."))
                .unwrap_or_default();
            format!(
                "The network mode of {app} is now {what}.{closed}{}",
                when(o)
            )
        }
        (What::Clipboard, _) | (What::Rule, _) => {
            format!("{app}: {}.{}", c.value, when(o))
        }
    }
}

/// `paddock app share NAME remove` of a folder with open files.
pub fn grant_remove_open(app: &str, shown_path: &str, open: u32) -> String {
    format!(
        "{app} has {open} files open in {shown_path}. Removing the folder closes them for the app, and unsaved changes in those files can be lost. To remove it anyway: paddock app share {app} remove '{shown_path}' --force"
    )
}

/// The shim did not answer `list`: the rows come from the record.
pub fn grant_list_from_record(e: &str) -> String {
    format!("the session did not answer ({e}). The counts below are from the session record")
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// The clipboard directions of a profile.
pub fn grant_clipboard_now(app: &str, c: Clipboard) -> String {
    format!(
        "Clipboard of {app}: paste from this Mac into the app {}, copy from the app to this Mac {}",
        on_off(c.to_app),
        on_off(c.from_app)
    )
}

/// The screen of `paddock app share NAME list`.
pub fn grant_list(l: &Listing) -> String {
    let mut out = String::new();
    let state = if l.running { "running" } else { "stopped" };
    out.push_str(&format!("Folders of {} ({state}):\n", l.app));
    if l.folders.is_empty() {
        out.push_str("  none. Add one: paddock app share NAME add ~/src/project\n");
    }
    for r in &l.folders {
        let status = match r.status {
            Applies::Live => "live",
            Applies::ReopenFiles => "live, reopen files",
            Applies::NextStart => "on next start",
        };
        let open = r
            .open_handles
            .map(|n| format!(", {n} open files"))
            .unwrap_or_default();
        let guest = r
            .guest
            .as_ref()
            .map(|g| format!(" -> {}", g.display()))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {}  {}  {status}{open}{guest}\n",
            r.shown,
            mode_text(&r.mode)
        ));
    }
    out.push_str(&format!(
        "Downloads: {} -> {}\n",
        l.downloads.shown,
        l.downloads.guest.display()
    ));
    out.push_str(&grant_clipboard_now(&l.app, l.clipboard));
    out.push('\n');
    if let Some(n) = &l.network {
        out.push_str(&format!("Network mode: {n}\n"));
    }
    out
}

/// A `grant` line while the session ends.
pub fn grant_quitting() -> String {
    "the session is ending: the change applies from the next start. Run the command again once the app has stopped".into()
}

/// A `grant` line while a share change waits for the framework.
pub fn grant_busy() -> String {
    "another folder change of this session is still going on. Try again in a few seconds".into()
}

/// The framework did not take a new share set.
pub fn grant_vm_refused(e: &str) -> String {
    format!("the VM did not take the folder change ({e}). The app sees the folders of before")
}

/// The running VM has no virtiofs device.
pub fn no_share_device() -> String {
    "the VM of the session has no shared folders device".into()
}

/// The agent cannot count open files.
pub fn open_files_off(detail: &str) -> String {
    format!("the guest agent does not count open files ({detail}). Folder rows show no open files in this session")
}

#[cfg(test)]
#[path = "grants_tests.rs"]
mod tests;

/// The live change was undone after persistence failed.
pub fn grant_reverted() -> &'static str {
    "The live change was reverted after the grants update failed"
}

/// An attempted live rollback failed.
pub fn grant_revert_failed(error: &str) -> String {
    format!("The live change could not be reverted: {error}")
}

/// The guest could not count open files.
pub fn grant_open_unknown(path: &Path) -> String {
    format!(
        "Cannot determine open files for {}; use --force to remove it",
        path.display()
    )
}

/// The session did not report a setting required for rollback.
pub fn grant_missing_live_setting(setting: &str) -> String {
    format!("Session did not report its {setting}")
}
