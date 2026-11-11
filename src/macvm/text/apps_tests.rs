//! One test per function of text/apps.rs, named as the function.

use super::super::apps as t;
use std::path::Path;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
    assert!(!s.contains('\n'), "{s:?} is more than one line");
}

const BUNDLE: &str = "/Applications/Sample Notes.app";

#[test]
fn no_mac_bundle() {
    has(
        &t::no_mac_bundle("sample-notes"),
        &["sample-notes", "[app.macos]", "bundles"],
    );
}

#[test]
fn bundle_not_installed() {
    has(
        &t::bundle_not_installed("vscode", &[BUNDLE.into(), "~/Applications/X.app".into()]),
        &["vscode", BUNDLE, "~/Applications/X.app", "/Applications"],
    );
}

#[test]
fn bundle_no_version() {
    has(
        &t::bundle_no_version(Path::new(BUNDLE)),
        &[BUNDLE, "CFBundleShortVersionString", "Info.plist"],
    );
}

#[test]
fn bundle_not_a_bundle() {
    has(
        &t::bundle_not_a_bundle(Path::new("/tmp/x")),
        &["/tmp/x", "Contents", ".app"],
    );
}

#[test]
fn copy_begin() {
    has(
        &t::copy_begin("Sample Notes", 600 << 20),
        &["Sample Notes", "0.6 GiB", "guest"],
    );
}

#[test]
fn copy_done() {
    has(
        &t::copy_done("Sample Notes", 21.47),
        &["Sample Notes", "21.5 s"],
    );
}

#[test]
fn copy_no_space() {
    let s = t::copy_no_space("Sample Notes", 2 << 30, 1 << 30);
    has(
        &s,
        &[
            "Sample Notes",
            "2 GiB",
            "1 GiB",
            "The old copy stays",
            "paddock app reset Sample Notes --data",
        ],
    );
}

#[test]
fn copy_while_running() {
    has(
        &t::copy_while_running("Sample Notes"),
        &["Sample Notes", "still runs", "Quit the app"],
    );
}

#[test]
fn copy_already_running() {
    has(
        &t::copy_already_running("Sample Notes"),
        &["Sample Notes", "already runs", "Wait"],
    );
}

#[test]
fn copy_command_failed() {
    has(
        &t::copy_command_failed("ditto", "No space left on device"),
        &["ditto", "No space left on device", "guest"],
    );
}

#[test]
fn cannot_clone_app() {
    has(
        &t::cannot_clone_app(Path::new(BUNDLE), "Permission denied"),
        &[BUNDLE, "Permission denied", "The old copy stays"],
    );
}

#[test]
fn cannot_commit_app() {
    has(
        &t::cannot_commit_app(Path::new("/p/apps/Sample Notes.app"), "File exists"),
        &["/p/apps/Sample Notes.app", "File exists"],
    );
}

#[test]
fn copy_target_refused() {
    has(
        &t::copy_target_refused(Path::new("/etc/Sample Notes.app")),
        &["/etc/Sample Notes.app", "Applications"],
    );
}

#[test]
fn cold_why() {
    for w in t::ColdWhy::ALL {
        let s = t::cold_why(w);
        assert!(!s.is_empty() && s.len() < 48 && !s.contains('\n'), "{s:?}");
        // The reason reads as the tail of the start line.
        let line = super::super::start_kind(super::super::StartKind::Cold, 40, Some(s));
        has(&line, &["cold start", "about 40 s", s]);
    }
    assert_eq!(
        t::cold_why(t::ColdWhy::First),
        "the first start of this profile"
    );
}

#[test]
fn backend_reason() {
    let s = t::backend_reason("15.2 (24C101)");
    has(&s, &["15.2 (24C101)"]);
    // It reads as the reason of the backend start line.
    has(
        &super::super::start_backend(&s),
        &["backend", "macvm", "15.2 (24C101)"],
    );
}

#[test]
fn memory_size_invalid() {
    has(
        &t::memory_size_invalid("sample-notes", "lots"),
        &["vm_memory", "sample-notes", "\"lots\"", "4G"],
    );
}

#[test]
fn launch_refused() {
    has(
        &t::launch_refused(
            "sample-notes",
            "no such app in the guest",
            "Sample Notes.app",
        ),
        &[
            "sample-notes",
            "no such app in the guest",
            "Sample Notes.app",
        ],
    );
}

#[test]
fn shim_rebuilt() {
    has(
        &t::shim_rebuilt("Sample Notes", "0.2.0", "0.2.1"),
        &["Sample Notes", "0.2.0", "0.2.1"],
    );
}

#[test]
fn sign_failed() {
    has(
        &t::sign_failed(Path::new("/x/Sample Notes.app"), "errSecInternalComponent"),
        &[
            "/x/Sample Notes.app",
            "errSecInternalComponent",
            "brew reinstall",
        ],
    );
}

#[test]
fn no_helper_binary() {
    has(
        &t::no_helper_binary(Path::new("/opt/homebrew/bin/paddock-vm")),
        &["/opt/homebrew/bin/paddock-vm", "brew reinstall"],
    );
}

#[test]
fn no_icon() {
    has(&t::no_icon("Sample Notes"), &["Sample Notes", "icon"]);
}

#[test]
fn launcher_written() {
    has(
        &t::launcher_written(
            "Sample Notes (paddock)",
            Path::new("/Users/me/Applications/Sample Notes (paddock).app"),
        ),
        &["Sample Notes (paddock)", "/Users/me/Applications", "Dock"],
    );
}

#[test]
fn drop_target_written() {
    has(
        &t::drop_target_written(Path::new("/Users/me/Applications/Paddock Apps.app")),
        &["Paddock Apps.app", "Drop an app"],
    );
}

#[test]
fn launcher_removed() {
    has(&t::launcher_removed(Path::new("/x/y.app")), &["/x/y.app"]);
}

#[test]
fn launcher_absent() {
    has(
        &t::launcher_absent(Path::new("/x/y.app")),
        &["/x/y.app", "does not exist"],
    );
}

#[test]
fn dropped_bundle_added() {
    has(
        &t::dropped_bundle_added("textedit", Path::new("/Applications/TextEdit.app")),
        &[
            "/Applications/TextEdit.app",
            "textedit",
            "paddock app run textedit",
        ],
    );
}

#[test]
fn dropped_not_a_bundle() {
    has(
        &t::dropped_not_a_bundle(Path::new("/Users/me/notes.txt")),
        &["/Users/me/notes.txt", ".app"],
    );
}

#[test]
fn not_a_table() {
    has(
        &t::not_a_table(Path::new("/Users/me/.config/paddock/config.toml"), "apps"),
        &["[apps]", "config.toml", "not a table"],
    );
}

#[test]
fn install_macos_only() {
    has(&t::install_macos_only(), &["macOS", "desktop entry"]);
}

#[test]
fn shim_no_target() {
    has(
        &t::shim_no_target(Path::new("/x/Paddock Apps.app")),
        &["/x/Paddock Apps.app", "paddock app install --drop"],
    );
}
