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
        "the grants file {} is damaged ({e}). paddock does not change it. Repair it, or move it away and set the network again with paddock app network; until then a start has no extra grants, network none and the clipboard off",
        shown(path)
    )
}

/// The grants file names a network mode that this paddock does not set.
pub fn grants_bad_network(path: &Path, word: &str) -> String {
    format!(
        "the grants file {} names the network mode {word:?}, which is not ask, allowlist or none. paddock does not change it. Repair it, or move it away and set the network again with paddock app network",
        shown(path)
    )
}

/// The grants file is gone, but the profile kept grants before.
pub fn grants_missing(path: &Path) -> String {
    format!(
        "the grants file {} is missing, but this profile saved grants before. To start again with no extra grants, set the network: paddock app network NAME ask|allowlist|none",
        shown(path)
    )
}

/// A grant change other than the network, while the grants file is missing.
pub fn grants_missing_unchanged(path: &Path) -> String {
    format!(
        "{}. Until then paddock saves no other grant change",
        grants_missing(path)
    )
}

/// The grants changed between the first read of a start and its session.
pub fn grants_changed_at_start() -> &'static str {
    "the saved folders changed while the app started. Nothing started; start it again"
}

/// Another grants command holds the lock for longer than a start waits.
pub fn grants_busy_at_start(path: &Path) -> String {
    format!(
        "another paddock command holds {} too long. Nothing started; start the app again",
        shown(path)
    )
}

/// The policy was renamed into place, but its folder sync failed.
pub fn grants_unconfirmed(path: &Path) -> String {
    format!(
        "the new grants file {} is in place, but paddock cannot confirm that it is on disk",
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
    format!("{e}. This start uses none of its folders; network is none unless explicitly overridden, and both clipboard directions are off")
}

/// The new grants file cannot be written.
pub fn grants_unwritable(path: &Path) -> String {
    format!(
        "paddock cannot confirm the write to {}. Read the saved grants before trying again",
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
        Applies::Unknown => " Its current state is unknown.".into(),
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
        (What::Folder, "keep") => format!(
            "Kept {what} for the next starts of {app}, {}. The running session does not change.",
            mode_text(&c.value)
        ),
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
/// The saved clipboard directions and, while the app runs, the ones of
/// the session.
pub fn grant_clipboard_both(app: &str, profile: Clipboard, session: Option<Clipboard>) -> String {
    let mut line = format!(
        "Saved clipboard of {app}: paste from this Mac into the app {}, copy from the app to this Mac {}",
        on_off(profile.to_app),
        on_off(profile.from_app)
    );
    if let Some(s) = session {
        line.push_str(&format!(
            "\nThis session: paste into the app {}, copy from the app {}. It ends when the app stops",
            on_off(s.to_app),
            on_off(s.from_app)
        ));
    }
    line
}

/// A change of the running session while the app does not run.
pub fn grant_session_not_running(app: &str) -> String {
    format!("{app} is not running. Start it with paddock app run {app}, or leave out --session to change the profile")
}

/// `share keep` while the app does not run: no folder of a session.
pub fn grant_keep_not_running(app: &str) -> String {
    format!("{app} is not running, so it has no folder of a session to keep. paddock app share {app} add PATH saves a folder for the next start")
}

/// `share keep` of a folder that the profile keeps already.
pub fn grant_already_kept(app: &str, path: &Path) -> String {
    format!(
        "{} is kept for the next starts of {app} already. paddock app share {app} list shows the folders",
        shown(path)
    )
}

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
            Applies::Unknown => "unknown",
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

/// The saved file has the change but its sync failed: the live change stays.
pub fn grant_kept_unconfirmed() -> &'static str {
    "The live change stays, because the saved grants already have it. Read the saved grants before trying again"
}

/// The session started or stopped between the check and the lock.
pub fn grant_session_changed(app: &str) -> String {
    format!("{app} started or stopped during this change. Nothing changed; run the command again")
}

/// An attempted live rollback failed: the session keeps a change that the
/// grants file does not hold.
pub fn grant_revert_failed(error: &str) -> String {
    format!("The live change could not be reverted: {error}. The app keeps the change until the app stops, but the grants file does not hold it, so the next start does not have it")
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

/// The mark at the end of a rule value from history, whose order paddock
/// does not know. Readers look for it to keep the uncertainty.
pub const HISTORICAL_MARK: &str = " (historical; order unknown)";

/// The value of a rule change from history: `allow session (historical;
/// order unknown)`.
pub fn grant_historical(value: &str) -> String {
    format!("{value}{HISTORICAL_MARK}")
}

/// The start found checkpoint grant history in the grants file.
pub fn grant_history_found() -> &'static str {
    "Checkpoint grant history was found. Older readers are safe only after all checkpoint writers have stopped; a running writer can reintroduce incompatible history."
}

/// The start could not move the history out of the grants file.
pub fn grant_history_not_moved(path: &Path, e: &anyhow::Error) -> String {
    format!(
        "Could not move historical grant changes out of {}: {e:#}. This start uses the saved policy, but older paddock versions may not read it safely.",
        path.display()
    )
}

/// The context of a failed read or plan of the grant history file.
pub fn grant_history_at(path: &Path) -> String {
    format!("grant history at {}", path.display())
}

/// The recovery copy of checkpoint history could not be written.
pub fn grant_checkpoint_not_kept() -> &'static str {
    "could not preserve checkpoint grant history"
}

/// The policy holds checkpoint history that a save cannot keep.
pub fn grant_checkpoint_unpreservable() -> &'static str {
    "cannot preserve the original checkpoint grant history"
}

/// The history sidecar was not written after the policy was saved.
pub fn grant_history_not_saved(e: &anyhow::Error) -> String {
    format!("grant history was not saved: {e:#}")
}

/// The context of a failed write of a history recovery copy.
pub fn grant_history_writing(path: &Path) -> String {
    format!("writing {}", path.display())
}

/// Every name for a history recovery copy is taken.
pub fn grant_history_no_recovery_name() -> &'static str {
    "no free grant history recovery name"
}
