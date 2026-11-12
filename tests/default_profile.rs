//! `paddock run --profile default` with a fake harness, on bwrap (Linux)
//! and Seatbelt (macOS): the home is an overlay on a private snapshot of the
//! harness's data roots, or a clone of them on macOS. The harness sees its
//! host data, its writes stay in the run, the host files never change, and
//! the run folder is gone at the end. A temp home only.

mod common;

use std::path::Path;
use std::process::Command;

/// bwrap starts, and it can mount an overlay in a user namespace.
#[cfg(target_os = "linux")]
fn overlay_works(dir: &Path) -> bool {
    let d: Vec<_> = ["low", "up", "work", "dst"]
        .iter()
        .map(|n| {
            let p = dir.join("probe").join(n);
            std::fs::create_dir_all(&p).unwrap();
            p
        })
        .collect();
    Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "--overlay-src"])
        .arg(&d[0])
        .arg("--overlay")
        .args([&d[1], &d[2], &d[3]])
        .arg("true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn cmd(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("PADDOCK_NO_PROMOTE", "1")
        .env("PADDOCK_NO_DEDUPE", "1")
        .env_remove("TMUX")
        .env_remove("DBUS_SESSION_BUS_ADDRESS");
    for v in paddock::proxy::PROXY_VARS {
        c.env_remove(v);
    }
    c
}

const SCRIPT: &str = r#"
cat "$HOME/.fake/conv.jsonl" > seen-conv
cat "$HOME/.fake/auth.json" > seen-auth
ls -A "$HOME" > seen-home
printf "turn2\n" >> "$HOME/.fake/conv.jsonl"
printf "new\n" > "$HOME/.fake/new.txt"
rm "$HOME/.fake/old.txt"
"#;

#[test]
fn a_shadow_run_sees_the_host_data_and_changes_only_its_snapshot() {
    let tmp = common::tempdir();
    // On macOS the temp folder is behind the /var symlink. A one-shot run
    // starts a tmux server for `paddock view`: the guard of `tmp` stops it,
    // also when its socket is in a short folder outside the test folder.
    let root_buf = std::fs::canonicalize(tmp.path()).unwrap();
    let root = root_buf.as_path();
    #[cfg(target_os = "macos")]
    let backend = "seatbelt";
    #[cfg(not(target_os = "macos"))]
    let backend = {
        if !common::bwrap_works() || !overlay_works(root) {
            eprintln!("skipped: bwrap cannot start with an overlay here");
            return;
        }
        "bwrap"
    };
    let home = root.join("home");
    for d in [
        "cfg/paddock/harness.d",
        "home/.fake",
        "home/.ssh",
        "p",
        "run",
    ] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(home.join(".fake/conv.jsonl"), "turn1\n").unwrap();
    std::fs::write(home.join(".fake/old.txt"), "old\n").unwrap();
    std::fs::write(home.join(".fake/auth.json"), "{\"token\":\"host\"}").unwrap();
    std::fs::write(home.join(".fake.json"), "{}").unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "key").unwrap();
    std::fs::write(home.join(".bashrc"), "# mine").unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[devices]\nmount = false\n[kv]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/harness.d/fake.toml"),
        format!(
            "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n[state]\ndata = [\".fake\", \".fake.json\"]\nauth = [\".fake/auth.json\"]\n",
            serde_json::to_string(SCRIPT).unwrap()
        ),
    )
    .unwrap();
    let project = root.join("p");
    let out = cmd(root)
        .args([
            "run",
            "fake",
            "--profile",
            "default",
            "--backend",
            backend,
            "--network",
            "none",
            "-y",
        ])
        .arg(&project)
        .current_dir(&project)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let read = |f: &str| std::fs::read_to_string(project.join(f)).unwrap_or_default();
    // The harness saw its host data and its login.
    assert_eq!(read("seen-conv"), "turn1\n");
    assert_eq!(read("seen-auth"), "{\"token\":\"host\"}");
    // The real home is never visible.
    let listed = read("seen-home");
    assert!(
        listed.contains(".fake") && listed.contains(".fake.json"),
        "{listed}"
    );
    assert!(
        !listed.contains(".ssh") && !listed.contains(".bashrc"),
        "{listed}"
    );
    // The host files are unchanged.
    assert_eq!(
        std::fs::read_to_string(home.join(".fake/conv.jsonl")).unwrap(),
        "turn1\n"
    );
    assert!(home.join(".fake/old.txt").exists());
    assert!(!home.join(".fake/new.txt").exists());
    // The summary, then the run folder is gone.
    assert!(
        err.contains("added 1, changed 1 and deleted 1 file(s)"),
        "{err}"
    );
    assert!(err.contains("+ ~/.fake/new.txt"), "{err}");
    let runs = root.join("data/paddock/shadow/runs");
    let left = std::fs::read_dir(&runs).map(|r| r.count()).unwrap_or(0);
    assert_eq!(left, 0, "a run folder is left in {}", runs.display());
}

