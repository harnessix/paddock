use super::*;

#[test]
fn a_harness_from_a_sandbox_writable_folder_is_refused() {
    let profile: Profile = toml::from_str("name = \"t\"\n").unwrap();
    let mut h = Harness {
        profile,
        exe: PathBuf::from("/usr/bin/tool"),
        install_mounts: vec![PathBuf::from("/home/demo/.local/share/mise/installs/t/1")],
        path_dirs: vec![],
        source: "test".into(),
    };
    let writable = vec![PathBuf::from("/home/demo/proj")];
    assert!(check_harness_install(&h, &writable).is_ok());
    // An interpreter that a project script names: its folder is on PATH.
    h.path_dirs.push(PathBuf::from("/home/demo/proj/bin"));
    assert!(check_harness_install(&h, &writable).is_err());
    h.path_dirs.clear();
    h.exe = PathBuf::from("/home/demo/proj/node_modules/.bin/tool");
    assert!(check_harness_install(&h, &writable).is_err());
}

#[test]
fn a_symlinked_home_is_refused_as_the_project() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(real.join("home")).unwrap();
    std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
    let home = tmp.path().join("link/home");
    let env = HostEnv::from_parts(home.clone(), "t".into(), 1000, 1000, Default::default());
    let h = Harness {
        profile: crate::profile::builtin("codex").unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "codex".into(),
        project: Some(home),
        assume_yes: true,
        ..Default::default()
    };
    let err = build_with(&env, &opts, h).err().expect("refused");
    assert!(err.to_string().contains("home directory"), "{err}");
}

/// Build a spec for a built-in profile with a stand-in executable, a
/// user configuration and a project file.
pub(super) fn privacy_spec(name: &str, user: &str, project: &str) -> (tempfile::TempDir, Built) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    let user = format!("{user}\n[compaction]\nenabled = false\n");
    std::fs::write(home.join(".config/paddock/config.toml"), user).unwrap();
    std::fs::write(proj.join("paddock.toml"), project).unwrap();
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("DISABLE_TELEMETRY".to_string(), String::new());
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, vars);
    let h = Harness {
        profile: crate::profile::builtin(name).unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: name.into(),
        project: Some(proj),
        assume_yes: true,
        ..Default::default()
    };
    let built = build_with(&env, &opts, h).unwrap();
    (tmp, built)
}

/// A Claude Code build in a fresh home, with a user configuration and
/// a project file.
fn claude_with(
    user: &str,
    project_file: Option<&str>,
    approvals: Option<crate::approvals::Approvals>,
    assume_yes: bool,
) -> (tempfile::TempDir, PathBuf, Result<Built>) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(env.paddock_config().join("config.toml"), user).unwrap();
    if let Some(f) = project_file {
        std::fs::write(proj.join(manifest::PROJECT_FILE), f).unwrap();
    }
    let h = Harness {
        profile: crate::profile::builtin("claude").unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "claude".into(),
        project: Some(proj.clone()),
        assume_yes,
        approvals,
        ..Default::default()
    };
    let b = build_with(&env, &opts, h);
    (tmp, proj, b)
}

