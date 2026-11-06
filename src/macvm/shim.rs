//! The shim bundle of a session: the app bundle whose executable is
//! `paddock-vm`, so the session shows the app's name and icon in the Dock,
//! in Cmd-Tab and in Mission Control.
//!
//! A process takes its name and icon from the bundle that holds its
//! executable, so `paddock app run NAME` starts
//! `macvm/shims/NAME.app/Contents/MacOS/NAME app …` instead of the plain
//! helper. The same bundle, with a target file that names the app, is the
//! launcher of `paddock app install` and the drag target `Paddock Apps.app`
//! of `paddock app install --drop`.
//!
//! Everything here is plain file work and text, so the tests run on Linux.
//! `codesign` is a closure, so a Linux test checks the argv that paddock
//! would run. Every bundle is built as `<name>.app.partial` and renamed, so
//! a crash in the middle never leaves a bundle that Launch Services starts.

use super::layout::{commit, partial, Layout};
use super::text;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The bundle identifier prefix of a session.
pub const ID_PREFIX: &str = "dev.paddock.app";
/// The icon inside the shim bundle, cloned from the host bundle.
pub const ICON_NAME: &str = "app.icns";
/// The file that tells the executable what it is: `paddock app run NAME`,
/// or the drag target. Finder starts a bundle with no arguments, so the
/// bundle itself has to carry them.
pub const TARGET_NAME: &str = "paddock-target.json";
/// The entitlements that the Virtualization framework needs.
pub const ENTITLEMENTS: &str = include_str!("../../packaging/macos/paddock-vm.entitlements");
/// The name of the drag target bundle in `~/Applications`.
pub const DROP_BUNDLE: &str = "Paddock Apps";
/// The document type of the drag target: any app bundle.
pub const DROP_UTI: &str = "com.apple.application-bundle";

/// What the executable of a shim bundle does when Finder starts it with
/// no arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Target {
    /// A launcher: run this app profile.
    Install { app: String },
    /// The drag target: make a profile of each bundle dropped on it.
    Drop,
}

/// What a bundle is made for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    /// The profile name, and the file name of the bundle.
    pub app: String,
    /// `ChatGPT (paddock)`: the Dock name.
    pub display_name: String,
    pub bundle_id: String,
    /// The icon of the host bundle, cloned into the shim.
    pub icon: Option<PathBuf>,
    /// None for the session bundle, which paddock starts with an argv.
    pub target: Option<Target>,
    /// The drag target takes app bundles as documents.
    pub accepts_apps: bool,
}

impl Face {
    /// The face of a session: `paddock app run` starts it with an argv.
    pub fn session(app: &str, display_name: &str, icon: Option<PathBuf>) -> Self {
        Self {
            app: app.to_string(),
            display_name: paddock_name(display_name),
            bundle_id: bundle_id(app),
            icon,
            target: None,
            accepts_apps: false,
        }
    }

    /// The face of the launcher that `paddock app install NAME` makes.
    pub fn launcher(app: &str, display_name: &str, icon: Option<PathBuf>) -> Self {
        Self {
            target: Some(Target::Install {
                app: app.to_string(),
            }),
            ..Self::session(app, display_name, icon)
        }
    }

    /// The face of the drag target of `paddock app install --drop`.
    pub fn drop_target() -> Self {
        Self {
            app: DROP_BUNDLE.to_string(),
            display_name: DROP_BUNDLE.to_string(),
            bundle_id: format!("{ID_PREFIX}.drop"), // not user text
            icon: None,
            target: Some(Target::Drop),
            accepts_apps: true,
        }
    }
}

/// `ChatGPT` becomes `ChatGPT (paddock)`, the name of every session in the
/// Dock and in Cmd-Tab.
pub fn paddock_name(display_name: &str) -> String {
    format!("{display_name} (paddock)") // not user text
}

/// `dev.paddock.app.chatgpt-desktop`.
pub fn bundle_id(app: &str) -> String {
    format!("{ID_PREFIX}.{}", super::layout::safe_name(app)) // not user text
}