#[test]
fn a_container_backend_refuses_the_default_profile() {
    let tmp = common::tempdir();
    let root = tmp.path();
    for d in ["cfg/paddock/harness.d", "home/.fake", "p"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(
        root.join("cfg/paddock/harness.d/fake.toml"),
        "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n[state]\ndata = [\".fake\"]\n",
    )
    .unwrap();
    let out = cmd(root)
        .args([
            "run",
            "fake",
            "--profile",
            "default",
            "--backend",
            "docker",
            "-y",
        ])
        .arg(root.join("p"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("only on the bwrap backend"), "{err}");
    let runs = root.join("data/paddock/shadow/runs");
    assert_eq!(std::fs::read_dir(&runs).map(|r| r.count()).unwrap_or(0), 0);
}

#[test]
#[cfg(target_os = "linux")]
fn codex_mcp_filter_changes_only_the_shadow_copy() {
    let tmp = common::tempdir();
    let root = tmp.path();
    if !common::bwrap_works() || !overlay_works(root) {
        eprintln!("skipped: bwrap cannot start with an overlay here");
        return;
    }
    for d in [
        "cfg/paddock/harness.d",
        "home/.codex",
        "p/.codex",
        "run",
        "tools/bin",
    ] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let harness = root.join("tools/bin/codex");
    std::fs::write(&harness, "#!/bin/sh\nif [ \"$1\" = mcp ]; then echo '[{\"name\":\"paddock_probe\",\"enabled\":false}]'; exit 0; fi\nfor arg do [ \"$arg\" = mcp_servers.apple.enabled=false ] && echo yes > seen-override; done\ncat \"$HOME/.codex/config.toml\" > seen-mcp\n").unwrap();
    std::fs::set_permissions(
        &harness,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(
        root.join("p/.codex/config.toml"),
        "[mcp_servers.apple]\nenabled = true\n",
    )
    .unwrap();
    let config = root.join("home/.codex/config.toml");
    let original = "[mcp_servers.apple]\ncommand = \"npx\"\nargs = [\"xcodebuildmcp@latest\"]\n[mcp_servers.keep]\ncommand = \"other\"\n";
    std::fs::write(&config, original).unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[devices]\nmount = false\n[kv]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/harness.d/codex.toml"),
        "name = \"codex\"\n[locate]\ncommands = [\"codex\"]\n[state]\ndata = [\".codex\"]\n[[mcp]]\nfile = \".codex/config.toml\"\nformat = \"toml\"\npath = [\"mcp_servers\"]\n[[mcp]]\nfile = \".codex/config.toml\"\nin_project = true\nformat = \"toml\"\npath = [\"mcp_servers\"]\n",
    )
    .unwrap();
    let project = root.join("p");
    let path = std::env::join_paths(std::iter::once(root.join("tools/bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let out = cmd(root)
        .env("PATH", path)
        .args([
            "run",
            "codex",
            "--profile",
            "default",
            "--backend",
            "bwrap",
            "--network",
            "none",
            "-y",
        ])
        .arg(&project)
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen: toml::Value = std::fs::read_to_string(project.join("seen-mcp"))
        .unwrap()
        .parse()
        .unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            std::fs::read_to_string(project.join("seen-override")).unwrap(),
            "yes\n"
        );
    }
    assert!(seen["mcp_servers"]["apple"].get("enabled").is_none());
    assert!(seen["mcp_servers"]["keep"].get("enabled").is_none());
    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
}

#[test]
#[cfg(target_os = "linux")]
fn gemini_mcp_filter_keeps_project_config_untouched() {
    let tmp = common::tempdir();
    let root = tmp.path();
    if !common::bwrap_works() || !overlay_works(root) {
        eprintln!("skipped: bwrap cannot start with an overlay here");
        return;
    }
    for d in ["cfg/paddock/harness.d", "home/.gemini", "p/.gemini", "run"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let user_config = root.join("home/.gemini/settings.json");
    let project_config = root.join("p/.gemini/settings.json");
    let user_original =
        r#"{"mcpServers":{"apple":{"command":"xcodebuildmcp"}},"mcp":{"excluded":["prior"]}}"#;
    let project_original = r#"{"mcpServers":{"other":{"command":"other"}}}"#;
    std::fs::write(&user_config, user_original).unwrap();
    std::fs::write(&project_config, project_original).unwrap();
    std::fs::write(
        root.join("p/paddock.toml"),
        "[mcp.servers.other]\nstart = \"never\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[devices]\nmount = false\n[kv]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(root.join("cfg/paddock/harness.d/gemini.toml"), "name = \"gemini\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", \"cat $HOME/.gemini/settings.json > seen-mcp\"]\n[state]\ndata = [\".gemini\"]\n[[mcp]]\nfile = \".gemini/settings.json\"\npath = [\"mcpServers\"]\n[[mcp]]\nfile = \".gemini/settings.json\"\nin_project = true\npath = [\"mcpServers\"]\n").unwrap();
    let project = root.join("p");
    let out = cmd(root)
        .args([
            "run",
            "gemini",
            "--profile",
            "default",
            "--backend",
            "bwrap",
            "--network",
            "none",
            "-y",
        ])
        .arg(&project)
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("seen-mcp")).unwrap()).unwrap();
    let exclusions = seen["mcp"]["excluded"].as_array().unwrap();
    assert!(exclusions.contains(&serde_json::json!("prior")));
    assert!(exclusions.contains(&serde_json::json!("other")));
    if cfg!(target_os = "linux") {
        assert!(exclusions.contains(&serde_json::json!("apple")));
    }
    assert_eq!(std::fs::read_to_string(user_config).unwrap(), user_original);
    assert_eq!(
        std::fs::read_to_string(project_config).unwrap(),
        project_original
    );
}