/// Manual approvals: the same sandbox, but no auto-approval flag, no
/// bypass answer and no trust answer, so Claude Code asks as usual.
#[test]
fn manual_approvals_leave_the_agents_own_questions_alone() {
    use crate::approvals::Approvals;
    let flag = "--dangerously-skip-permissions".to_string();
    let (ta, _p, auto) = claude_with("", None, None, true);
    let auto = auto.unwrap();
    let (tm, _p, manual) = claude_with("", None, Some(Approvals::Manual), true);
    let manual = manual.unwrap();
    assert_eq!(auto.approvals, Some(Approvals::Auto));
    // A user configuration and no marker: a current installation keeps auto.
    assert_eq!(auto.approvals_origin, crate::approvals::CURRENT);
    assert!(auto.spec.command.contains(&flag));
    assert!(auto
        .spec
        .home_settings
        .iter()
        .any(|s| s.file == Path::new(".claude.json")));
    assert_eq!(manual.approvals, Some(Approvals::Manual));
    assert_eq!(manual.approvals_origin, "command line");
    assert!(manual.manual());
    assert!(!manual.spec.command.contains(&flag));
    assert!(
        !manual.spec.home_settings.iter().any(|s| {
            s.path
                .iter()
                .any(|k| k == "hasTrustDialogAccepted" || k == "skipDangerousModePermissionPrompt")
        }),
        "{:?}",
        manual.spec.home_settings
    );
    // The sandbox itself does not change: the same network and the
    // same mounts, relative to each test's own folder.
    assert_eq!(auto.spec.network, manual.spec.network);
    let dsts = |b: &Built, root: &Path| -> Vec<String> {
        let root = root.display().to_string();
        b.spec
            .mounts
            .iter()
            .map(|m| m.dst.display().to_string().replace(&root, "ROOT"))
            .collect()
    };
    assert_eq!(dsts(&auto, ta.path()), dsts(&manual, tm.path()));
}

#[test]
fn approvals_come_from_flag_project_agent_then_user() {
    use crate::approvals::Approvals::{Auto, Manual};
    let user = "[run]\napprovals = \"manual\"\n";
    let (_t, _p, b) = claude_with(user, None, None, false);
    let b = b.unwrap();
    assert_eq!(b.approvals, Some(Manual));
    assert_eq!(b.approvals_origin, "user configuration [run] approvals");
    let agent = "[run]\napprovals = \"manual\"\n[run.agents.claude]\napprovals = \"auto\"\n";
    let (_t, _p, b) = claude_with(agent, None, None, false);
    assert_eq!(b.unwrap().approvals, Some(Auto));
    let other = "[run.agents.codex]\napprovals = \"manual\"\n";
    let (_t, _p, b) = claude_with(other, None, None, false);
    assert_eq!(b.unwrap().approvals, Some(Auto));
    // A project file that asks for manual needs no approval.
    let proj = "[run]\napprovals = \"manual\"\n";
    let (_t, _p, b) = claude_with(agent, Some(proj), None, false);
    let b = b.unwrap();
    assert_eq!(b.approvals, Some(Manual));
    assert_eq!(b.approvals_origin, "paddock.toml [run] approvals");
    // `--auto` wins over all of them.
    let (_t, _p, b) = claude_with(user, Some(proj), Some(Auto), false);
    assert_eq!(b.unwrap().approvals, Some(Auto));
}

/// The agent can edit the project file: `approvals = "auto"` there
/// needs the one-time approval, and `[run.agents]` is refused.
#[test]
fn a_project_file_needs_approval_for_auto_approvals() {
    use crate::approvals::Approvals::{Auto, Manual};
    let user = "[run]\napprovals = \"manual\"\n";
    let proj = "[run]\napprovals = \"auto\"\n";
    let (_t, _p, b) = claude_with(user, Some(proj), None, false);
    let e = format!("{:#}", b.err().expect("not approved"));
    assert!(e.contains("not approved"), "{e}");
    let (_t, _p, b) = claude_with(user, Some(proj), None, true);
    let b = b.unwrap();
    assert_eq!(b.approvals, Some(Auto));
    let (_t, _p, b) = claude_with(user, Some(proj), Some(Manual), true);
    assert_eq!(b.unwrap().approvals, Some(Manual));
    let agents = "[run.agents.claude]\napprovals = \"auto\"\n";
    let (_t, _p, b) = claude_with("", Some(agents), None, true);
    let e = format!("{:#}", b.err().expect("refused"));
    assert!(
        e.contains("[run.agents] is only read from the user configuration"),
        "{e}"
    );
}

