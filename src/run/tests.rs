//! Tests of run.rs.

#[test]
fn no_horse_handoff_preserves_child_pipes() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    crate::horse::live::handoff(None, &mut child);
    child.stdin.take().unwrap().write_all(b"hello\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello\n");
}

mod egress_addr_monitor_tests {
    use super::super::*;

    #[test]
    fn egress_addr_monitor_joins_on_finish() {
        let t = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            t.path().join("host"),
            "demo".into(),
            1,
            1,
            Default::default(),
        );
        let info: crate::state::Running = serde_json::from_value(serde_json::json!({
            "id": "s-test",
            "harness": "h",
            "project": t.path().join("proj"),
            "backend": "bwrap",
            "network": "egress",
            "gui": "off",
            "pid": 1,
            "started": 100,
        }))
        .unwrap();
        let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let monitor = EgressAddrMonitor::start(env, info, found.clone());
        let start = std::time::Instant::now();
        drop(monitor);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}

mod history_sync_tests {
    use super::super::*;
    use crate::profile::Profile;
    use std::collections::BTreeMap;
    use std::time::Instant;

    #[test]
    fn new_history_reaches_the_host_before_the_sandbox_exits() {
        let t = tempfile::tempdir().unwrap();
        let host_home = t.path().join("host");
        let sb = t.path().join("sandbox");
        std::fs::create_dir_all(sb.join(".t/sessions")).unwrap();
        std::fs::create_dir_all(&host_home).unwrap();
        let env = HostEnv::from_parts(host_home.clone(), "demo".into(), 1, 1, BTreeMap::new());
        let p: Profile = toml::from_str(
            "name = \"t\"\n[state.history]\ncopy_back = [\".t/sessions\"]\nfiles = [\"*.jsonl\"]\n",
        )
        .unwrap();
        let history = crate::history::CopyBack::new(&env, &p, &state::AuthMode::Shared, &sb);
        let live = HistorySync::start(Some(history), Duration::ZERO);
        std::fs::write(sb.join(".t/sessions/a.jsonl"), "{}\n").unwrap();
        let host = host_home.join(".t/sessions/a.jsonl");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !host.exists() {
            assert!(
                Instant::now() < deadline,
                "the history did not reach the host"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let (_, copied) = live.finish();
        assert_eq!(copied, 1);
    }
}

mod deliver_tests {
    use super::super::*;

    #[test]
    fn a_file_credential_sets_no_variable() {
        let all = crate::credentials::parse(
            "[credentials.GH_TOKEN]\nvia = \"deliver\"\nas = \"file\"\n\n[credentials.DB]\nvia = \"deliver\"\n",
        )
        .unwrap();
        let home = std::path::Path::new("/home/demo");
        let mut spec = crate::spec::SandboxSpec::new(
            "sb-1".into(),
            "claude",
            std::path::Path::new("/home/demo/app"),
            home.join("sb"),
            home,
            1000,
            1000,
            "t",
        );
        let secrets = crate::credentials::secret::SecretFiles::new(None, None);
        let mut out = RunCredentials::default();
        for (name, value) in [("GH_TOKEN", "ghp_file"), ("DB", "db_env")] {
            let c = &all[name];
            deliver(
                &mut spec,
                &secrets,
                &mut out,
                c,
                name.to_string(),
                c.sandbox_var(name),
                value.into(),
            );
        }
        assert!(!spec.env.contains_key("GH_TOKEN"));
        assert!(!spec.env.values().any(|v| v.value == "ghp_file"));
        assert_eq!(spec.env["DB"].value, "db_env");
        assert_eq!(secrets.check("GH_TOKEN").unwrap().as_str(), "ghp_file");
        assert_eq!(out.secret_files, ["GH_TOKEN"]);
        assert_eq!(out.delivered, ["DB"]);
        assert_eq!(out.marks.len(), 2);
    }
}

mod cleanup_tests {
    use super::super::*;
    use crate::backend::{Backend, Launch};
    use crate::build::{build_with, Options};
    use crate::profile::{Harness, Profile};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    struct MissingProgram;

    impl Backend for MissingProgram {
        fn name(&self) -> &'static str {
            "test"
        }
        fn check(&self, _: &HostEnv, _: &crate::spec::SandboxSpec) -> Result<()> {
            Ok(())
        }
        fn render(
            &self,
            env: &HostEnv,
            spec: &crate::spec::SandboxSpec,
            _: bool,
        ) -> Result<Launch> {
            Ok(Launch {
                program: PathBuf::from("/no-such-paddock-harness"),
                args: vec![],
                env: crate::backend::base_env(env, spec),
                files: vec![],
                cwd: None,
            })
        }
    }

    #[test]
    fn an_early_return_still_removes_the_run_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            home.join(".config/paddock/config.toml"),
            "[devices]\nmount = false\n[kv]\nenabled = false\n",
        )
        .unwrap();
        let mut vars = BTreeMap::new();
        vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, vars);
        // The run starts the view server of this HOME. Stop it at the end.
        let _server = crate::view::guard::TmuxGuard::socket(crate::view::socket(&env));
        let profile: Profile = toml::from_str("name = \"t\"\n").unwrap();
        let h = Harness {
            profile,
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "t".into(),
            project: Some(project),
            assume_yes: true,
            inherit_env: Some(false),
            ..Default::default()
        };
        let built = build_with(&env, &opts, h).unwrap();
        let id = built.spec.id.clone();
        let run_dir = env.paddock_run().join(&id);
        let borrow_dir = crate::borrow::dir(&env, &id);
        let err = match run(&env, built, &MissingProgram, true, true) {
            Ok(_) => panic!("run succeeded"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("cannot start") || err.to_string().contains("No such file"),
            "{err:#}"
        );
        assert!(
            !run_dir.exists(),
            "run folder left behind: {}",
            run_dir.display()
        );
        assert!(
            !borrow_dir.exists(),
            "borrow folder left behind: {}",
            borrow_dir.display()
        );
    }

    /// srt's home rule with a harness that ends at once.
    struct SrtHome;

    impl Backend for SrtHome {
        fn name(&self) -> &'static str {
            "test"
        }
        fn check(&self, _: &HostEnv, _: &crate::spec::SandboxSpec) -> Result<()> {
            Ok(())
        }
        fn harness_home(&self, spec: &crate::spec::SandboxSpec) -> PathBuf {
            crate::backend::srt::Srt.harness_home(spec)
        }
        fn render(
            &self,
            env: &HostEnv,
            spec: &crate::spec::SandboxSpec,
            _: bool,
        ) -> Result<Launch> {
            // `true` is /bin/true on Linux and /usr/bin/true on macOS.
            Ok(Launch {
                program: env.which("true").expect("true on PATH"),
                args: vec![],
                env: crate::backend::base_env(env, spec),
                files: vec![],
                cwd: None,
            })
        }
    }

    /// Review of HOOK1, item a: `run` gives the spec the home that the
    /// backend's harness sees before it writes the sandbox home, so on srt
    /// the Codex hook trust names the sandbox home at its host path. After
    /// the write, the run records delivery through the mail hooks.
    #[test]
    fn a_codex_run_on_srt_trusts_its_hooks_under_the_home_that_codex_sees() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            home.join(".config/paddock/config.toml"),
            "[devices]\nmount = false\n[kv]\nenabled = false\n",
        )
        .unwrap();
        let mut vars = BTreeMap::new();
        vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
        let env = HostEnv::from_parts(home.clone(), "t".into(), 1000, 1000, vars);
        // The run starts the view server of this HOME. Stop it at the end.
        let _server = crate::view::guard::TmuxGuard::socket(crate::view::socket(&env));
        let h = Harness {
            profile: crate::profile::builtin("codex").unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "codex".into(),
            project: Some(project),
            assume_yes: true,
            inherit_env: Some(false),
            ..Default::default()
        };
        let built = build_with(&env, &opts, h).unwrap();
        let id = built.spec.id.clone();
        let home_src = built.spec.home_src.clone();
        assert_ne!(home_src, home);
        let out = run(&env, built, &SrtHome, true, true).unwrap();
        assert_eq!(out.code, 0);
        let cfg = std::fs::read_to_string(home_src.join(".codex/config.toml")).unwrap();
        let doc: toml::Table = toml::from_str(&cfg).unwrap();
        let keys: Vec<&String> = doc["hooks"]["state"].as_table().unwrap().keys().collect();
        let seen = format!("{}/.codex/hooks.json:", home_src.display());
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert!(keys.iter().all(|k| k.starts_with(&seen)), "{keys:?}");
        let info = crate::mail::run_info(&env, &id).expect("the run's mailbox");
        assert_eq!(info.delivery.as_deref(), Some(crate::mail::deliver::HOOKS));
    }
}

