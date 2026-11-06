//! What the parent of a session does before it starts the shim: find the
//! host app bundle, refuse what the guest cannot run, clone the bundle into
//! the profile, and build the Dock face of the session.
//!
//! The parts that touch no Mac have Linux tests: the bundle choice, the
//! memory of the guest and the shim argv. `codesign` is a closure.

use super::appcopy::{self, HostBundle};
use super::layout::Layout;
use super::session::{self, Profile, Stamp};
use super::shim::{self, Face};
use super::text;
use crate::env::HostEnv;
use crate::manifest::AppCfg;
use crate::profile::Profile as AppProfile;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The memory of the guest when the profile names none.
pub const DEFAULT_MEMORY_MIB: u64 = 4096;
/// `codesign` of the host.
pub const CODESIGN: &str = "/usr/bin/codesign";
/// The root that the absolute `[app.macos] bundles` entries live under:
/// `/` on a Mac. The tests pass a temporary folder, so that they never
/// read the apps installed on the machine that runs them.
pub const SYSTEM_ROOT: &str = "/";

/// The app bundle on this Mac. `[apps.NAME] bundle` of the user
/// configuration wins, which is how the drag target adds an app; else the
/// first `[app.macos] bundles` entry that exists. An absolute entry is
/// looked up under `root` (see [`SYSTEM_ROOT`]); the refusal names the
/// entries as the profile writes them.
pub fn find_bundle(
    root: &Path,
    env: &HostEnv,
    name: &str,
    profile: Option<&AppProfile>,
    cfg: Option<&AppCfg>,
) -> Result<PathBuf> {
    if let Some(named) = cfg.and_then(|c| c.bundle.as_deref()) {
        let p = env.expand(named, &env.home);
        if !p.join("Contents").is_dir() {
            bail!(text::bundle_not_installed(name, &[p.display().to_string()]));
        }
        return Ok(p);
    }
    let bundles = bundle_list(profile);
    if bundles.is_empty() {
        bail!(text::no_mac_bundle(name));
    }
    for b in &bundles {
        let p = under_root(root, &env.expand(b, &env.home), b);
        if p.join("Contents").is_dir() {
            return Ok(p);
        }
    }
    bail!(text::bundle_not_installed(name, &bundles))
}

/// `path` under `root` when the entry `raw` is absolute; an entry under
/// the home (`~/`) or relative to it stays as it is.
fn under_root(root: &Path, path: &Path, raw: &str) -> PathBuf {
    if !Path::new(raw).is_absolute() {
        return path.to_path_buf();
    }
    root.join(path.strip_prefix("/").unwrap_or(path))
}

/// The `[app.macos] bundles` list of a profile, empty without one.
fn bundle_list(profile: Option<&AppProfile>) -> Vec<String> {
    profile
        .and_then(|p| p.app.as_ref())
        .and_then(|a| a.macos.as_ref())
        .map(|m| m.bundles.clone())
        .unwrap_or_default()
}

/// The host bundle of a session, when the profile or the user
/// configuration names one. None means this session runs no app of its
/// own: the guest boots and the Mac checks drive it. A named bundle that
/// is missing, an App Store bundle and a bundle without a version are
/// errors, never None.
pub fn optional_bundle(
    root: &Path,
    env: &HostEnv,
    name: &str,
    profile: Option<&AppProfile>,
    cfg: Option<&AppCfg>,
) -> Result<Option<HostBundle>> {
    let named = cfg.and_then(|c| c.bundle.as_deref()).is_some();
    if !named && bundle_list(profile).is_empty() {
        return Ok(None);
    }
    read_bundle(root, env, name, profile, cfg).map(Some)
}

/// The host bundle of a session, read and checked: an App Store bundle is
/// refused here, before anything is cloned or locked.
pub fn read_bundle(
    root: &Path,
    env: &HostEnv,
    name: &str,
    profile: Option<&AppProfile>,
    cfg: Option<&AppCfg>,
) -> Result<HostBundle> {
    let path = find_bundle(root, env, name, profile, cfg)?;
    let bundle = appcopy::read(&path)?;
    appcopy::check(&bundle)?;
    Ok(bundle)
}

/// [`read_bundle`] on this Mac: the bundles under [`SYSTEM_ROOT`].
pub fn read_installed(
    env: &HostEnv,
    name: &str,
    profile: Option<&AppProfile>,
    cfg: Option<&AppCfg>,
) -> Result<HostBundle> {
    read_bundle(Path::new(SYSTEM_ROOT), env, name, profile, cfg)
}

/// The memory of the guest in MiB: `[apps.NAME] vm_memory`, else
/// [`DEFAULT_MEMORY_MIB`]. `host_gib` is the memory of this Mac, when
/// paddock knows it: more than half of it is refused.
pub fn memory_mib(name: &str, cfg: Option<&AppCfg>, host_gib: Option<u32>) -> Result<u64> {
    let Some(text) = cfg.and_then(|c| c.vm_memory.as_deref()) else {
        return Ok(DEFAULT_MEMORY_MIB);
    };
    let Some(bytes) = session::parse_size(text) else {
        bail!(text::memory_size_invalid(name, text));
    };
    let mib = bytes >> 20;
    if !(1024..=262_144).contains(&mib) {
        bail!(text::memory_out_of_range(mib, 1024, 262_144));
    }
    if let Some(line) = super::limits::memory_refusal(name, mib, host_gib) {
        bail!(line);
    }
    Ok(mib)
}

