//! The texts of the app on the route: the host bundle and its refusals, the
//! copy into the guest, the shim bundle that gives the session the app's
//! Dock face, the launcher of `paddock app install` and the drag target.

use super::{gib, shown};
use std::path::Path;

// ---- The host bundle ----------------------------------------------------

/// The profile has no `[app.macos] bundles`, so the route cannot find
/// the app on this Mac.
pub fn no_mac_bundle(name: &str) -> String {
    format!("the {name} profile names no app bundle for macOS. Add bundles = [\"/Applications/<App>.app\"] to [app.macos] of its profile")
}

/// None of the named bundles is installed.
pub fn bundle_not_installed(name: &str, tried: &[String]) -> String {
    format!(
        "{name} is not installed on this Mac: no app bundle at {}. Install it into /Applications, then run paddock app run {name} again",
        tried.join(", ")
    )
}

/// `Contents/Info.plist` names no version, so the copy cannot be stamped.
pub fn bundle_no_version(path: &Path) -> String {
    format!(
        "{} has no CFBundleShortVersionString in Contents/Info.plist, so paddock cannot tell its version. Reinstall the app from the vendor's download",
        shown(path)
    )
}

/// The bundle has no `Contents` folder.
pub fn bundle_not_a_bundle(path: &Path) -> String {
    format!(
        "{} is not an app bundle: it has no Contents folder. Name the .app folder of the app",
        shown(path)
    )
}

// ---- The copy into the guest --------------------------------------------

/// The reason line of the start event of a copy.
pub fn copy_begin(app: &str, size: u64) -> String {
    format!("the copy of {app} ({}) into the guest starts", gib(size))
}

/// The copy ended, with the seconds it took.
pub fn copy_done(app: &str, secs: f64) -> String {
    format!("{app} is in the guest, the copy took {secs:.1} s")
}

/// The guest has too little room on the data disk for the copy. The old
/// copy is untouched.
pub fn copy_no_space(app: &str, needs: u64, free: u64) -> String {
    format!(
        "no room in the guest for {app} (needs {}, {} free on the data disk). The old copy stays. Free space in the guest, or give the profile a new data disk: paddock app reset {app} --data",
        gib(needs),
        gib(free)
    )
}

/// The app still runs in the guest, so the copy does not replace it.
pub fn copy_while_running(app: &str) -> String {
    format!("{app} still runs in the guest, so paddock keeps the copy that runs. Quit the app, then start it again to get the new version")
}

/// A second `app.copy` while one runs.
pub fn copy_already_running(app: &str) -> String {
    format!("a copy of {app} into the guest already runs. Wait for it to end, then try again")
}

/// A `ditto` or a rename of the copy failed in the guest.
pub fn copy_command_failed(what: &str, reason: &str) -> String {
    format!("the {what} of the app copy failed in the guest: {reason}")
}

/// The copy in the guest does not carry an intact signature.
pub fn copy_signature_broken(app: &str, reason: &str) -> String {
    format!(
        "the copy of {app} in the guest has a broken signature, so macOS would call it damaged: {reason}. The old copy stays, and the next start copies again"
    )
}

/// The clone of the host bundle into the profile failed.
pub fn cannot_clone_app(path: &Path, reason: &str) -> String {
    format!(
        "paddock could not copy {} into the profile: {reason}. The old copy stays, and the next start tries again",
        shown(path)
    )
}

/// The rename of the clone to its final name failed.
pub fn cannot_commit_app(path: &Path, reason: &str) -> String {
    format!(
        "paddock could not finish the copy {}: {reason}. The next start copies the app again",
        shown(path)
    )
}

/// The target of `app.copy` is not under the guest home.
pub fn copy_target_refused(path: &Path) -> String {
    format!(
        "the app copy target {} is not an Applications folder of the guest home",
        shown(path)
    )
}

// ---- The shim bundle ----------------------------------------------------

/// Why a start is a cold one, in the few words that the start line
/// carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdWhy {
    /// The profile is new.
    First,
    /// A crash left the profile half built, and paddock rebuilt it.
    Repaired,
    /// The guest image is another version now.
    NewGolden,
    /// This Mac runs another macOS build than the saved state.
    NewHost,
    /// No saved state matches this start.
    NoState,
}