#[test]
fn the_start_line_names_the_approvals_mode() {
    use crate::approvals::Approvals;
    use crate::spec::{GuiMode, Network};
    let p = std::path::Path::new("/w/app");
    assert_eq!(
        super::start_line(
            "codex",
            p,
            "seatbelt",
            Network::Egress,
            GuiMode::Off,
            (Some(Approvals::Manual), "default")
        ),
        "codex in /w/app (backend seatbelt, network egress, display off, manual approvals: default)"
    );
    assert!(super::start_line(
        "claude",
        p,
        "bwrap",
        Network::Api,
        GuiMode::Off,
        (Some(Approvals::Auto), "user configuration [run] approvals")
    )
    .ends_with("network api, display off, auto approvals: user configuration [run] approvals)"));
    assert!(super::start_line(
        "claude",
        p,
        "bwrap",
        Network::Api,
        GuiMode::Off,
        (None, "x")
    )
    .ends_with("approvals by the command)"));
}

mod supported_args_tests {
    use super::super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_flag_passes_only_when_the_help_names_it_as_a_whole_word() {
        let help = "Options:\n  --no-daemon  Run without the server\n  [--fast-x]\n";
        let want = args(&["--no-daemon", "--fast", "--fast-x", "plain", "-"]);
        assert_eq!(
            names_in_help(help, &want),
            args(&["--no-daemon", "--fast-x"])
        );
        assert!(names_in_help("--no-daemon-x", &args(&["--no-daemon"])).is_empty());
        assert!(names_in_help("", &args(&["--no-daemon"])).is_empty());
    }

