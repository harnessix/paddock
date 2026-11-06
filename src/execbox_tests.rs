use super::*;

/// A host whose PATH holds only `bwrap`, a script with this body.
#[cfg(not(target_os = "macos"))]
fn fake_bwrap(dir: &Path, body: &str) -> HostEnv {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let f = bin.join("bwrap");
    std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    let vars = [("PATH".to_string(), bin.display().to_string())].into();
    HostEnv::from_parts(dir.join("home"), "demo".into(), 1, 1, vars)
}

#[test]
#[cfg(not(target_os = "macos"))]
fn a_bwrap_that_cannot_start_gives_no_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let e = detect(&fake_bwrap(dir.path(), "exit 1")).unwrap_err();
    let m = format!("{e:#}");
    assert!(m.contains("run a command in isolation"), "{m}");
    assert!(m.contains("cannot start a sandbox"), "{m}");
    let ok = tempfile::tempdir().unwrap();
    assert_eq!(
        detect(&fake_bwrap(ok.path(), "exit 0")).unwrap(),
        Isolation::Bwrap
    );
    // No bwrap at all: the same message, without the probe.
    let none = HostEnv::from_parts("/nowhere".into(), "demo".into(), 1, 1, Default::default());
    let m = format!("{:#}", detect(&none).unwrap_err());
    assert!(
        m.contains("run a command in isolation") && !m.contains("cannot start"),
        "{m}"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn a_mac_isolates_with_seatbelt() {
    let none = HostEnv::from_parts("/nowhere".into(), "demo".into(), 1, 1, Default::default());
    assert_eq!(detect(&none).unwrap(), Isolation::Seatbelt);
}

fn env() -> HostEnv {
    HostEnv::from_parts("/home/demo".into(), "demo".into(), 1, 1, Default::default())
}

#[test]
fn the_default_hides_home_and_keeps_the_network() {
    assert_eq!(ExecCfg::parse(&env(), "").unwrap(), ExecCfg::default());
    let c = ExecCfg::parse(
        &env(),
        "[exec]\nhome = \"read-only\"\nnetwork = false\npaths = [\"~/src/other\"]\nro_paths = [\"/srv/data\"]\nenv = [\"GOPATH\"]\n",
    )
    .unwrap();
    assert_eq!(c.home, Home::ReadOnly);
    assert!(!c.network);
    assert_eq!(c.paths, [PathBuf::from("/home/demo/src/other")]);
    assert_eq!(c.ro_paths, [PathBuf::from("/srv/data")]);
    assert_eq!(c.env, ["GOPATH"]);
    for bad in [
        "[exec]\nhome = \"open\"\n",
        "[exec]\npaths = [\"~\"]\n",
        "[exec]\nro_paths = [\"/\"]\n",
        "[exec]\nenv = [\"A=B\"]\n",
        "[exec]\nshell = true\n",
    ] {
        assert!(ExecCfg::parse(&env(), bad).is_err(), "{bad}");
    }
}

#[test]
fn only_a_few_host_variables_pass() {
    let host: BTreeMap<String, String> = [
        ("PATH", "/usr/bin"),
        ("GITHUB_TOKEN", "secret"),
        ("GOPATH", "/go"),
        ("LANG", "C.UTF-8"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut c = ExecCfg::default();
    let got = c.pass_env(&host);
    assert_eq!(got.keys().collect::<Vec<_>>(), ["LANG", "PATH"]);
    c.env = vec!["GOPATH".into()];
    assert!(c.pass_env(&host).contains_key("GOPATH"));
    assert!(!c.pass_env(&host).contains_key("GITHUB_TOKEN"));
}

#[test]
fn the_systemd_unit_has_the_hardening() {
    let c = command(
        Isolation::Systemd,
        &ExecCfg::default(),
        Path::new("/nowhere/run"),
        Path::new("/home/demo"),
        Path::new("/home/demo/proj"),
        &["make".into(), "test".into()],
        &BTreeMap::new(),
        60,
        "paddock-exec-x",
    )
    .unwrap();
    let args: Vec<String> = c
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for want in [
        "ProtectHome=tmpfs",
        "ProtectSystem=strict",
        "NoNewPrivileges=yes",
        "RestrictNamespaces=yes",
        "BindPaths=/home/demo/proj",
        "WorkingDirectory=/home/demo/proj",
        "RuntimeMaxSec=65",
    ] {
        assert!(args.iter().any(|a| a == want), "{want} missing: {args:?}");
    }
    assert_eq!(&args[args.len() - 3..], ["--", "make", "test"]);
}

fn args_of(c: &Command) -> Vec<String> {
    c.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_token_stays_off_the_systemd_run_command_line() {
    let env: BTreeMap<String, String> = [("GH_TOKEN".to_string(), "ghp_secret".to_string())].into();
    let c = command(
        Isolation::Systemd,
        &ExecCfg::default(),
        Path::new("/nowhere/run"),
        Path::new("/home/demo"),
        Path::new("/home/demo/proj"),
        &["gh".into()],
        &env,
        60,
        "paddock-exec-x",
    )
    .unwrap();
    let a = args_of(&c);
    assert!(a.iter().all(|a| !a.contains("ghp_secret")), "{a:?}");
    assert!(a.iter().any(|a| a == "--setenv=GH_TOKEN"), "{a:?}");
    assert!(c
        .get_envs()
        .any(|(k, v)| k == "GH_TOKEN" && v == Some("ghp_secret".as_ref())));
}

#[test]
fn a_path_with_a_space_stays_one_path_for_systemd() {
    let cfg = ExecCfg {
        home: Home::Full,
        paths: vec![PathBuf::from("/srv/a:b")],
        ..ExecCfg::default()
    };
    let a = args_of(
        &command(
            Isolation::Systemd,
            &cfg,
            Path::new("/nowhere/run"),
            Path::new("/home/demo/my home"),
            Path::new("/home/demo/my home/My Project"),
            &["make".into()],
            &BTreeMap::new(),
            60,
            "paddock-exec-x",
        )
        .unwrap(),
    );
    for want in [
        r"BindPaths=/home/demo/my\ home/My\ Project",
        r"ReadWritePaths=/home/demo/my\ home",
        r"BindPaths=/srv/a\:b",
        "WorkingDirectory=/home/demo/my home/My Project",
    ] {
        assert!(a.iter().any(|a| a == want), "{want} missing: {a:?}");
    }
}

#[test]
fn the_user_session_is_out_of_reach() {
    let cfg = ExecCfg {
        hide: vec![PathBuf::from("/run/user")],
        ..ExecCfg::default()
    };
    let run = |how| {
        args_of(
            &command(
                how,
                &cfg,
                Path::new("/nowhere/run"),
                Path::new("/home/demo"),
                Path::new("/run/user/1000/paddock/proj"),
                &["make".into()],
                &BTreeMap::new(),
                60,
                "paddock-exec-x",
            )
            .unwrap(),
        )
    };
    let a = run(Isolation::Systemd);
    assert!(
        a.iter().any(|a| a == "TemporaryFileSystem=/run/user"),
        "{a:?}"
    );
    let a = run(Isolation::Bwrap);
    assert!(a.iter().any(|a| a == "--disable-userns"), "{a:?}");
    let tmpfs = a.windows(2).position(|w| w == ["--tmpfs", "/run/user"]);
    let bind = a.iter().position(|a| a == "/run/user/1000/paddock/proj");
    assert!(
        tmpfs.is_some() && tmpfs < bind,
        "the project must be bound over the hidden folder: {a:?}"
    );
    // A user whose runtime folder exists hides it by default.
    if Path::new("/run/user").is_dir() {
        assert!(ExecCfg::default()
            .hide
            .contains(&PathBuf::from("/run/user")));
    }
}

/// The real systemd unit, where this host has a systemd user instance:
/// what the command can see and change. Opt in:
/// `cargo test --lib execbox -- --ignored`.
#[test]
#[ignore]
fn the_systemd_unit_isolates_the_command() {
    let host = HostEnv::detect().unwrap();
    if detect(&host).ok() != Some(Isolation::Systemd) {
        eprintln!("skipped: no systemd user instance");
        return;
    }
    let t = tempfile::Builder::new()
        .prefix("paddock-execbox-")
        .tempdir_in(host.home.join(".cache"))
        .unwrap();
    let project = t.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(t.path().join("outside.txt"), "private").unwrap();
    let script = format!(
        "test -e '{}' && echo OUTSIDE_VISIBLE || echo OUTSIDE_HIDDEN; \
         touch /usr/paddock-x 2>/dev/null && echo USR_WRITABLE || echo USR_RO; \
         touch made && echo PROJECT_RW; unshare -U true 2>/dev/null && echo NS_OK || echo NS_BLOCKED; \
         echo TOKEN=${{PADDOCK_TEST_TOKEN:-none}}",
        t.path().join("outside.txt").display()
    );
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), std::env::var("PATH").unwrap());
    let mut cmd = command(
        Isolation::Systemd,
        &ExecCfg::default(),
        Path::new("/nowhere/run"),
        &host.home,
        &project,
        &["sh".into(), "-c".into(), script],
        &env,
        30,
        "paddock-exec-test",
    )
    .unwrap();
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    for want in [
        "OUTSIDE_HIDDEN",
        "USR_RO",
        "PROJECT_RW",
        "NS_BLOCKED",
        "TOKEN=none",
    ] {
        assert!(text.contains(want), "{want} missing:\n{text}");
    }
    let cfg = ExecCfg {
        network: false,
        ..ExecCfg::default()
    };
    let mut cmd = command(
        Isolation::Systemd,
        &cfg,
        Path::new("/nowhere/run"),
        &host.home,
        &project,
        &[
            "sh".into(),
            "-c".into(),
            "tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '".into(),
        ],
        &env,
        30,
        "paddock-exec-test-net",
    )
    .unwrap();
    let out = cmd.output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "lo");
}

#[test]
fn a_bad_seatbelt_profile_is_a_setup_failure() {
    let m = setup_failure(
        Isolation::Seatbelt,
        65,
        "sandbox-exec: unbound variable: x\n",
    )
    .unwrap();
    assert!(
        m.contains("Seatbelt") && m.contains("unbound variable"),
        "{m}"
    );
    // The command's own exit 65, and other exits, are the command's.
    assert!(setup_failure(Isolation::Seatbelt, 65, "make: error\n").is_none());
    assert!(setup_failure(Isolation::Seatbelt, 1, "sandbox-exec: x").is_none());
    assert!(setup_failure(Isolation::Bwrap, 65, "sandbox-exec: x").is_none());
}

#[test]
fn the_seatbelt_command_has_its_profile_home_and_tmp_until_it_ends() {
    let t = tempfile::tempdir().unwrap();
    let real = |p: &Path| std::fs::canonicalize(p).unwrap();
    let run = t.path().join("run");
    let home = t.path().join("home");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let env: BTreeMap<String, String> = [
        ("HOME".to_string(), home.display().to_string()),
        ("LANG".to_string(), "C".to_string()),
    ]
    .into();
    let cfg = ExecCfg {
        hide: Vec::new(),
        ..ExecCfg::default()
    };
    let p = command(
        Isolation::Seatbelt,
        &cfg,
        &run,
        &home,
        &project,
        &["make".into(), "a b".into()],
        &env,
        60,
        "paddock-exec-abc",
    )
    .unwrap();
    let file = run.join("exec/paddock-exec-abc.sb");
    let dir = run.join("exec/paddock-exec-abc");
    let a = args_of(&p);
    assert_eq!(p.get_program(), SANDBOX_EXEC);
    let cwd = project.display().to_string();
    let want = [
        "-f",
        &file.display().to_string(),
        "/bin/sh",
        "-c",
        SEATBELT_SHELL,
        "paddock",
        &cwd,
        "make",
        "a b",
    ];
    assert_eq!(a, want);
    let vars: BTreeMap<String, String> = p
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    assert_eq!(vars["HOME"], real(&dir.join("home")).display().to_string());
    assert_eq!(
        vars["TMPDIR"],
        format!("{}/", real(&dir.join("tmp")).display())
    );
    assert_eq!(vars["LANG"], "C");
    assert!(vars.contains_key("PATH"));
    let profile = std::fs::read_to_string(&file).unwrap();
    assert!(profile.contains(&format!(
        "(allow file-read* file-write* (subpath \"{}\"))",
        real(&project).display()
    )));
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    // A link that the command made in its home is removed, not followed.
    let keep = t.path().join("keep");
    std::fs::write(&keep, "x").unwrap();
    std::os::unix::fs::symlink(t.path(), dir.join("home/link")).unwrap();
    drop(p);
    assert!(!file.exists() && !dir.exists());
    assert!(keep.exists());
    // A unit name cannot leave the folder.
    for bad in ["", "../x", ".hidden"] {
        assert!(command(
            Isolation::Seatbelt,
            &cfg,
            &run,
            &home,
            &project,
            &["true".into()],
            &env,
            60,
            bad,
        )
        .is_err());
    }
}

/// The Seatbelt command starts in its working folder, never in the
/// folder of the executor, which its profile cannot read (getcwd EPERM
/// on a Mac), and PWD names the folder as the caller named it.
#[test]
fn the_seatbelt_command_starts_in_its_working_folder() {
    let t = tempfile::tempdir().unwrap();
    let run = t.path().join("run");
    let home = t.path().join("home");
    let project = t.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let argv = vec!["pwd".to_string()];
    let build = |cwd: &Path| {
        command_in(
            Isolation::Seatbelt,
            &ExecCfg::default(),
            &run,
            &home,
            &project,
            cwd,
            &argv,
            &BTreeMap::new(),
            30,
            "u1",
        )
        .unwrap()
    };
    let p = build(&project);
    assert_eq!(p.get_current_dir(), Some(project.as_path()));
    let pwd = p
        .get_envs()
        .find(|(k, _)| *k == "PWD")
        .and_then(|(_, v)| v)
        .unwrap();
    assert_eq!(pwd, project.as_os_str());
    let args: Vec<_> = p.get_args().collect();
    assert_eq!(args[args.len() - 2], project.as_os_str(), "cd \"$1\"");
    drop(p);
    // A missing folder: the command starts in its own TMPDIR, and `cd`
    // names the folder.
    let gone = t.path().join("gone");
    let p = build(&gone);
    // The real path, as Seatbelt sees it (/private/tmp on a Mac).
    let tmp = std::fs::canonicalize(run.join("exec/u1/tmp")).unwrap();
    assert_eq!(p.get_current_dir(), Some(tmp.as_path()));
}
