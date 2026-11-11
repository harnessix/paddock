#[test]
fn a_request_that_ends_while_the_clock_is_read_is_no_quiet_time() {
    // The clock said 5, then a request ended at 6.
    assert_eq!(quiet(5, 6), 0);
    assert_eq!(quiet(130, 10), 120);
}
use super::*;

#[test]
fn the_socket_is_in_the_runtime_folder_or_the_state_folder() {
    let env = HostEnv::from_parts(
        "/home/demo".into(),
        "u".into(),
        1000,
        1000,
        [("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string())].into(),
    );
    let got = socket_path(&env);
    if cfg!(target_os = "macos") {
        // The same path for launchd and for an executor of paddock.
        assert_eq!(
            got,
            PathBuf::from("/home/demo/.local/state/paddock/gatewayd.sock")
        );
    } else {
        assert_eq!(got, PathBuf::from("/run/user/1000/paddock/gatewayd.sock"));
    }
}

#[test]
fn the_macos_socket_fits_for_a_long_home() {
    // A home of 60 characters.
    let home = format!("/Users/{}", "a".repeat(53));
    assert_eq!(home.len(), 60);
    let env = HostEnv::from_parts(home.into(), "u".into(), 501, 20, Default::default());
    let p = state_socket(&env);
    assert!(p.as_os_str().len() < 103, "{}", p.display());
    assert!(crate::sockpath::check(&p).is_ok());
}

/// Without launchd, the executor binds the state socket itself.
#[test]
#[cfg(target_os = "macos")]
fn without_launchd_the_executor_binds_the_state_socket() {
    let t = tempfile::Builder::new()
        .prefix("pd")
        .tempdir_in("/tmp")
        .unwrap();
    let env = HostEnv::from_parts(t.path().into(), "u".into(), 501, 20, Default::default());
    let l = listener(&env).unwrap().expect("a listener");
    assert_eq!(
        l.local_addr().unwrap().as_pathname(),
        Some(state_socket(&env).as_path())
    );
    // A second executor finds the first one and does not start.
    assert!(listener(&env).unwrap().is_none());
}

#[test]
fn without_isolation_exec_is_refused_not_run() {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
    let key = [3u8; 32];
    let req = Request {
        v: 1,
        cap: "exec".into(),
        args: [("argv".to_string(), serde_json::json!(["touch", "ran"]))]
            .into_iter()
            .collect(),
        reason: String::new(),
    };
    let job = gateway::sign(
        &key,
        "s",
        t.path(),
        &req,
        Default::default(),
        serde_json::Value::Null,
    )
    .unwrap();
    let seen = Seen::default();
    let no = || {
        Err("exec needs a systemd user instance or bwrap to run a command in isolation".to_string())
    };
    let e = handle(&env, no(), &key, &seen, &job).unwrap_err();
    assert!(format!("{e:#}").contains("isolation"), "{e:#}");
    assert!(!t.path().join("ran").exists());
    let e = handle(&env, no(), &key, &seen, &job).unwrap_err();
    assert!(format!("{e:#}").contains("used already"), "{e:#}");
}

/// A push in a real systemd unit (home read-only, only the scratch
/// repository and the local remote writable), where this host has a
/// systemd user instance. Opt in: `cargo test --lib gatewayd -- --ignored`.
#[test]
#[ignore]
fn a_push_runs_in_a_systemd_unit() {
    let host = HostEnv::detect().unwrap();
    if crate::execbox::detect(&host).ok() != Some(crate::execbox::Isolation::Systemd) {
        eprintln!("skipped: no systemd user instance");
        return;
    }
    let t = tempfile::Builder::new()
        .prefix("paddock-push-")
        .tempdir_in(host.home.join(".cache"))
        .unwrap();
    let git = |dir: &Path, args: &[&str]| -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let work = t.path().join("work");
    let remote = t.path().join("remote.git");
    std::fs::create_dir_all(&work).unwrap();
    git(
        t.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let head = git(&work, &["rev-parse", "HEAD"]);
    let repo = crate::broker::git::Repo::snapshot(&work).unwrap();
    let run_dir = t.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let mut args = std::collections::BTreeMap::new();
    args.insert("remote".to_string(), serde_json::json!("origin"));
    args.insert(
        "url".to_string(),
        serde_json::json!(remote.to_str().unwrap()),
    );
    args.insert(
        "refs".to_string(),
        serde_json::json!([{"src": "HEAD", "sha": head, "dst": "refs/heads/main"}]),
    );
    let plan = crate::broker::git::plan(Some(&repo), &run_dir, &args).unwrap();
    let req = Request {
        v: 1,
        cap: "git-push".into(),
        args,
        reason: String::new(),
    };
    let key = [5u8; 32];
    let job = gateway::sign(
        &key,
        "s",
        &work,
        &req,
        host_env(),
        serde_json::to_value(plan.exec()).unwrap(),
    )
    .unwrap();
    let r = handle(
        &host,
        Ok(crate::execbox::Isolation::Systemd),
        &key,
        &Seen::default(),
        &job,
    )
    .unwrap();
    assert_eq!(r["refs"][0]["ok"], true, "{r}");
    assert_eq!(git(&remote, &["rev-parse", "main"]), head);
}

#[test]
fn base64_matches_rfc_4648() {
    for (raw, want) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(raw.as_bytes()), want, "{raw}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn the_active_window_becomes_a_grim_geometry() {
    let j = r#"{"at": [10, 20], "size": [800, 600], "title": "x"}"#;
    assert_eq!(window_geometry(j).unwrap(), "10,20 800x600");
    assert!(window_geometry("{}").is_err());
}

#[test]
#[cfg(target_os = "linux")]
fn a_desktop_tool_sees_the_display_and_nothing_else() {
    let t = tempfile::tempdir().unwrap();
    let rt = t.path().join("run");
    std::fs::create_dir_all(rt.join("hypr/sig")).unwrap();
    std::fs::write(rt.join("wayland-1"), "").unwrap();
    let job = Job {
        grant: gateway::Grant {
            v: 1,
            hash: String::new(),
            cap: "screenshot".into(),
            sandbox: "s".into(),
            project: "/p".into(),
            expires: 0,
            nonce: "n".into(),
        },
        request: Request {
            v: 1,
            cap: "screenshot".into(),
            args: Default::default(),
            reason: String::new(),
        },
        env: [
            ("XDG_RUNTIME_DIR", rt.to_str().unwrap()),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "sig"),
            ("GITHUB_TOKEN", "secret"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        plan: serde_json::Value::Null,
        mac: String::new(),
    };
    let c = desktop_cfg(&job, Path::new("/w"));
    assert_eq!(c.home, crate::execbox::Home::Hidden);
    assert!(!c.network);
    assert_eq!(c.paths, [PathBuf::from("/w")]);
    assert_eq!(c.ro_paths, [rt.join("wayland-1"), rt.join("hypr/sig")]);
    let vars = c.pass_env(&job.env);
    assert!(vars.contains_key("WAYLAND_DISPLAY"));
    assert!(!vars.contains_key("GITHUB_TOKEN"));
}

#[test]
fn the_executor_runs_only_gh_by_an_absolute_path() {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
    let key = [4u8; 32];
    for tool in ["gh", "/usr/bin/curl", "/tmp/gh/../sh"] {
        let plan = crate::broker::forge::ForgeExec {
            tool: tool.into(),
            argv: vec!["--version".into()],
            stdin: None,
            timeout: 5,
            workdir: t.path().into(),
        };
        let req = Request {
            v: 1,
            cap: "forge".into(),
            args: Default::default(),
            reason: String::new(),
        };
        let job = gateway::sign(
            &key,
            "s",
            t.path(),
            &req,
            Default::default(),
            serde_json::to_value(&plan).unwrap(),
        )
        .unwrap();
        let e = handle(
            &env,
            Ok(crate::execbox::Isolation::None),
            &key,
            &Seen::default(),
            &job,
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("does not run"), "{tool}: {e:#}");
    }
}

#[test]
fn only_web_links_reach_open_on_macos() {
    assert!(web_link("https://claude.ai/oauth/authorize?x=1"));
    assert!(web_link("http://localhost:1455/auth/callback"));
    for bad in [
        "file:///etc/passwd",
        "-a Terminal",
        "x-apple.systempreferences:com.apple",
        "HTTPS://example.com",
        "javascript:alert(1)",
        " https://example.com",
    ] {
        assert!(!web_link(bad), "{bad}");
    }
}

/// A forge tool outside the system folders stays runnable under Seatbelt:
/// its folder is read-only in the view. Linux adds nothing.
#[test]
fn the_folder_of_a_forge_tool_is_readable_on_macos_only() {
    let t = tempfile::tempdir().unwrap();
    let gh = t.path().join("gh");
    std::fs::write(&gh, "").unwrap();
    let got = crate::broker::forge::exec_ro_paths(&gh.display().to_string());
    if cfg!(target_os = "macos") {
        assert_eq!(got, vec![t.path().to_path_buf()]);
    } else {
        assert!(got.is_empty());
    }
    assert!(
        crate::broker::forge::exec_ro_paths("/gh").is_empty(),
        "never the root"
    );
    assert!(crate::broker::forge::exec_ro_paths("/no/such/folder/gh").is_empty());
}
