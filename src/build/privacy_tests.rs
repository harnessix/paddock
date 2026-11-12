//! The privacy mode in the spec: telemetry off by default, vendor mode
//! from the user configuration only.

use super::tests::privacy_spec;
use super::Built;
use std::path::{Path, PathBuf};

/// The home settings that the privacy mode adds. Other profile tables
/// (the Codex mail hooks of [[state.settings]]) add settings too.
fn privacy_settings(b: &Built) -> Vec<&crate::spec::HomeSetting> {
    let origin = format!("profile {} privacy", b.profile.name);
    b.spec
        .home_settings
        .iter()
        .filter(|s| s.origin.starts_with(&origin))
        .collect()
}

#[test]
fn privacy_defaults_are_in_the_spec_with_their_origin() {
    let origin = |n: &str| format!("profile {n} privacy (telemetry off)");
    let (_t, b) = privacy_spec("claude", "", "");
    for k in ["DISABLE_TELEMETRY", "DISABLE_ERROR_REPORTING"] {
        let v = &b.spec.env[k];
        // Set over the empty host value, so the host cannot undo it.
        assert_eq!(v.value, "1", "{k}");
        assert_eq!(v.origin, origin("claude"));
    }
    let (_t, b) = privacy_spec("grok", "", "");
    assert_eq!(b.spec.env["DISABLE_ERROR_REPORTING"].origin, origin("grok"));
    let (_t, b) = privacy_spec("copilot", "", "");
    assert!(b
        .spec
        .command
        .contains(&"--disable-client-telemetry".to_string()));
    let (_t, b) = privacy_spec("codex", "", "");
    let privacy = privacy_settings(&b);
    assert_eq!(privacy.len(), 1);
    let s = privacy[0];
    assert!(!s.no_pty);
    assert_eq!(s.file, PathBuf::from(".codex/config.toml"));
    assert_eq!(s.path, vec!["analytics", "enabled"]);
    assert_eq!(s.value, serde_json::json!(false));
    assert_eq!(s.origin, origin("codex"));
    let (_t, b) = privacy_spec("vscode", "", "");
    assert!(b
        .spec
        .home_settings
        .iter()
        .any(|s| s.path == ["telemetry.telemetryLevel"]
            && s.value == "off"
            && s.file == Path::new(".config/Code/User/settings.json")));
    // Every settings file is relative to the sandbox home.
    for name in ["codex", "gemini", "vscode"] {
        let (_t, b) = privacy_spec(name, "", "");
        assert!(!b.spec.home_settings.is_empty(), "{name}");
        for s in &b.spec.home_settings {
            assert!(s.file.is_relative(), "{name}: {}", s.file.display());
        }
    }
}

#[test]
fn vendor_mode_removes_the_privacy_defaults() {
    let user = "[privacy]\ntelemetry = \"vendor\"\n";
    let (_t, b) = privacy_spec("claude", user, "");
    assert!(!b.spec.env.values().any(|v| v.origin.contains("privacy")));
    assert_eq!(b.spec.env["DISABLE_TELEMETRY"].value, "");
    let (_t, b) = privacy_spec("codex", user, "");
    assert!(privacy_settings(&b).is_empty());
    let (_t, b) = privacy_spec("copilot", user, "");
    assert!(!b
        .spec
        .command
        .contains(&"--disable-client-telemetry".to_string()));
}

#[test]
fn a_project_file_cannot_change_the_privacy_mode() {
    let vendor = "[privacy]\ntelemetry = \"vendor\"\n";
    let off = "[privacy]\ntelemetry = \"off\"\n";
    // Turning telemetry on from the project is ignored ...
    let (_t, b) = privacy_spec("codex", "", vendor);
    assert_eq!(privacy_settings(&b).len(), 1);
    // ... and so are project variables with the same names.
    let (_t, b) = privacy_spec("claude", "", "[env.set]\nDISABLE_ERROR_REPORTING = \"\"\n");
    assert_eq!(b.spec.env["DISABLE_ERROR_REPORTING"].value, "1");
    // A project cannot turn vendor mode off either.
    let (_t, b) = privacy_spec("codex", vendor, off);
    assert!(privacy_settings(&b).is_empty());
}