impl ColdWhy {
    pub const ALL: [ColdWhy; 5] = [
        ColdWhy::First,
        ColdWhy::Repaired,
        ColdWhy::NewGolden,
        ColdWhy::NewHost,
        ColdWhy::NoState,
    ];
}

/// The reason of a cold start, for `start_kind`.
pub fn cold_why(w: ColdWhy) -> &'static str {
    match w {
        ColdWhy::First => "the first start of this profile",
        ColdWhy::Repaired => "the profile was rebuilt",
        ColdWhy::NewGolden => "the guest image changed",
        ColdWhy::NewHost => "this Mac runs a new macOS build",
        ColdWhy::NoState => "no saved state",
    }
}

/// Why `start_backend` picks the guest: the golden is sealed.
pub fn backend_reason(golden: &str) -> String {
    format!("the guest macOS {golden} is ready")
}

/// `[apps.NAME] vm_memory` is not a size.
pub fn memory_size_invalid(name: &str, value: &str) -> String {
    format!("vm_memory = {value:?} in [apps.{name}] is not a size. Write it as 4G, 6144M or a number of bytes")
}

/// The agent refused `app.launch`: the copy is not there, or the guest
/// said no.
pub fn launch_refused(app: &str, code: &str, detail: &str) -> String {
    format!(
        "{app} did not start in the guest: {code} ({detail}). The next start copies the app again"
    )
}

/// The shim bundle is from another paddock version and is built again.
pub fn shim_rebuilt(app: &str, old: &str, new: &str) -> String {
    format!("the Dock face of {app} was built by paddock {old}, so paddock {new} makes it again")
}

/// `codesign` of the shim bundle failed. Without a signature the
/// Virtualization framework refuses the start.
pub fn sign_failed(path: &Path, reason: &str) -> String {
    format!(
        "paddock could not sign {}: {reason}. Install paddock again (brew reinstall paddock), then try once more",
        shown(path)
    )
}

/// The `paddock-vm` helper is not next to paddock.
pub fn no_helper_binary(path: &Path) -> String {
    format!(
        "the helper {} is missing, and the macOS guest needs it. Install paddock again: brew reinstall paddock",
        shown(path)
    )
}

/// The host bundle has no icon to clone, so the session shows the
/// generic application icon.
pub fn no_icon(app: &str) -> String {
    format!("{app} carries no icon file, so its paddock session shows the generic application icon")
}

// ---- The launcher and the drag target -----------------------------------

pub fn launcher_written(label: &str, path: &Path) -> String {
    format!(
        "{label} is in {}. Start it from the Dock, Launchpad or Spotlight",
        shown(path)
    )
}

pub fn drop_target_written(path: &Path) -> String {
    format!(
        "the drag target is in {}. Drop an app on it to run that app in paddock's macOS guest",
        shown(path)
    )
}

pub fn launcher_removed(path: &Path) -> String {
    format!("removed {}", shown(path))
}

pub fn launcher_absent(path: &Path) -> String {
    format!("{} does not exist", shown(path))
}

/// A dropped bundle became an app profile of the user configuration.
pub fn dropped_bundle_added(slug: &str, path: &Path) -> String {
    format!(
        "{} runs in paddock's macOS guest as the profile {slug}. Start it later with paddock app run {slug}",
        shown(path)
    )
}

/// The dropped item is not an app bundle.
pub fn dropped_not_a_bundle(path: &Path) -> String {
    format!(
        "{} is not an app bundle, so paddock cannot run it in the guest. Drop a .app folder of /Applications",
        shown(path)
    )
}

/// A table of the user configuration is not a table.
pub fn not_a_table(file: &Path, key: &str) -> String {
    format!(
        "[{key}] of {} is not a table. Fix the file by hand, or move it aside",
        shown(file)
    )
}

/// `paddock app install` on a system that has no macOS guest.
pub fn install_macos_only() -> String {
    "a Dock launcher for paddock's macOS guest is made only on macOS. On Linux, paddock app install writes a desktop entry".into()
}

/// The drag target holds no app name, so the shim cannot tell what to run.
pub fn shim_no_target(path: &Path) -> String {
    format!(
        "{} names no app to start. Make it again: paddock app install --drop",
        shown(path)
    )
}

#[cfg(test)]
#[path = "apps_tests.rs"]
mod tests;
