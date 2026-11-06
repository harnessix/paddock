//! The Dock launchers of app mode on macOS: `~/Applications/<Name>
//! (paddock).app` for one app, and `~/Applications/Paddock Apps.app` as the
//! drag target for any bundle.
//!
//! Both are the shim bundle of `macvm/shim.rs` with a target file that
//! says what the executable does when Finder starts it: run one profile,
//! or make a profile of each bundle dropped on it. The paths, the names
//! and the slug of a dropped bundle are pure, with Linux tests;
//! `codesign` runs only on a Mac.

use crate::env::HostEnv;
use crate::macvm::appcopy;
use crate::macvm::host_app::codesign;
use crate::macvm::shim::{self, Face, DROP_BUNDLE};
use crate::macvm::text;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Where macOS looks for a user's own apps.
pub fn applications_dir(env: &HostEnv) -> PathBuf {
    env.home.join("Applications")
}

/// `~/Applications/<Name> (paddock).app` of one app profile.
pub fn launcher_path(env: &HostEnv, display_name: &str) -> PathBuf {
    applications_dir(env).join(format!("{}.app", shim::paddock_name(display_name)))
}

/// `~/Applications/Paddock Apps.app`, the drag target.
pub fn drop_path(env: &HostEnv) -> PathBuf {
    applications_dir(env).join(format!("{DROP_BUNDLE}.app"))
}

/// `paddock app install NAME` on macOS: the launcher bundle, with the
/// app's own name and icon, which runs `paddock app run NAME`.
pub fn install(
    env: &HostEnv,
    name: &str,
    display_name: &str,
    icon: Option<PathBuf>,
) -> Result<PathBuf> {
    let dir = applications_dir(env);
    std::fs::create_dir_all(&dir).with_context(|| text::cannot_make(&dir))?;
    let helper = crate::vm::host::helper_path(env)?;
    let face = Face::launcher(name, display_name, icon);
    // The bundle's folder name is its Dock name, not the profile name, so
    // Spotlight and Launchpad show "ChatGPT (paddock)".
    make_named(
        &dir,
        &face,
        &shim::paddock_name(display_name),
        &helper,
        crate::macvm::version::paddock_version(),
    )
}

/// `paddock app install --drop` on macOS: the drag target.
pub fn install_drop(env: &HostEnv) -> Result<PathBuf> {
    let dir = applications_dir(env);
    std::fs::create_dir_all(&dir).with_context(|| text::cannot_make(&dir))?;
    let helper = crate::vm::host::helper_path(env)?;
    make_named(
        &dir,
        &Face::drop_target(),
        DROP_BUNDLE,
        &helper,
        crate::macvm::version::paddock_version(),
    )
}

/// Build a shim bundle whose folder name is `file_name` instead of the
/// profile name, and whose executable keeps the safe name.
fn make_named(
    dir: &Path,
    face: &Face,
    file_name: &str,
    helper: &Path,
    paddock: &str,
) -> Result<PathBuf> {
    let built = shim::make(dir, face, helper, paddock, &codesign)?;
    let wanted = dir.join(format!("{file_name}.app"));
    if built == wanted {
        return Ok(built);
    }
    let _ = std::fs::remove_dir_all(&wanted);
    std::fs::rename(&built, &wanted).with_context(|| text::cannot_move(&built, &wanted))?;
    Ok(wanted)
}

/// `paddock app uninstall NAME` on macOS: the launcher of one app. True
/// when a bundle was there.
pub fn uninstall(env: &HostEnv, display_name: &str) -> Result<bool> {
    shim::remove(&launcher_path(env, display_name))
}

/// Remove the drag target.
pub fn uninstall_drop(env: &HostEnv) -> Result<bool> {
    shim::remove(&drop_path(env))
}

/// A bundle that the user dropped on the drag target becomes an app
/// profile: `[apps.<slug>] bundle = "<path>"` in the user configuration,
/// and its own launcher. Returns the slug.
///
/// The slug is unique against the built-in profiles and the ones the user
/// already has, so a second TextEdit never takes over the first one's
/// profile and its login.
pub fn add_dropped(env: &HostEnv, path: &Path) -> Result<String> {
    if !path.join("Contents").is_dir() {
        bail!(text::dropped_not_a_bundle(path));
    }
    let bundle = appcopy::read(path)?;
    appcopy::check(&bundle)?;
    let taken = taken_names(env);
    // A bundle that is already an app keeps its profile.
    if let Some(slug) = existing_for(env, path) {
        return Ok(slug);
    }
    let slug = appcopy::slug(&bundle.display_name, &taken);
    set_bundle(env, &slug, path)?;
    install(env, &slug, &bundle.display_name, bundle.icon.clone())?;
    Ok(slug)
}

/// The profile names that a new slug must avoid: the built-in profiles
/// and the `[apps.*]` entries of the user configuration.
pub fn taken_names(env: &HostEnv) -> Vec<String> {
    let mut out: Vec<String> = crate::profile::HarnessFactory::new(env)
        .map(|f| f.names())
        .unwrap_or_default();
    if let Ok(m) = crate::manifest::Manifest::load(env, &env.home.join(".paddock-no-project")) {
        out.extend(m.merged.apps.keys().cloned());
    }
    out.sort();
    out.dedup();
    out
}

/// The slug of an app whose `[apps.NAME] bundle` is already this path.
fn existing_for(env: &HostEnv, path: &Path) -> Option<String> {
    let m = crate::manifest::Manifest::load(env, &env.home.join(".paddock-no-project")).ok()?;
    m.merged.apps.iter().find_map(|(name, cfg)| {
        let named = cfg.bundle.as_deref()?;
        (env.expand(named, &env.home) == path).then(|| name.clone())
    })
}

/// Write `[apps.NAME] bundle = "<path>"` in the user configuration,
/// keeping the rest of the file as it is. It mirrors
/// `state::set_app_roots`, which writes the folders the same way.
pub fn set_bundle(env: &HostEnv, name: &str, bundle: &Path) -> Result<()> {
    let file = env.paddock_config().join("config.toml");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        existing.parse().with_context(|| text::cannot_read(&file))?;
    let apps = doc.entry("apps").or_insert(toml_edit::table());
    let apps = apps
        .as_table_mut()
        .with_context(|| text::not_a_table(&file, "apps"))?;
    apps.set_implicit(true);
    let app = apps.entry(name).or_insert(toml_edit::table());
    let app = app
        .as_table_mut()
        .with_context(|| text::not_a_table(&file, name))?;
    app.insert("bundle", toml_edit::value(bundle.display().to_string()));
    std::fs::create_dir_all(file.parent().unwrap()).with_context(|| text::cannot_make(&file))?;
    std::fs::write(&file, doc.to_string()).with_context(|| text::cannot_write(&file))?;
    Ok(())
}

#[cfg(test)]
#[path = "macapp_tests.rs"]
mod tests;
