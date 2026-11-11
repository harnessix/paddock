//! Startup-policy reporting must agree with the sandbox's supported filters.
mod common;

#[test]
fn catalog_reports_disabled_and_unsupported_servers() {
    let tmp = common::tempdir();
    let root = tmp.path();
    for dir in ["home/.codex", "cfg/paddock/harness.d", "p"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(
        root.join("home/.codex/config.toml"),
        "[mcp_servers.off]\ncommand = \"sh\"\n[mcp_servers.later]\ncommand = \"sh\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[mcp.servers.off]\nstart = \"never\"\n[mcp.servers.later]\nstart = \"lazy\"\n",
    )
    .unwrap();
    std::fs::write(root.join("cfg/paddock/harness.d/codex.toml"), "name = \"codex\"\n[locate]\ncommands = [\"sh\"]\n[[mcp]]\nfile = \".codex/config.toml\"\nformat = \"toml\"\npath = [\"mcp_servers\"]\n").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_paddock"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .args(["mcp", "codex"])
        .current_dir(root.join("p"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("off: disabled by [mcp] policy"), "{text}");
    assert!(
        text.contains("off: lazy startup is not supported"),
        "{text}"
    );
    let explained = std::process::Command::new(env!("CARGO_BIN_EXE_paddock"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .args(["explain", "codex", "--json"])
        .current_dir(root.join("p"))
        .output()
        .unwrap();
    assert!(
        explained.status.success(),
        "{}",
        String::from_utf8_lossy(&explained.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&explained.stdout).unwrap();
    let reports = doc["mcp"].as_array().unwrap();
    assert!(reports.iter().any(|r| r["name"] == "off"
        && r["reason"]
            .as_str()
            .unwrap()
            .contains("disabled by [mcp] policy")));
    assert!(reports.iter().any(|r| r["name"] == "off"
        && r["same_as"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("later"))));
    assert!(reports.iter().any(|r| r["name"] == "later"
        && r["reason"]
            .as_str()
            .unwrap()
            .contains("lazy process startup is not supported")));
}