/// The shim bundle of a session: `macvm/shims/<NAME>.app`.
pub fn bundle(layout: &Layout, app: &str) -> PathBuf {
    layout
        .shims
        .join(format!("{}.app", super::layout::safe_name(app)))
}

/// The executable inside a bundle. Launch Services starts this file, and
/// the process name in the Dock comes from the bundle around it.
pub fn executable(bundle: &Path, app: &str) -> PathBuf {
    bundle
        .join("Contents/MacOS")
        .join(super::layout::safe_name(app))
}

/// The target file of a bundle, when it has one.
pub fn target_file(bundle: &Path) -> PathBuf {
    bundle.join("Contents/Resources").join(TARGET_NAME)
}

/// The `Info.plist` of a shim bundle. `paddock` is the version of this
/// build, which a later start compares to decide a rebuild.
pub fn info_plist(face: &Face, paddock: &str) -> String {
    let exe = super::layout::safe_name(&face.app);
    let icon = face
        .icon
        .as_ref()
        .map(|_| format!("\t<key>CFBundleIconFile</key>\n\t<string>{ICON_NAME}</string>\n"))
        .unwrap_or_default();
    let documents = if face.accepts_apps {
        format!(
            "\t<key>CFBundleDocumentTypes</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>CFBundleTypeName</key>\n\t\t\t<string>Application</string>\n\t\t\t<key>CFBundleTypeRole</key>\n\t\t\t<string>Viewer</string>\n\t\t\t<key>LSItemContentTypes</key>\n\t\t\t<array>\n\t\t\t\t<string>{DROP_UTI}</string>\n\t\t\t</array>\n\t\t</dict>\n\t</array>\n"
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>{exe}</string>
	<key>CFBundleIdentifier</key>
	<string>{id}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundleDisplayName</key>
	<string>{name}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{paddock}</string>
	<key>CFBundleVersion</key>
	<string>{paddock}</string>
{icon}{documents}	<key>LSMinimumSystemVersion</key>
	<string>14.0</string>
	<key>LSUIElement</key>
	<false/>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
</dict>
</plist>
"#,
        id = face.bundle_id,
        name = xml_text(&face.display_name),
    )
}

/// XML text of a plist string value.
fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The `codesign` arguments for a shim bundle: ad hoc, with the
/// entitlements of the helper, as the Homebrew formula signs it.
pub fn sign_argv(bundle: &Path, entitlements: &Path) -> Vec<String> {
    vec![
        "--force".into(),
        "--sign".into(),
        "-".into(),
        "--entitlements".into(),
        entitlements.display().to_string(),
        "--timestamp=none".into(),
        bundle.display().to_string(),
    ]
}

/// The version of paddock that built a bundle, from its `Info.plist`.
pub fn built_by(bundle: &Path) -> Option<String> {
    let bytes = std::fs::read(bundle.join("Contents/Info.plist")).ok()?;
    let xml = crate::backend::seatbelt::plist_xml(&bytes)?;
    crate::backend::seatbelt::plist_string(&xml, "CFBundleShortVersionString")
}

/// The file in `Contents/Resources` with the sha256 of the helper that the
/// bundle's executable was copied from. The signature rewrites the copy,
/// so the hash of the copy itself says nothing.
pub const HELPER_HASH: &str = "helper.sha256";

/// A bundle of another paddock version, of another helper binary (a
/// rebuild or a patched package keeps the version), or one that is not
/// there, is built again at the start. A helper that cannot be read
/// leaves the version alone to decide.
pub fn needs_rebuild(bundle: &Path, paddock: &str, helper: &Path) -> bool {
    match built_by(bundle) {
        Some(v) if v != paddock => true,
        Some(_) => match super::layout::sha256_file(helper) {
            Ok(h) => std::fs::read_to_string(bundle.join("Contents/Resources").join(HELPER_HASH))
                .map(|s| s.trim() != h)
                .unwrap_or(true),
            Err(_) => false,
        },
        None => true,
    }
}

/// Where `make` writes the entitlements it signs with.
fn entitlements_file(dir: &Path) -> PathBuf {
    dir.join("paddock-vm.entitlements")
}

/// Build a shim bundle under `dir`, as `<name>.app.partial`, then rename
/// it. `helper` is the `paddock-vm` of this install, `sign` runs
/// `codesign` with the argv of [`sign_argv`].
///
/// Nothing partial survives: a failure anywhere removes the partial
/// bundle, so the next start finds either a complete bundle of this
/// version or none.
pub fn make(
    dir: &Path,
    face: &Face,
    helper: &Path,
    paddock: &str,
    sign: &dyn Fn(&[String]) -> Result<()>,
) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if !helper.is_file() {
        bail!(text::no_helper_binary(helper));
    }
    let final_path = dir.join(format!("{}.app", super::layout::safe_name(&face.app)));
    let tmp = partial(&final_path);
    let _ = std::fs::remove_dir_all(&tmp);
    let built = (|| -> Result<()> {
        std::fs::create_dir_all(tmp.join("Contents/MacOS"))
            .with_context(|| text::cannot_make(&tmp))?;
        std::fs::create_dir_all(tmp.join("Contents/Resources"))
            .with_context(|| text::cannot_make(&tmp))?;
        let exe = executable(&tmp, &face.app);
        std::fs::copy(helper, &exe).with_context(|| text::cannot_copy(helper, &exe))?;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
        let plist = tmp.join("Contents/Info.plist");
        std::fs::write(&plist, info_plist(face, paddock))
            .with_context(|| text::cannot_write(&plist))?;
        if let Some(icon) = &face.icon {
            let dst = tmp.join("Contents/Resources").join(ICON_NAME);
            super::drop::clone_into(icon, &dst).with_context(|| text::cannot_copy(icon, &dst))?;
        }
        if let Some(target) = &face.target {
            let path = target_file(&tmp);
            let json = serde_json::to_vec_pretty(target)?;
            std::fs::write(&path, json).with_context(|| text::cannot_write(&path))?;
        }
        let hash =
            super::layout::sha256_file(helper).with_context(|| text::cannot_copy(helper, &exe))?;
        let hash_file = tmp.join("Contents/Resources").join(HELPER_HASH);
        std::fs::write(&hash_file, hash).with_context(|| text::cannot_write(&hash_file))?;
        let ent = entitlements_file(&tmp.join("Contents/Resources"));
        std::fs::write(&ent, ENTITLEMENTS).with_context(|| text::cannot_write(&ent))?;
        sign(&sign_argv(&tmp, &ent))?;
        Ok(())
    })();
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&final_path);
    commit(&tmp).with_context(|| text::cannot_commit(&final_path))?;
    Ok(final_path)
}