/// A detached Claude Code run has no terminal to answer its own two
/// questions: the bypass warning (only with auto-approval) and the
/// project trust question.
#[test]
fn claude_starts_without_its_trust_and_bypass_questions() {
    let built = |no_yolo: bool| {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
        let h = Harness {
            profile: crate::profile::builtin("claude").unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "claude".into(),
            project: Some(proj.clone()),
            assume_yes: true,
            no_yolo,
            // A fresh home is a new installation (manual): ask for auto.
            approvals: Some(crate::approvals::Approvals::Auto),
            ..Default::default()
        };
        let b = build_with(&env, &opts, h).unwrap();
        (tmp, proj, b)
    };
    let has = |b: &Built, file: &str, path: &[String]| {
        b.spec.home_settings.iter().any(|s| {
            s.file == Path::new(file) && s.path == path && s.value == serde_json::json!(true)
        })
    };
    let bypass = vec!["skipDangerousModePermissionPrompt".to_string()];
    let (_t, proj, b) = built(false); // the path getcwd sees
    let trust = vec![
        "projects".to_string(),
        std::fs::canonicalize(&proj).unwrap().display().to_string(),
        "hasTrustDialogAccepted".to_string(),
    ];
    assert!(has(&b, ".claude/settings.json", &bypass));
    assert!(has(&b, ".claude.json", &trust));
    let (_t, proj, b) = built(true);
    let trust = vec![
        "projects".to_string(),
        std::fs::canonicalize(&proj).unwrap().display().to_string(),
        "hasTrustDialogAccepted".to_string(),
    ];
    assert!(
        !has(&b, ".claude/settings.json", &bypass),
        "no bypass answer with --no-yolo"
    );
    assert!(has(&b, ".claude.json", &trust));
    // settings.json is a writable copy now, so the answer can be written.
    let p = crate::profile::builtin("claude").unwrap();
    assert!(p.state.seed.iter().any(|s| s == ".claude/settings.json"));
    assert!(!p
        .state
        .import_ro
        .iter()
        .any(|s| s == ".claude/settings.json"));
}

#[test]
fn the_harness_folder_is_on_the_sandbox_path() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let tools = tmp.path().join("tools/codex/1.0/bin");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&tools).unwrap();
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
    let h = Harness {
        profile: crate::profile::builtin("codex").unwrap(),
        exe: tools.join("codex"),
        install_mounts: vec![tools.parent().unwrap().to_path_buf()],
        path_dirs: vec![],
        source: "test".into(),
    };
    for shell in [false, true] {
        let opts = Options {
            harness: "codex".into(),
            project: Some(proj.clone()),
            assume_yes: true,
            shell,
            ..Default::default()
        };
        let b = build_with(&env, &opts, h.clone()).unwrap();
        let path = b.spec.path_value();
        let dirs: Vec<&str> = path.split(':').collect();
        let at = dirs.iter().position(|d| Path::new(d) == tools);
        assert!(at.is_some(), "shell={shell}: {path}");
        // Never before paddock's own tools (gh, git) or the system tools.
        let usr_bin = dirs.iter().position(|d| *d == "/usr/bin").unwrap();
        assert!(at.unwrap() > usr_bin, "{path}");
    }
    // A harness in /usr/bin changes nothing: no second /usr/bin, and
    // the mise shims and paddock's tools stay before it.
    let opts = Options {
        harness: "codex".into(),
        project: Some(proj.clone()),
        assume_yes: true,
        ..Default::default()
    };
    let before = build_with(&env, &opts, h.clone())
        .unwrap()
        .spec
        .path_value();
    let sys = Harness {
        exe: PathBuf::from("/usr/bin/codex"),
        ..h.clone()
    };
    let after = build_with(&env, &opts, sys).unwrap().spec.path_value();
    assert_eq!(after.matches(":/usr/bin:").count(), 1, "{after}");
    assert_eq!(
        after,
        before.replace(&format!(":{}", tools.display()), ""),
        "{after}"
    );
}

