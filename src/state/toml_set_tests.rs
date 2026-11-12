use super::*;

#[test]
fn sets_project_trust_and_keeps_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        "# mine\nmodel = \"x\"\n\n[mcp_servers.a]\ncommand = \"a\"\n",
    )
    .unwrap();
    let profile: Profile = toml::from_str(
        "name = \"c\"\n[[state.toml_set]]\nfile = \".codex/config.toml\"\npath = [\"projects\", \"{project}\", \"trust_level\"]\nvalue = \"trusted\"\n",
    )
    .unwrap();
    toml_set(&profile, home, Path::new("/w/app"), false).unwrap();
    let text = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    let v: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        v["projects"]["/w/app"]["trust_level"].as_str(),
        Some("trusted")
    );
    assert_eq!(v["mcp_servers"]["a"]["command"].as_str(), Some("a"));
    assert!(text.starts_with("# mine"));
}

#[test]
fn manual_approvals_leave_the_trust_entries_out() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let profile: Profile = toml::from_str(
        "name = \"c\"\n[[state.toml_set]]\nfile = \".codex/config.toml\"\npath = [\"projects\", \"{project}\", \"trust_level\"]\nvalue = \"trusted\"\ntrust = true\n[[state.toml_set]]\nfile = \".codex/config.toml\"\npath = [\"other\"]\nvalue = \"kept\"\n",
    )
    .unwrap();
    toml_set(&profile, home, Path::new("/w/app"), true).unwrap();
    let v: toml::Value =
        toml::from_str(&std::fs::read_to_string(home.join(".codex/config.toml")).unwrap()).unwrap();
    assert!(v.get("projects").is_none(), "no trust in manual mode: {v}");
    assert_eq!(v["other"].as_str(), Some("kept"));
    toml_set(&profile, home, Path::new("/w/app"), false).unwrap();
    let v: toml::Value =
        toml::from_str(&std::fs::read_to_string(home.join(".codex/config.toml")).unwrap()).unwrap();
    assert_eq!(
        v["projects"]["/w/app"]["trust_level"].as_str(),
        Some("trusted")
    );
}