    #[test]
    fn the_probe_reads_the_help_once_and_caches_both_answers() {
        let tmp = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            tmp.path().join("home"),
            "t".into(),
            1000,
            1000,
            Default::default(),
        );
        let runs = tmp.path().join("runs");
        let exe = tmp.path().join("tool");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho x >> {}\necho \"  --no-daemon  no server\"\n",
                runs.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = args(&["--help"]);
        let flag = args(&["--no-daemon"]);
        assert_eq!(supported_args(&env, &exe, &flag, &probe), flag);
        assert_eq!(supported_args(&env, &exe, &flag, &probe), flag);
        let missing = args(&["--no-such"]);
        assert!(supported_args(&env, &exe, &missing, &probe).is_empty());
        assert!(supported_args(&env, &exe, &missing, &probe).is_empty());
        let n = std::fs::read_to_string(&runs).unwrap().lines().count();
        assert_eq!(n, 2, "one help run per set of flags");
        // No entries: no help run at all.
        assert!(supported_args(&env, &exe, &[], &probe).is_empty());
        // A missing executable passes nothing.
        let gone = tmp.path().join("gone");
        assert!(supported_args(&env, &gone, &flag, &probe).is_empty());
    }

    #[test]
    fn the_probe_writes_in_a_scratch_home_and_never_in_the_users_home() {
        // DAEMON1 probes `codex --help` on the host. A harness that writes
        // at start writes in an empty home that paddock removes after it.
        let tmp = tempfile::tempdir().unwrap();
        let seen = tmp.path().join("seen");
        let exe = tmp.path().join("tool");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh
mkdir -p \"$HOME/.codex/sessions\" && echo \"$HOME|${{XDG_STATE_HOME:-unset}}|${{CODEX_HOME:-unset}}\" > {}\necho --no-daemon\n",
                seen.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let text = super::super::help::help_text(&exe, &args(&["--help"])).unwrap();
        assert!(text.contains("--no-daemon"), "{text}");
        let line = std::fs::read_to_string(&seen).unwrap();
        let mut parts = line.trim().split('|');
        let home = std::path::PathBuf::from(parts.next().unwrap());
        assert_eq!(parts.next(), Some("unset"), "{line}");
        assert_eq!(parts.next(), Some("unset"), "{line}");
        assert_ne!(Some(home.as_os_str()), std::env::var_os("HOME").as_deref());
        assert!(home
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("paddock-probe-")));
        assert!(!home.exists(), "the scratch home stays after the probe");
    }
}