#[test]
fn a_pasture_member_gets_the_shared_folder() {
    let (t, b) = privacy_spec("claude", "", "");
    assert!(b.pasture.is_none());
    assert!(!b.spec.mounts.iter().any(|m| m.dst == Path::new("/pasture")));
    assert!(!b.spec.env.contains_key("PADDOCK_PASTURE"));
    drop(t);
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
    let h = Harness {
        profile: crate::profile::builtin("claude").unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "claude".into(),
        project: Some(proj.clone()),
        assume_yes: true,
        pasture: Some("team".into()),
        ..Default::default()
    };
    let b = build_with(&env, &opts, h).unwrap();
    let m = b
        .spec
        .mounts
        .iter()
        .find(|m| m.dst == Path::new("/pasture"))
        .expect("no /pasture mount");
    assert_eq!(m.kind, crate::spec::MountKind::Rw);
    let ground = crate::pasture::ground(&env, &b.home_key, "team");
    assert_eq!(m.src.as_deref(), Some(ground.as_path()));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&ground).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(b.spec.env["PADDOCK_PASTURE"].value, "/pasture");
    assert_eq!(b.spec.env["PADDOCK_PASTURE_NAME"].value, "team");
    // The workbench folder: read-only for a member.
    let w = b
        .spec
        .mounts
        .iter()
        .find(|m| m.dst == Path::new("/run/paddock/workbench"))
        .expect("no workbench mount");
    assert_eq!(w.kind, crate::spec::MountKind::Ro);
    assert_eq!(
        w.src.as_deref(),
        Some(crate::pasture::workbench::dir(&env, &b.home_key, "team").as_path())
    );
    assert_eq!(
        b.spec.env["PADDOCK_WORKBENCH"].value,
        "/run/paddock/workbench"
    );
    // Port sockets: the tree is read-only, this member's folder is not.
    let ports = crate::pasture::ports::host_dir(&env, &b.home_key, "team");
    let tree = b
        .spec
        .mounts
        .iter()
        .find(|m| m.dst == Path::new(crate::pasture::ports::PORTS_MOUNT))
        .expect("no ports mount");
    assert_eq!(tree.kind, crate::spec::MountKind::Ro);
    assert_eq!(tree.src.as_deref(), Some(ports.as_path()));
    let own = crate::pasture::ports::member_dir(&ports, &b.spec.id).unwrap();
    let mine = b
        .spec
        .mounts
        .iter()
        .find(|m| m.dst == Path::new(crate::pasture::ports::OWN_MOUNT))
        .expect("no own ports mount");
    assert_eq!(mine.kind, crate::spec::MountKind::Rw);
    assert_eq!(mine.src.as_deref(), Some(own.as_path()));
    assert_eq!(
        b.spec.env[crate::pasture::ports::PORTS_ENV].value,
        crate::pasture::ports::PORTS_MOUNT
    );
    assert_eq!(
        b.spec.env[crate::pasture::ports::OWN_ENV].value,
        crate::pasture::ports::OWN_MOUNT
    );
    assert!(!ports.starts_with(&ground));
    // The admission check of `build` comes before this: a configured
    // member only.
    assert!(build(&env, &opts).is_err());
}

#[test]
fn profile_ports_settings_and_host_notices_reach_the_spec() {
    // dsh serves its UI to the host browser on one loopback port.
    let (_t, b) = privacy_spec("dsh", "", "");
    assert_eq!(b.spec.publish, vec![3080]);
    let (_t, b) = privacy_spec("dsh", "[network]\nmode = \"none\"\n", "");
    assert!(b.spec.publish.is_empty());
    // A self-update switch goes into the sandbox copy of the settings.
    let (_t, b) = privacy_spec("devin", "", "");
    assert!(b
        .spec
        .home_settings
        .iter()
        .any(|s| s.path == ["auto_update"]
            && s.value == serde_json::json!(false)
            && s.origin.contains("state.settings")));
    // muse keeps its own sandbox: `paddock muse` says what that means.
    let (_t, b) = privacy_spec("muse", "", "");
    assert!(b.spec.warnings.iter().any(|w| w.contains("own sandbox")));
}

