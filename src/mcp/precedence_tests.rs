use super::*;

#[test]
fn claude_local_server_takes_precedence_over_project_and_user() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let doc = serde_json::json!({
        "mcpServers": {"same": {"command": "user"}},
        "projects": {(project.to_str().unwrap()): {"mcpServers": {"same": {"command": "local"}}}}
    });
    std::fs::write(home.join(".claude.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"same":{"command":"project"}}}"#,
    )
    .unwrap();
    let profile: Profile = toml::from_str(include_str!("../../profiles/claude.toml")).unwrap();
    let env = crate::env::HostEnv::from_parts(home, "test".into(), 1, 1, Default::default());
    let found = servers(&env, &profile, &project);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].command.as_deref(), Some("local"));
    assert!(found[0].trusted);
}