/// Remove a shim bundle and any partial of it.
pub fn remove(path: &Path) -> Result<bool> {
    let partial_path = partial(path);
    let _ = std::fs::remove_dir_all(&partial_path);
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| text::cannot_remove(path)),
    }
}

/// The argv that a shim bundle stands for when Finder starts its
/// executable with no arguments of its own. `argv` is the process's own
/// argv without the program name; `exe` is the running executable.
///
/// Finder of older macOS passes `-psn_0_12345`, and a document open
/// arrives as an Apple event, not as an argument, so an empty argv is the
/// normal case. None means the argv already says what to do.
pub fn argv_from_bundle(exe: &Path, argv: &[String]) -> Option<Vec<String>> {
    if argv.iter().any(|a| !a.starts_with("-psn_")) {
        return None;
    }
    // Contents/MacOS/<exe> -> the bundle folder.
    let bundle = exe.parent()?.parent()?.parent()?;
    let json = std::fs::read(target_file(bundle)).ok()?;
    match serde_json::from_slice::<Target>(&json).ok()? {
        Target::Install { app } => Some(vec!["install".into(), "--app".into(), app]),
        Target::Drop => Some(vec!["install".into(), "--drop-target".into()]),
    }
}

#[cfg(test)]
#[path = "shim_tests.rs"]
mod tests;