#[test]
fn pure_caches_are_tmpfs_in_the_spec_unless_the_user_turns_them_off() {
    use crate::spec::MountKind;
    let tmpfs = |b: &Built| -> Vec<PathBuf> {
        b.spec
            .mounts
            .iter()
            .filter(|m| m.kind == MountKind::Tmpfs)
            .filter_map(|m| m.dst.strip_prefix(&b.spec.home).ok())
            .map(Path::to_path_buf)
            .collect()
    };
    let (_t, b) = privacy_spec("codex", "", "");
    let on = tmpfs(&b);
    for p in [
        ".cache/mesa_shader_cache",
        ".cache/fontconfig",
        ".codex/cache",
    ] {
        assert!(on.contains(&PathBuf::from(p)), "{p}: {on:?}");
    }
    // Paths only known after the app ran are for gc, never a mount.
    assert!(!on.iter().any(|p| p.to_string_lossy().contains('*')));
    // Never a login or a settings file.
    assert!(!on.iter().any(|p| p.starts_with(".codex/auth.json")));
    let (_t, b) = privacy_spec("claude", "", "");
    assert!(!tmpfs(&b).contains(&PathBuf::from(".codex/cache")));
    let (_t, b) = privacy_spec("codex", "[storage]\ntmpfs_caches = false\n", "");
    assert!(tmpfs(&b).is_empty(), "{:?}", tmpfs(&b));
}

#[test]
fn an_npm_harness_shares_the_npm_cache_without_a_package_json() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let pkg = tmp.path().join("tools/lib/node_modules/fake-agent");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(pkg.join("bin")).unwrap();
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
    let profile: Profile = toml::from_str("name = \"fake\"\n").unwrap();
    let opts = Options {
        harness: "fake".into(),
        project: Some(proj.clone()),
        assume_yes: true,
        ..Default::default()
    };
    let npm = |exe: PathBuf| {
        let h = Harness {
            profile: profile.clone(),
            exe,
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let b = build_with(&env, &opts, h).unwrap();
        let dst = b.spec.home.join(".npm");
        b.spec
            .mounts
            .iter()
            .find(|m| m.dst == dst)
            .and_then(|m| m.src.clone())
    };
    let shared = npm(pkg.join("bin/agent.js")).expect("an npm harness gets .npm");
    assert!(shared.ends_with("npm"), "{}", shared.display());
    assert!(shared.starts_with(crate::storage::package_cache::root(&env)));
    assert_eq!(npm(PathBuf::from("/usr/bin/true")), None);
}

#[test]
fn dates_for_branch_names() {
    assert_eq!(ymd(0), "19700101");
    assert_eq!(ymd(951_782_400), "20000229");
    assert_eq!(ymd(4_102_444_799), "20991231");
}

#[test]
fn an_app_worktree_has_its_own_branch_and_report() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir_all(&repo).unwrap();
    let g = |dir: &Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    g(&repo, &["init", "-q", "-b", "main"]);
    g(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let repo = std::fs::canonicalize(&repo).unwrap();
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "XDG_DATA_HOME".to_string(),
        tmp.path().join("data").display().to_string(),
    );
    // PATH so that the sandboxed work-tree check can find bwrap and git.
    if let Ok(p) = std::env::var("PATH") {
        vars.insert("PATH".to_string(), p);
    }
    let env = HostEnv::from_parts(tmp.path().join("home"), "t".into(), 1000, 1000, vars);
    let w = create_worktree(&env, &repo, Some("vscode")).unwrap();
    assert!(w
        .branch
        .starts_with(&format!("paddock/vscode-{}-", worktree::today())));
    assert!(w
        .path
        .starts_with(tmp.path().join("data/paddock/worktrees")));
    assert!(worktree_report(&env, &w).contains("made no changes"));
    g(
        &w.path,
        &["commit", "-q", "--allow-empty", "-m", "work in the app"],
    );
    std::fs::write(w.path.join("new.txt"), "x").unwrap();
    let r = worktree_report(&env, &w);
    assert!(r.contains("work in the app"), "{r}");
    // The worktree state comes from a sandboxed `git status`. A host that
    // cannot run one reports it as unknown instead of guessing.
    let (commits, changed) = worktree_counts(&env, &w);
    assert_eq!(commits, 1);
    match changed {
        Some(n) => {
            assert_eq!(n, 1);
            assert!(r.contains("?? new.txt"), "{r}");
        }
        None => assert!(r.contains("cannot read the worktree state") || r.contains("commits")),
    }
    // The checkout itself stays on its branch.
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main");
}

