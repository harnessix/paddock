use super::*;

#[test]
fn starting_a_server_from_project_config_requires_approval() {
    let never: Manifest = toml::from_str("[mcp.servers.s]\nstart = \"never\"\n").unwrap();
    assert!(!never.widens(&Base::default()));
    assert!(never.widening(&Base::default()).get("mcp.start").is_none());
    for mode in ["auto", "always", "lazy"] {
        let m: Manifest =
            toml::from_str(&format!("[mcp.servers.s]\nstart = \"{mode}\"\n")).unwrap();
        assert!(m.widens(&Base::default()));
        assert_eq!(
            m.widening(&Base::default())["mcp.start"]["s"]["start"],
            mode
        );
    }
}

#[test]
fn host_tools_in_a_project_file_are_ignored_with_a_warning() {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
    let cfg = env.paddock_config();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[host.tools.mine]\ncommand = [\"true\"]\n",
    )
    .unwrap();
    let project = t.path().join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(PROJECT_FILE),
        "[host.tools.evil]\ncommand = [\"sh\"]\nargs = [\"**\"]\nmode = \"allow\"\n",
    )
    .unwrap();
    let l = Manifest::load(&env, &project).unwrap();
    let w = l.project.as_ref().unwrap().1.project_warnings();
    assert!(
        w[0].contains("[host.tools] in a project paddock.toml is ignored"),
        "{w:?}"
    );
    let host = l.merged.host.unwrap();
    assert!(host["tools"].get("mine").is_some());
    assert!(host["tools"].get("evil").is_none());
}

#[test]
fn a_project_file_cannot_change_the_training_policy() {
    use crate::training::{policy, Policy};
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
    let project = t.path().join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join(PROJECT_FILE),
        "[privacy]\ntraining = \"vendor\"\n",
    )
    .unwrap();
    let l = Manifest::load(&env, &project).unwrap();
    assert_eq!(policy(&l.merged), Policy::Deny);
    let w = l.project.as_ref().unwrap().1.project_warnings();
    assert!(w.iter().any(|w| w.contains("[privacy]")), "{w:?}");
    let cfg = env.paddock_config();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[privacy]\ntraining = \"manual\"\n",
    )
    .unwrap();
    let l = Manifest::load(&env, &project).unwrap();
    assert_eq!(policy(&l.merged), Policy::Manual);
    // Only the three words.
    assert!(toml::from_str::<Manifest>("[privacy]\ntraining = \"yes\"\n").is_err());
}

#[test]
fn user_only_tables_load_from_the_user_file_and_fail_in_a_project_file() {
    let text = "backend = \"bwrap\"\n[devices]\nwait = true\n[kv]\nenabled = true\n";
    let m = Manifest::parse_user(text).unwrap();
    assert_eq!(m.backend.as_deref(), Some("bwrap"));
    assert!(Manifest::parse_user("[exclave.peers.test]\ntransport = ['canary']\n").is_ok());
    let err =
        Manifest::parse_user("[exclave.peers.test]\ntransport = ['CANARY_SECRET'\n").unwrap_err();
    assert!(!format!("{err:#}").contains("CANARY_SECRET"));
    for k in USER_ONLY {
        let t = format!("[{k}]\nx = 1\n");
        assert!(
            toml::from_str::<Manifest>(&t).is_err(),
            "[{k}] in a project file"
        );
    }
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join(PROJECT_FILE);
    std::fs::write(&p, text).unwrap();
    assert!(Manifest::read(&p).is_err());
    std::fs::write(&p, "[tack.secrets.GH_TOKEN]\n").unwrap();
    let e = format!("{:#}", Manifest::read(&p).unwrap_err());
    assert!(
        e.contains("[tack] is only read from the user configuration"),
        "{e}"
    );
}

#[test]
fn a_planted_project_file_is_never_followed_or_opened() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("p");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(t.path().join("other.toml"), "[mounts]\nrw = [\"/\"]\n").unwrap();
    std::os::unix::fs::symlink(t.path().join("other.toml"), p.join(PROJECT_FILE)).unwrap();
    assert!(Manifest::read(&p.join(PROJECT_FILE)).is_err());
    std::fs::remove_file(p.join(PROJECT_FILE)).unwrap();
    let c = std::ffi::CString::new(p.join(PROJECT_FILE).to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(Manifest::read(&p.join(PROJECT_FILE)).is_err());
}

