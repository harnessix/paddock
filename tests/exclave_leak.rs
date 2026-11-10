//! Transport arguments must never appear in visible output or diagnostics.
#[test]
fn transport_canary_does_not_enter_commands_or_errors() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    assert!(Command::new("git")
        .current_dir(&project)
        .env("HOME", root)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(project.join("file"), "tracked").unwrap();
    assert!(Command::new("git")
        .current_dir(&project)
        .env("HOME", root)
        .args(["add", "file"])
        .status()
        .unwrap()
        .success());
    let config = root.join("config/paddock");
    std::fs::create_dir_all(&config).unwrap();
    let canary = "EXCLAVE_TRANSPORT_CANARY_TEST";
    let file = config.join("config.toml");
    std::fs::write(
        &file,
        format!("[exclave.peers.test]\ntransport = ['/does-not-exist', '{canary}']\n"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_paddock"))
            .current_dir(&project)
            .env_clear()
            .env("HOME", root)
            .env("PATH", std::env::var("PATH").unwrap())
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .args(args)
            .output()
            .unwrap()
    };
    for args in [
        vec!["ps", "--json"],
        vec!["explain", "codex"],
        vec!["exclave", "run", "--on", "test", "--", "/bin/echo", "local"],
    ] {
        let out = run(&args);
        assert!(!String::from_utf8_lossy(&out.stdout).contains(canary));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(canary));
    }
    std::fs::write(
        &file,
        format!("[exclave.peers.test]\ntransport = ['{canary}'\n"),
    )
    .unwrap();
    for args in [
        vec!["ps", "--json"],
        vec!["explain", "codex"],
        vec!["exclave", "run", "--", "/bin/true"],
    ] {
        let out = run(&args);
        assert!(!String::from_utf8_lossy(&out.stdout).contains(canary));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(canary));
    }
}