#[test]
fn a_failed_build_does_not_leave_a_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir_all(&repo).unwrap();
    let g = |dir: &Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    g(&repo, &["init", "-q", "-b", "main"]);
    g(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let missing = tmp.path().join("no-such-mount");
    std::fs::write(
        repo.join("paddock.toml"),
        format!("[mounts]\nro = [\"{}\"]\n", missing.display()),
    )
    .unwrap();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "XDG_DATA_HOME".to_string(),
        tmp.path().join("data").display().to_string(),
    );
    if let Ok(p) = std::env::var("PATH") {
        vars.insert("PATH".to_string(), p);
    }
    let env = HostEnv::from_parts(tmp.path().join("home"), "t".into(), 1000, 1000, vars);
    let h = Harness {
        profile: crate::profile::builtin("claude").unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "claude".into(),
        project: Some(repo.clone()),
        assume_yes: true,
        worktree: true,
        ..Default::default()
    };
    let Err(err) = build_with(&env, &opts, h) else {
        panic!("build succeeded");
    };
    assert!(err.to_string().contains("does not exist"), "{err:#}");
    let listed = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        !listed.contains("paddock/wt-"),
        "leftover worktree:\n{listed}"
    );
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list", "paddock/wt-*"])
        .output()
        .unwrap();
    let branches = String::from_utf8_lossy(&branches.stdout);
    assert!(branches.trim().is_empty(), "leftover branch:\n{branches}");
}

/// A codex build with a stand-in executable whose `--help` prints `help`,
/// and the user arguments `args`.
fn codex_argv(help: &str, args: &[&str]) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        home.join(".config/paddock/config.toml"),
        "[compaction]\nenabled = false\n",
    )
    .unwrap();
    let exe = bin.join("codex");
    std::fs::write(&exe, format!("#!/bin/sh\ncat <<'END'\n{help}\nEND\n")).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
    let h = Harness {
        profile: crate::profile::builtin("codex").unwrap(),
        exe: exe.clone(),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "codex".into(),
        project: Some(proj),
        assume_yes: true,
        args: args.iter().map(|a| a.to_string()).collect(),
        ..Default::default()
    };
    let mut c = build_with(&env, &opts, h).unwrap().spec.command;
    assert_eq!(c.remove(0), exe.display().to_string());
    c
}

const CODEX_HELP: &str = "Usage: codex [OPTIONS] [PROMPT]\n      --no-alt-screen  Disable alternate screen mode\n      --no-daemon  Run without the shared background server, even if it is already running\n";

#[test]
fn codex_starts_without_its_daemon_when_the_help_names_the_flag() {
    let flag = "--no-daemon".to_string();
    // The start, resume, fork and exec: the flag is a root option, before
    // the subcommand and its arguments.
    for args in [
        vec![],
        vec!["resume", "--last"],
        vec!["fork", "--last"],
        vec!["exec", "hi"],
    ] {
        let c = codex_argv(CODEX_HELP, &args);
        assert_eq!(c.iter().filter(|a| **a == flag).count(), 1, "{c:?}");
        let at = c.iter().position(|a| *a == flag).unwrap();
        let user: Vec<&str> = c[at + 1..].iter().map(String::as_str).collect();
        assert_eq!(user, args, "{c:?}");
    }
    // An older codex without the flag starts as before.
    let old = CODEX_HELP.replace("--no-daemon", "--no-other");
    for args in [vec![], vec!["resume", "--last"], vec!["exec", "hi"]] {
        let c = codex_argv(&old, &args);
        assert!(!c.contains(&flag), "{c:?}");
    }
    // The one-shot command never gets it.
    let p = crate::profile::builtin("codex").unwrap();
    let o = p.oneshot.as_ref().unwrap();
    assert_eq!(o.args[0], "exec");
    assert!(!o.render(Path::new("/p")).contains(&flag));
    assert_eq!(p.launch.args_if_supported, vec![flag]);
}
