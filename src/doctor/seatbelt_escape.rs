//! `paddock doctor --escape --backend seatbelt`: the escape probe in a real
//! Seatbelt sandbox on macOS. `probe.sh` checks mounts, namespaces and
//! /proc, which macOS does not have, so this run has its own probe
//! (`probe_seatbelt.sh`) with the checks that fit: the real home, its
//! secrets, other users, the host /tmp and paddock's state are out of
//! reach, the project is writable, and the network matches the mode.

use super::{doctor_harness, Recorder, Scratch, SCRATCH_DIR};
use crate::build::{self, Options};
use crate::env::HostEnv;
use crate::policy::DENIED_HOME;
use crate::run;
use crate::spec::{GuiMode, Network};
use anyhow::{bail, Result};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

/// The probe of a Seatbelt run.
pub const PROBE: &str = include_str!("probe_seatbelt.sh");

/// Whether `doctor --escape` runs the Seatbelt probe: `--backend seatbelt`,
/// or `auto` on macOS (a terminal run gets Seatbelt there).
pub fn wanted(backend: &str, macos: bool) -> bool {
    backend == crate::backend::seatbelt::NAME || (macos && backend == "auto")
}

/// Why the Seatbelt probe cannot run with these options, or None.
pub fn refusal(o: &super::EscapeOpts, macos: bool) -> Option<String> {
    if !macos {
        return Some(
            "the seatbelt backend runs on macOS only: on Linux use paddock doctor --escape --backend bwrap, srt, podman or docker".into(),
        );
    }
    if o.display != GuiMode::Off {
        return Some(
            "the Seatbelt escape probe is for terminal runs: drop --gui, --window and --app".into(),
        );
    }
    None
}

/// The homes of the other users in `/Users`: not this user's, not Shared.
pub fn other_users(entries: &[String], me: &Path) -> Vec<String> {
    let mine = me.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    entries
        .iter()
        .filter(|n| !n.starts_with('.') && *n != "Shared" && *n != mine)
        .map(|n| format!("/Users/{n}"))
        .collect()
}

/// paddock's own folders that a sandbox must not reach.
pub fn paddock_dirs(env: &HostEnv) -> Vec<PathBuf> {
    vec![
        env.paddock_state(),
        env.paddock_config(),
        env.paddock_data().join("homes"),
    ]
}

/// The probe variables that do not depend on the run.
pub fn probe_vars(
    env: &HostEnv,
    project: &Path,
    users: &[String],
    network: Network,
) -> Vec<(&'static str, String)> {
    let denied: Vec<String> = DENIED_HOME
        .iter()
        .map(|p| env.home.join(p).display().to_string())
        .collect();
    let paddock: Vec<String> = paddock_dirs(env)
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    vec![
        ("PADDOCK_PROBE_PROJECT", project.display().to_string()),
        ("PADDOCK_PROBE_REALHOME", env.home.display().to_string()),
        ("PADDOCK_PROBE_DENIED", denied.join(":")),
        ("PADDOCK_PROBE_USERS", users.join(":")),
        ("PADDOCK_PROBE_PADDOCK", paddock.join(":")),
        (
            "PADDOCK_PROBE_NETWORK",
            format!("{network:?}").to_lowercase(),
        ),
    ]
}

/// Count the result lines of the probe and print them: (seen, failed).
pub fn tally(text: &str) -> (usize, usize) {
    let (mut seen, mut failed) = (0, 0);
    for line in text.lines() {
        if line.starts_with("ok ") || line.starts_with("skip ") {
            seen += 1;
            println!("  {line}");
        } else if line.starts_with("FAIL ") {
            seen += 1;
            failed += 1;
            println!("  {line}");
        }
    }
    (seen, failed)
}