/// The memory of this Mac in GiB, from `sysctl hw.memsize`. None when the
/// value cannot be read, and always None off macOS.
pub fn host_memory_gib() -> Option<u32> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    let gib = bytes >> 30;
    (gib > 0).then_some(gib as u32)
}

/// Clone the host bundle into `profile/apps/<Name>.app` when the guest
/// needs this version. True when the session will copy it into the guest.
///
/// The clone is the source of the `apps` share, so it has to be there
/// before the shim starts. A clone that fails is one line, not a refusal:
/// the session then runs the copy that is already in the guest.
pub fn stage_bundle(profile: &Profile, bundle: &HostBundle) -> Result<bool> {
    stage_bundle_with(profile, bundle, &appcopy::ditto_archive)
}

/// [`stage_bundle`] with the archive writer: `ditto -c` on macOS, a fake
/// in the tests.
pub fn stage_bundle_with(
    profile: &Profile,
    bundle: &HostBundle,
    archive: &dyn Fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> Result<bool> {
    let stamp = Stamp::read(&profile.stamp()).ok();
    let needs = appcopy::needs_copy(stamp.as_ref().and_then(|s| s.app.as_ref()), &bundle.stamp());
    // The clone is also the file the session reads the app's version and
    // display name from, so it has to be there even when the guest keeps
    // the copy it has. A missing one (someone removed `apps/`) is cloned
    // again, and the session still copies nothing.
    let have_clone = bundle.clone_path(profile).join("Contents").is_dir();
    if !needs && have_clone {
        return Ok(false);
    }
    appcopy::clone_into_profile(profile, bundle, &super::drop::clone_into)?;
    if needs {
        appcopy::archive_into_profile(profile, bundle, archive)?;
    }
    Ok(needs)
}

/// The executable of the session's Dock face: `macvm/shims/<NAME>.app`,
/// built or rebuilt when it is missing or from another paddock version.
/// Instance N of an app has its own face, `<NAME>--N.app`, named
/// `ChatGPT N (paddock)`, so the Dock tells the instances apart.
pub fn shim_exe(
    layout: &Layout,
    name: &str,
    instance: u32,
    bundle: Option<&HostBundle>,
    helper: &Path,
    paddock: &str,
) -> Result<PathBuf> {
    let key = super::instance::key(name, instance);
    let app = key.as_str();
    let path = shim::bundle(layout, app);
    if shim::needs_rebuild(&path, paddock, helper) {
        if let Some(old) = shim::built_by(&path) {
            eprintln!("{}", text::shim_rebuilt(app, &old, paddock));
        }
        crate::privfs::dir(&layout.shims).with_context(|| text::cannot_make(&layout.shims))?;
        // Without a bundle the session still gets a Dock face, with the
        // profile name and the generic application icon.
        let display = bundle.map(|b| b.display_name.as_str()).unwrap_or(name);
        let shown = super::instance::label(display, instance);
        let face = Face::session(app, &shown, bundle.and_then(|b| b.icon.clone()));
        if face.icon.is_none() && bundle.is_some() {
            eprintln!("{}", text::no_icon(display));
        }
        shim::make(&layout.shims, &face, helper, paddock, &codesign)?;
    }
    Ok(shim::executable(&path, app))
}

/// Sign a bundle ad hoc with the entitlements that the Virtualization
/// framework needs. Without the signature the framework refuses the start.
pub fn codesign(argv: &[String]) -> Result<()> {
    let out = std::process::Command::new(CODESIGN)
        .args(argv)
        .output()
        .with_context(|| text::cannot_run(CODESIGN))?;
    if out.status.success() {
        return Ok(());
    }
    let target = argv.last().cloned().unwrap_or_default();
    let reason = String::from_utf8_lossy(&out.stderr)
        .lines()
        .last()
        .unwrap_or("codesign failed")
        .trim()
        .to_string();
    bail!(text::sign_failed(Path::new(&target), &reason))
}

/// The app arguments of a session: the guest paths of the workspace
/// folders, for an editor whose profile says `open_roots`.
pub fn app_args(profile: Option<&AppProfile>, shares: &[super::shares::Share]) -> Vec<String> {
    let opens = profile
        .and_then(|p| p.app.as_ref())
        .is_some_and(|a| a.open_roots);
    if !opens {
        return Vec::new();
    }
    shares
        .iter()
        .filter(|s| s.is_workspace())
        .map(|s| s.guest_path().display().to_string())
        .collect()
}

#[cfg(test)]
#[path = "host_app_tests.rs"]
mod tests;