fn host_env(t: &tempfile::TempDir) -> HostEnv {
    HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default())
}

fn write_user(env: &HostEnv, text: &str) {
    let cfg = env.paddock_config();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), text).unwrap();
}

fn project_with(t: &tempfile::TempDir, text: &str) -> PathBuf {
    let project = t.path().join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(PROJECT_FILE), text).unwrap();
    project
}

#[test]
fn the_template_loads_as_written_and_with_a_device_request() {
    let t = tempfile::tempdir().unwrap();
    let env = host_env(&t);
    let project = project_with(&t, TEMPLATE);
    let l = Manifest::load(&env, &project).unwrap();
    assert!(
        !l.project.as_ref().unwrap().1.widens(&l.base),
        "the template asks for nothing"
    );
    let filled = TEMPLATE.replace("# use = [\"mac\"]", "use = [\"mac\"]");
    assert_ne!(filled, TEMPLATE);
    let project = project_with(&t, &filled);
    let l = Manifest::load(&env, &project).unwrap();
    assert_eq!(l.merged.devices.r#use, ["mac"]);
    assert!(l.project.as_ref().unwrap().1.widens(&l.base));
}

#[test]
fn a_project_file_can_request_a_device_but_never_define_one() {
    let t = tempfile::tempdir().unwrap();
    for (text, names) in [
        (
            "[devices.mac]\nshape = \"exec\"\ncommand = [\"sh\"]\n",
            "[devices.mac]",
        ),
        (
            "[devices]\nuse = [\"mac\"]\necho = true\n",
            "[devices] echo",
        ),
        ("[devices]\nmount = true\n", "[devices] mount"),
    ] {
        let project = project_with(&t, text);
        let e = format!(
            "{:#}",
            Manifest::read(&project.join(PROJECT_FILE)).unwrap_err()
        );
        assert!(e.contains(names), "{e}");
        assert!(
            e.contains("is only read from the user configuration"),
            "{e}"
        );
    }
    // The user configuration keeps its definitions out of the manifest.
    let m =
        Manifest::parse_user("[devices.mac]\nshape = \"exec\"\ncommand = [\"true\"]\n").unwrap();
    assert!(m.devices.r#use.is_empty());
}

#[test]
fn a_device_request_of_a_project_needs_approval_and_then_grants() {
    use crate::devices::grant::{status, GrantCtx, GrantHit};
    let t = tempfile::tempdir().unwrap();
    let env = host_env(&t);
    let user = "[devices.mac]\nshape = \"exec\"\nenabled = true\ncommand = [\"true\"]\ngrant = [\"pasture:workers\"]\n";
    write_user(&env, user);
    let project = project_with(&t, "[devices]\nuse = [\"mac\"]\n");
    let l = Manifest::load(&env, &project).unwrap();
    // Tests have no terminal: the request is refused until approved.
    let e = require_approval(&env, &l, false).unwrap_err().to_string();
    assert!(e.contains("not approved"), "{e}");
    require_approval(&env, &l, true).unwrap();
    require_approval(&env, &l, false).unwrap();
    let def = crate::devices::DevicesCfg::parse(user)
        .unwrap()
        .user
        .remove(0);
    let ctx = GrantCtx {
        project: project.clone(),
        harness: "claude".into(),
        home: env.home.clone(),
        project_use: l.merged.devices.r#use.clone(),
        ..Default::default()
    };
    assert!(matches!(status(&def, &ctx), Ok(GrantHit::Project(_))));
    // Without the request, the device stays out.
    let ctx = GrantCtx {
        project_use: vec![],
        ..ctx
    };
    assert!(status(&def, &ctx).is_err());
}

fn base(mode: Network, allow: &[&str]) -> Base {
    Base {
        mode,
        allow: allow.iter().map(|s| s.to_string()).collect(),
        cli_network: false,
    }
}

fn net(text: &str) -> Manifest {
    toml::from_str(&format!("[network]\n{text}\n")).unwrap()
}

#[test]
fn a_network_mode_wider_than_the_user_mode_asks_and_a_narrower_one_does_not() {
    use Network::*;
    let all = [None, Api, Allowlist, Egress, Host];
    for user in all {
        for project in all {
            let m = net(&format!("mode = \"{}\"", net_name(project)));
            let b = base(user, &[]);
            let wider = net_rank(project) > net_rank(user);
            assert_eq!(
                m.widens(&b),
                wider || project == Host,
                "user {user:?}, project {project:?}"
            );
            if wider && project != Host {
                assert_eq!(m.widening(&b)["network.mode"], net_name(project));
            } else {
                assert!(m.widening(&b).get("network.mode").is_none());
            }
        }
    }
    // An unset user mode is the default, egress.
    assert!(!net("mode = \"egress\"").widens(&Base::default()));
    // A file without the new keys hashes as before.
    let plain = net("publish = [5173]");
    assert!(plain
        .widening(&base(None, &[]))
        .get("network.allow")
        .is_none());
}

#[test]
fn allow_domains_that_the_user_does_not_allow_ask() {
    let b = base(Network::Api, &["crates.io"]);
    assert!(!net("allow = [\"crates.io\"]").widens(&b));
    let m = net("allow = [\"crates.io\", \"evil.example\", \"evil.example\"]");
    assert!(m.widens(&b));
    assert_eq!(
        m.widening(&b)["network.allow"],
        serde_json::json!(["evil.example"])
    );
}

#[test]
fn the_user_configuration_is_the_base_and_an_approved_file_that_changes_asks_again() {
    let t = tempfile::tempdir().unwrap();
    let env = host_env(&t);
    write_user(
        &env,
        "[network]\nmode = \"none\"\nallow = [\"crates.io\"]\n",
    );
    // Narrower or equal: no card.
    let project = project_with(&t, "[network]\nmode = \"none\"\nallow = [\"crates.io\"]\n");
    let l = Manifest::load(&env, &project).unwrap();
    assert_eq!(l.base, base(Network::None, &["crates.io"]));
    require_approval(&env, &l, false).unwrap();
    for wider in [
        "[network]\nmode = \"egress\"\n",
        "[network]\nmode = \"api\"\n",
        "[network]\nallow = [\"pypi.org\"]\n",
    ] {
        let project = project_with(&t, wider);
        let l = Manifest::load(&env, &project).unwrap();
        let e = require_approval(&env, &l, false).unwrap_err().to_string();
        assert!(e.contains("not approved"), "{wider}: {e}");
        require_approval(&env, &l, true).unwrap();
        require_approval(&env, &l, false).unwrap();
    }
    // The approved file changes: it asks again.
    let project = project_with(&t, "[network]\nallow = [\"pypi.org\", \"npmjs.org\"]\n");
    let l = Manifest::load(&env, &project).unwrap();
    assert!(require_approval(&env, &l, false).is_err());
    // The user widens the default: the same file no longer asks.
    write_user(&env, "[network]\nallow = [\"pypi.org\", \"npmjs.org\"]\n");
    let l = Manifest::load(&env, &project).unwrap();
    require_approval(&env, &l, false).unwrap();
}

#[test]
fn a_network_flag_wins_without_a_card() {
    let t = tempfile::tempdir().unwrap();
    let env = host_env(&t);
    write_user(&env, "[network]\nmode = \"none\"\n");
    for mode in ["egress", "host"] {
        let project = project_with(&t, &format!("[network]\nmode = \"{mode}\"\n"));
        let mut l = Manifest::load(&env, &project).unwrap();
        assert!(require_approval(&env, &l, false).is_err());
        l.base.cli_network = true;
        require_approval(&env, &l, false).unwrap();
    }
    // Other widenings still ask with the flag, and an approval without the
    // flag covers a run with it.
    let project = project_with(&t, "[network]\nmode = \"egress\"\npublish = [80]\n");
    let mut l = Manifest::load(&env, &project).unwrap();
    l.base.cli_network = true;
    assert!(require_approval(&env, &l, false).is_err());
    l.base.cli_network = false;
    require_approval(&env, &l, true).unwrap();
    l.base.cli_network = true;
    require_approval(&env, &l, false).unwrap();
}

#[test]
fn network_max_refuses_a_wider_project_approval_and_user_mode() {
    let t = tempfile::tempdir().unwrap();
    let env = host_env(&t);
    let project = project_with(&t, "[network]\nmode = \"egress\"\n");
    write_user(&env, "[network]\nmode = \"none\"\n");
    let l = Manifest::load(&env, &project).unwrap();
    require_approval(&env, &l, true).unwrap();
    // The ceiling comes later: the approval no longer helps.
    write_user(&env, "[network]\nmode = \"none\"\nmax = \"api\"\n");
    let e = format!("{:#}", Manifest::load(&env, &project).err().unwrap());
    assert!(e.contains("network.max = \"api\""), "{e}");
    assert!(e.contains("network.mode = \"egress\""), "{e}");
    // A narrower project loads, and an unset mode stays under the ceiling.
    write_user(&env, "[network]\nmax = \"api\"\n");
    let project = project_with(&t, "[network]\nmode = \"none\"\n");
    let l = Manifest::load(&env, &project).unwrap();
    assert_eq!(l.merged.network.mode, Some(Network::None));
    std::fs::remove_file(project.join(PROJECT_FILE)).unwrap();
    let l = Manifest::load(&env, &project).unwrap();
    assert_eq!(l.merged.network.mode, Some(Network::Api));
    assert_eq!(l.base.mode, Network::Api);
    // The user's own mode above the ceiling is refused too.
    write_user(&env, "[network]\nmode = \"host\"\nmax = \"egress\"\n");
    let e = format!("{:#}", Manifest::load(&env, &project).err().unwrap());
    assert!(e.contains("network.max = \"egress\""), "{e}");
    // A project cannot set a ceiling of its own.
    let project = project_with(&t, "[network]\nmax = \"host\"\n");
    let e = format!(
        "{:#}",
        Manifest::read(&project.join(PROJECT_FILE)).unwrap_err()
    );
    assert!(
        e.contains("network.max is only read from the user configuration"),
        "{e}"
    );
    assert!(check_max(Some(Network::Api), Network::Egress, "--network").is_err());
    assert!(check_max(Some(Network::Api), Network::Api, "--network").is_ok());
    assert!(check_max(Option::None, Network::Host, "--network").is_ok());
}

fn build_env(t: &Path) -> HostEnv {
    let run = t.join("run");
    crate::privfs::dir(&run).unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("HOME".into(), t.join("home").display().to_string());
    vars.insert("XDG_RUNTIME_DIR".into(), run.display().to_string());
    vars.insert(
        "XDG_STATE_HOME".into(),
        t.join("state").display().to_string(),
    );
    vars.insert(
        "XDG_CONFIG_HOME".into(),
        t.join("cfg").display().to_string(),
    );
    vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    HostEnv::from_parts(t.join("home"), "demo".into(), 1000, 1000, vars)
}

fn fake_harness() -> crate::profile::Harness {
    crate::profile::Harness {
        profile: toml::from_str("name = \"fake\"\n").unwrap(),
        exe: PathBuf::from("/bin/sh"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    }
}

#[test]
fn the_network_flag_keeps_its_precedence_and_meets_network_max() {
    use crate::build::{build_with, Options};
    let t = tempfile::tempdir().unwrap();
    let env = build_env(t.path());
    write_user(&env, "[network]\nmode = \"none\"\n");
    let project = project_with(&t, "[network]\nmode = \"egress\"\n");
    let opts = |n: Option<Network>| Options {
        harness: "fake".into(),
        project: Some(project.clone()),
        network: n,
        inherit_env: Some(false),
        ..Default::default()
    };
    // No flag: the wider mode of the file needs a card, and a test has no
    // terminal.
    let e = format!(
        "{:#}",
        build_with(&env, &opts(None), fake_harness()).err().unwrap()
    );
    assert!(e.contains("not approved"), "{e}");
    // The flag wins and needs no card.
    let b = build_with(&env, &opts(Some(Network::Api)), fake_harness()).unwrap();
    assert_eq!(b.spec.network, Network::Api);
    // network.max limits the flag too.
    write_user(&env, "[network]\nmode = \"none\"\nmax = \"api\"\n");
    std::fs::remove_file(project.join(PROJECT_FILE)).unwrap();
    let e = format!(
        "{:#}",
        build_with(&env, &opts(Some(Network::Egress)), fake_harness())
            .err()
            .unwrap()
    );
    assert!(e.contains("network.max = \"api\""), "{e}");
    let b = build_with(&env, &opts(None), fake_harness()).unwrap();
    assert_eq!(b.spec.network, Network::None);
}
