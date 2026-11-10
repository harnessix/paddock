//! End-to-end fake transport; requires host namespaces and a user manager.
#[cfg(target_os = "linux")]
#[test]
fn fake_peer_streams_output_and_returns_exit_code() {
    use std::process::{Command, Stdio};
    let bwrap = Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let scope = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !bwrap || !scope {
        eprintln!("skipped: peer requires bwrap and a systemd user manager");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut git = Command::new("git");
    assert!(git
        .current_dir(&project)
        .env("HOME", &home)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(project.join("source"), "original").unwrap();
    std::os::unix::fs::symlink("source", project.join("source-link")).unwrap();
    assert!(Command::new("git")
        .current_dir(&project)
        .env("HOME", &home)
        .args(["add", "source", "source-link"])
        .status()
        .unwrap()
        .success());
    let config = temp.path().join("config/paddock");
    std::fs::create_dir_all(&config).unwrap();
    let peer = temp.path().join("peer");
    std::fs::create_dir(&peer).unwrap();
    let tools = temp.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    std::fs::write(tools.join("tool-real"), "#!/bin/sh\nprintf tool-ok\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        tools.join("tool-real"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink(tools.join("tool-real"), tools.join("tool-link")).unwrap();
    std::fs::create_dir_all(peer.join("config/paddock")).unwrap();
    std::fs::write(peer.join("config/paddock/config.toml"), format!("[exclave.serve]\nallow_shared_disk = true\nro_paths = ['{}']\n[exclave.serve.env]\nPIP_INDEX_URL = 'https://mirror.example/simple'\nUV_INDEX_URL = 'https://mirror.example/simple'\n", tools.display())).unwrap();
    let script = temp.path().join("transport.sh");
    let binary = env!("CARGO_BIN_EXE_paddock");
    std::fs::write(&script, format!("#!/bin/sh\nexport HOME='{0}' XDG_CONFIG_HOME='{0}/config' XDG_DATA_HOME='{0}/data' XDG_STATE_HOME='{0}/state' XDG_CACHE_HOME='{0}/cache'\nexec '{1}' exclave serve\n", peer.display(), binary)).unwrap();
    let canary = "EXCLAVE_TEST_TRANSPORT_CANARY";
    std::fs::write(
        config.join("config.toml"),
        format!(
            "[exclave.peers.fake]\ntransport = ['/bin/sh', '{}', '{}']\n",
            script.display(),
            canary
        ),
    )
    .unwrap();
    let run = |text: &str| {
        let mut c = Command::new(binary);
        c.current_dir(&project)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"));
        c.args([
            "exclave",
            "run",
            "--on",
            "fake",
            "--network",
            "none",
            "--",
            "/bin/sh",
            "-c",
            text,
        ])
        .output()
        .unwrap()
    };
    let refused_gpu = Command::new(binary)
        .current_dir(&project)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .args([
            "exclave",
            "run",
            "--on",
            "fake",
            "--gpu",
            "--network",
            "none",
            "--",
            "/bin/true",
        ])
        .output()
        .unwrap();
    assert!(!refused_gpu.status.success());
    assert!(String::from_utf8_lossy(&refused_gpu.stderr).contains("GPU is not enabled by the peer"));
    let link = run("test -L source-link && cat source-link");
    assert!(
        link.status.success(),
        "{}",
        String::from_utf8_lossy(&link.stderr)
    );
    assert_eq!(link.stdout, b"original");
    let tool = run(&format!(
        "'{}' && ! touch '{}/new'",
        tools.join("tool-link").display(),
        tools.display()
    ));
    assert!(
        tool.status.success(),
        "{}",
        String::from_utf8_lossy(&tool.stderr)
    );
    assert_eq!(tool.stdout, b"tool-ok");
    std::fs::remove_file(project.join("source-link")).unwrap();
    std::os::unix::fs::symlink("../outside", project.join("source-link")).unwrap();
    let escaped = run("true");
    assert!(!escaped.status.success());
    assert!(String::from_utf8_lossy(&escaped.stderr).contains("source-link"));
    std::fs::remove_file(project.join("source-link")).unwrap();
    std::os::unix::fs::symlink("source", project.join("source-link")).unwrap();
    let first = run("cat source; printf err >&2; exit 7");
    assert_eq!(
        first.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, b"original");
    assert!(first.stderr.ends_with(b"err"));
    std::fs::write(project.join("source"), "changed").unwrap();
    let second = run("cat source");
    assert_eq!(
        second.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(second.stdout, b"changed");
    let cold_python = run("mkdir -p \"$UV_CACHE_DIR\" \"$PIP_CACHE_DIR\"; printf warm > \"$UV_CACHE_DIR/marker\"; printf warm > .venv/marker");
    assert!(
        cold_python.status.success(),
        "{}",
        String::from_utf8_lossy(&cold_python.stderr)
    );
    let warm_python = run(
        "test \"$(cat \"$UV_CACHE_DIR/marker\")\" = warm && test \"$(cat .venv/marker)\" = warm",
    );
    assert!(
        warm_python.status.success(),
        "{}",
        String::from_utf8_lossy(&warm_python.stderr)
    );
    let environment = run("env");
    assert!(environment.status.success());
    let env_text = String::from_utf8_lossy(&environment.stdout);
    assert!(env_text.contains("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"));
    assert!(env_text.contains("PIP_INDEX_URL=https://mirror.example/simple"));
    assert!(env_text.contains("UV_INDEX_URL=https://mirror.example/simple"));
    assert!(env_text.contains("SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt"));
    assert!(!String::from_utf8_lossy(&environment.stdout).contains(canary));
    assert!(!String::from_utf8_lossy(&environment.stderr).contains(canary));
    for bytes in [&first.stdout, &first.stderr, &second.stdout, &second.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains(canary));
    }
    // Exercise the sandbox-side CLI through a real local broker socket.
    let mut vars = std::collections::BTreeMap::new();
    for (key, dir) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        vars.insert(key.to_string(), temp.path().join(dir).display().to_string());
    }
    vars.insert("PATH".into(), std::env::var("PATH").unwrap());
    let env = paddock::env::HostEnv::from_parts(
        home.clone(),
        "test".into(),
        unsafe { libc::getuid() },
        unsafe { libc::getgid() },
        vars,
    );
    let run_id = "exclave-test";
    paddock::mail::create(&env, run_id, &paddock::mail::Starter::host()).unwrap();
    let context = paddock::broker::Context {
        id: run_id.into(),
        harness: "test".into(),
        project: project.clone(),
        repo: None,
        chain: vec![],
        depth: 0,
        role: None,
    };
    let broker = paddock::broker::Broker::new(
        env.clone(),
        context,
        paddock::broker::HostPolicy::default(),
        std::sync::Arc::new(paddock::broker::DenyAll),
    );
    let socket = temp.path().join("broker.sock");
    paddock::broker::serve(&socket, std::sync::Arc::new(broker)).unwrap();
    let output = Command::new(binary)
        .current_dir(&project)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("PADDOCK_HOST_SOCKET", &socket)
        .env("PADDOCK_DEVICES", temp.path().join("no-devices"))
        .args([
            "host",
            "exclave",
            "run",
            "--on",
            "fake",
            "--network",
            "none",
            "--",
            "/bin/sh",
            "-c",
            "printf broker; exit 9",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(9),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"broker");
    let mail = paddock::mail::read(&env, run_id, paddock::mail::Dir::Up);
    assert!(mail
        .iter()
        .any(|m| m.text.contains("peer fake") && m.text.contains("snapshot")));
    assert!(!format!("{mail:?}").contains(canary));
    // A fixture crate uses the same peer target cache on the second run.
    let fixture = project.join("fixture");
    std::fs::create_dir_all(fixture.join("src")).unwrap();
    std::fs::write(
        fixture.join("Cargo.toml"),
        "[package]\nname = \"peer-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        fixture.join("src/lib.rs"),
        "#[test] fn it_works() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .current_dir(&project)
        .env("HOME", &home)
        .args(["add", "fixture"])
        .status()
        .unwrap()
        .success());
    let cargo = || {
        Command::new(binary)
            .current_dir(&fixture)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .args([
                "exclave",
                "run",
                "--on",
                "fake",
                "--network",
                "none",
                "--",
                "cargo",
                "test",
                "--offline",
                "-v",
            ])
            .output()
            .unwrap()
    };
    let cold = cargo();
    assert!(
        cold.status.success(),
        "cold fixture: {}",
        String::from_utf8_lossy(&cold.stderr)
    );
    let warm = cargo();
    assert!(
        warm.status.success(),
        "warm fixture: {}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stderr).contains("Fresh peer-fixture"),
        "warm fixture: {}",
        String::from_utf8_lossy(&warm.stderr)
    );
    std::fs::write(
        fixture.join("src/lib.rs"),
        "#[test] fn changed() { assert_eq!(3 + 3, 6); }\n",
    )
    .unwrap();
    let changed = cargo();
    assert!(
        changed.status.success(),
        "changed fixture: {}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&changed.stderr).contains("Compiling peer-fixture"),
        "changed fixture did not rebuild: {}",
        String::from_utf8_lossy(&changed.stderr)
    );
}
