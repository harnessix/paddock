//! `paddock list --json` with the thin PATH of an app started from Finder,
//! a service or `env -i`: agents outside PATH are found and marked.

use std::path::Path;
use std::process::Command;

fn program(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn list_json_marks_agents_found_outside_path() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    let onpath = root.join("onpath");
    program(&home.join(".grok/bin/grok"));
    program(&home.join(".local/bin/paddock-fake-common"));
    program(&onpath.join("paddock-fake-path"));
    let d = home.join(".config/paddock/harness.d");
    std::fs::create_dir_all(&d).unwrap();
    for (name, cmd) in [
        ("fakecommon", "paddock-fake-common"),
        ("fakepath", "paddock-fake-path"),
    ] {
        std::fs::write(
            d.join(format!("{name}.toml")),
            format!("name = \"{name}\"\n[locate]\ncommands = [\"{cmd}\"]\n"),
        )
        .unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["list", "--json"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", onpath.display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let get = |name: &str| {
        v["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == name)
            .cloned()
            .unwrap()
    };
    let grok = get("grok");
    assert_eq!(grok["installed"], true);
    assert_eq!(grok["found_by"], "profile");
    assert_eq!(
        grok["exe"],
        home.join(".grok/bin/grok").display().to_string()
    );
    assert_eq!(get("fakecommon")["found_by"], "common");
    assert_eq!(get("fakepath")["found_by"], "path");
}