/// Run the probe in a Seatbelt sandbox. Returns the number of failed checks.
pub fn escape(env: &HostEnv, o: &super::EscapeOpts) -> Result<usize> {
    if let Some(why) = refusal(o, cfg!(target_os = "macos")) {
        bail!(why);
    }
    let mut scratch = Scratch::new(&env.paddock_cache().join(SCRATCH_DIR))?;
    crate::storage::remove_doctor_homes(env);
    let root = scratch.root.clone();
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    std::fs::write(project.join(".env"), "TOKEN=paddock-secret-canary\n")?;
    std::fs::write(project.join("README"), "paddock doctor project\n")?;
    // A git project, as `paddock codex` in a repository: it has a borrow folder.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(&project)
        .args(["init", "-q"])
        .status();

    // A file in another sandbox home, one in the host home, one in the host /tmp.
    let other_dir = env
        .paddock_data()
        .join("homes")
        .join("doctor-other")
        .join(root.file_name().unwrap_or_default());
    scratch.also_remove(other_dir.clone());
    crate::privfs::dir(&other_dir)?;
    let other = other_dir.join("canary");
    std::fs::write(&other, "x")?;
    let host_file = root.join("host-canary");
    std::fs::write(&host_file, "x")?;
    let host_tmp = PathBuf::from(format!("/tmp/paddock-doctor-{}", std::process::id()));
    scratch.also_remove(host_tmp.clone());
    std::fs::write(&host_tmp, "x")?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || for _ in listener.incoming() {});

    let opts = Options {
        harness: "doctor".into(),
        project: Some(project.clone()),
        backend: Some(crate::backend::seatbelt::NAME.into()),
        network: Some(o.network),
        gui: Some(GuiMode::Off),
        assume_yes: true,
        inherit_env: Some(o.inherit),
        args: vec!["-c".into(), PROBE.into()],
        allow_domains: if matches!(o.network, Network::Allowlist | Network::Api) {
            vec!["example.com".into()]
        } else {
            vec![]
        },
        ..Default::default()
    };
    let mut built = build::build_with(env, &opts, doctor_harness()?)?;
    scratch.also_remove(built.spec.home_src.clone());
    crate::safefs::Root::open(&built.spec.home_src)?.write(
        Path::new(".paddock-doctor-canary"),
        b"x",
        0o600,
    )?;
    let users: Vec<String> = std::fs::read_dir("/Users")
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let s = &mut built.spec;
    for (k, v) in probe_vars(env, &project, &other_users(&users, &env.home), o.network) {
        s.set_env(k, v, "doctor");
    }
    s.set_env("PADDOCK_PROBE_OTHER", other.display().to_string(), "doctor");
    s.set_env(
        "PADDOCK_PROBE_HOSTFILE",
        host_file.display().to_string(),
        "doctor",
    );
    s.set_env(
        "PADDOCK_PROBE_HOSTTMP",
        host_tmp.display().to_string(),
        "doctor",
    );
    s.set_env("PADDOCK_PROBE_PORT", port.to_string(), "doctor");
    if o.online {
        s.set_env("PADDOCK_PROBE_ONLINE", "1", "doctor");
    }
    if crate::backend::seatbelt::run::developer_dir().is_some() {
        s.set_env("PADDOCK_PROBE_XCRUN", "1", "doctor");
    }
    // Host passthrough requests from the probe are denied.
    built.approver = Some(std::sync::Arc::new(Recorder::default()));
    built.view = false;
    let b = run::select_backend(env, &built)?;
    println!(
        "Escape test: backend {}, network {:?}, env {}",
        b.name(),
        o.network,
        if o.inherit { "inherit" } else { "clean" },
    );
    let outcome = run::run(env, built, b.as_ref(), true, true)?;
    let _ = std::fs::remove_file(&host_tmp);
    let (mut seen, mut failed) = tally(&outcome.stdout.unwrap_or_default());
    // What the probe wrote in the project reached the host.
    seen += 1;
    match std::fs::read_to_string(project.join(".paddock-probe-write")) {
        Ok(t) if t.trim() == "probe" => println!("  ok project-write-reaches-host"),
        _ => {
            failed += 1;
            println!(
                "  FAIL project-write-reaches-host: the host does not see the file the probe wrote"
            );
        }
    }
    if seen == 1 {
        println!(
            "  FAIL probe-ran: the probe printed nothing (exit code {})",
            outcome.code
        );
        failed += 1;
    }
    drop(scratch);
    if let Some(p) = other_dir.parent() {
        let _ = std::fs::remove_dir(p);
    }
    println!(
        "{}",
        if failed == 0 {
            "All checks passed."
        } else {
            "Some checks failed."
        }
    );
    Ok(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::AppBus;

    fn opts(display: GuiMode) -> super::super::EscapeOpts {
        super::super::EscapeOpts {
            backend: "seatbelt".into(),
            network: Network::Egress,
            display,
            online: false,
            inherit: true,
            app_bus: AppBus::Private,
        }
    }

    #[test]
    fn seatbelt_and_auto_on_macos_run_this_probe() {
        assert!(wanted("seatbelt", false));
        assert!(wanted("seatbelt", true));
        assert!(wanted("auto", true));
        assert!(!wanted("auto", false));
        assert!(!wanted("bwrap", false));
        assert!(!wanted("podman", true));
    }

    #[test]
    fn refusals_are_one_line_with_the_command_that_works() {
        let linux = refusal(&opts(GuiMode::Off), false).unwrap();
        assert!(linux.contains("--backend bwrap"), "{linux}");
        assert_eq!(linux.lines().count(), 1);
        assert!(refusal(&opts(GuiMode::App), true).is_some());
        assert_eq!(refusal(&opts(GuiMode::Off), true), None);
    }

    #[test]
    fn other_users_skip_mine_shared_and_dot_files() {
        let names: Vec<String> = ["ana", "bo", "Shared", ".localized", "me"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            other_users(&names, Path::new("/Users/me")),
            vec!["/Users/ana".to_string(), "/Users/bo".to_string()]
        );
    }

    #[test]
    fn the_probe_gets_the_real_home_its_secrets_and_paddock_state() {
        let t = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            t.path().join("me"),
            "me".into(),
            501,
            20,
            Default::default(),
        );
        let vars: std::collections::BTreeMap<_, _> =
            probe_vars(&env, Path::new("/p"), &["/Users/ana".into()], Network::Api)
                .into_iter()
                .collect();
        let home = env.home.display().to_string();
        assert_eq!(vars["PADDOCK_PROBE_REALHOME"], home);
        assert!(vars["PADDOCK_PROBE_DENIED"]
            .split(':')
            .any(|p| p == format!("{home}/.ssh")));
        assert_eq!(vars["PADDOCK_PROBE_USERS"], "/Users/ana");
        for d in paddock_dirs(&env) {
            assert!(
                vars["PADDOCK_PROBE_PADDOCK"]
                    .split(':')
                    .any(|p| Path::new(p) == d),
                "{}",
                d.display()
            );
        }
        assert_eq!(vars["PADDOCK_PROBE_NETWORK"], "api");
    }

    #[test]
    fn the_tally_counts_ok_skip_and_fail() {
        assert_eq!(tally("ok a\nskip b: x\nFAIL c: y\nnoise\n"), (3, 1));
        assert_eq!(tally(""), (0, 0));
    }

    /// The probe covers each point of MACRUN1 finding 4.
    #[test]
    fn the_probe_has_the_checks_of_a_seatbelt_run() {
        for name in [
            "real-home-unreadable",
            "real-home-unwritable",
            "ssh-unreadable",
            "protected-paths-absent",
            "users-unlisted",
            "other-users-unreadable",
            "host-tmp-hidden",
            "host-tmp-unwritable",
            "private-tmpdir",
            "paddock-state-unreadable",
            "paddock-state-unwritable",
            "project-writable",
            "host-loopback-unreachable",
            "direct-egress-blocked",
            "proxy-refuses-host",
            "xcrun-python3",
            "xcrun-git",
            "xcrun-cache-private",
            "xcrun-login-shell",
        ] {
            assert!(PROBE.contains(name), "{name}");
        }
    }

    /// MACRUN1-F2: the login shell check is a real check, never a skip.
    #[test]
    fn the_login_shell_xcrun_check_can_fail_and_never_skips() {
        assert!(!PROBE.contains("skip xcrun-login-shell"));
        assert!(PROBE.contains("xcrun_check xcrun-login-shell /bin/zsh -lc"));
    }

    /// MACRUN1-F2: the probe variables reach the Seatbelt run as host paths,
    /// so the real home it checks is the host home, not the sandbox home.
    #[test]
    fn the_seatbelt_run_gets_the_host_home_as_the_real_home() {
        let env = HostEnv::from_parts(
            PathBuf::from("/Users/me"),
            "me".into(),
            501,
            20,
            Default::default(),
        );
        let mut s = crate::spec::SandboxSpec::new(
            "id".into(),
            "doctor",
            Path::new("/Users/me/Library/Caches/paddock/doctor/project"),
            env.paddock_data().join("homes/doctor/shared/project-1"),
            &env.home,
            501,
            20,
            "me",
        );
        let project = s.project.clone();
        for (k, v) in probe_vars(&env, &project, &[], Network::Egress) {
            assert!(
                k.starts_with(crate::backend::seatbelt::run::HOST_PATH_VAR_PREFIX),
                "{k}"
            );
            s.set_env(k, v, "doctor");
        }
        let tmp = crate::backend::seatbelt::run::private_tmp(&s);
        let vars = crate::backend::seatbelt::run::run_env(&env, &s, &tmp);
        assert_eq!(vars["PADDOCK_PROBE_REALHOME"], "/Users/me");
        assert!(vars["PADDOCK_PROBE_DENIED"]
            .split(':')
            .all(|p| p.starts_with("/Users/me/") && !p.contains("/homes/")));
        assert_eq!(vars["PADDOCK_PROBE_PROJECT"], project.display().to_string());
        assert_ne!(vars["HOME"], "/Users/me");
    }

    /// The probe is valid sh.
    #[test]
    fn the_probe_parses() {
        let ok = std::process::Command::new("/bin/sh")
            .args(["-n", "-c", PROBE])
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }
}
