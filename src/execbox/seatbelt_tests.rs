//! The executor's Seatbelt profile in the real `sandbox-exec`. These
//! tests run only on macOS.

use super::super::{command, ExecCfg, Home, Isolation};
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Fixture {
    base: tempfile::TempDir,
    run: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

/// A project in the real home, as most projects are.
fn setup() -> Fixture {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let base = tempfile::tempdir_in(&home).unwrap();
    let project = base.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(base.path().join("secret.txt"), "secret").unwrap();
    Fixture {
        run: base.path().join("run"),
        base,
        home,
        project,
    }
}

fn sh(b: &Fixture, cfg: &ExecCfg, script: &str) -> (i32, String, String) {
    let mut env = BTreeMap::new();
    env.insert(
        "PATH".to_string(),
        "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
    );
    let mut p = command(
        Isolation::Seatbelt,
        cfg,
        &b.run,
        &b.home,
        &b.project,
        &["/bin/sh".into(), "-c".into(), script.into()],
        &env,
        30,
        "paddock-exec-test",
    )
    .unwrap();
    crate::broker::run_capture(&mut p, std::time::Duration::from_secs(30)).unwrap()
}

#[test]
fn the_command_writes_the_project_and_sees_an_empty_home() {
    let b = setup();
    let cfg = ExecCfg::default();
    let (code, out, err) = sh(&b, &cfg, "touch made && echo PROJECT_RW; pwd");
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("PROJECT_RW"), "{out}{err}");
    assert!(b.project.join("made").exists());
    // The working folder is the project.
    let pwd = out.lines().last().unwrap_or("");
    assert_eq!(
        std::fs::canonicalize(pwd).unwrap(),
        std::fs::canonicalize(&b.project).unwrap()
    );
    // HOME is an empty folder, and the real home stays closed.
    let (_, out, _) = sh(&b, &cfg, "ls -A \"$HOME\" | wc -l | tr -d ' '");
    assert_eq!(out.trim(), "0");
    let real = b.home.display().to_string();
    let (code, _, _) = sh(&b, &cfg, &format!("ls '{real}'"));
    assert_ne!(code, 0, "the real home was listed");
    let secret = b.base.path().join("secret.txt");
    let (_, out, _) = sh(
        &b,
        &cfg,
        &format!(
            "cat '{}' 2>/dev/null && echo VISIBLE || echo HIDDEN",
            secret.display()
        ),
    );
    assert_eq!(out.trim(), "HIDDEN");
    // TMPDIR is the command's own, and writable.
    let (code, out, err) = sh(&b, &cfg, "touch \"$TMPDIR/x\" && echo TMP_RW");
    assert_eq!(code, 0, "{out}{err}");
    // Its files are gone after the command.
    assert!(!b.run.join("exec/paddock-exec-test").exists());
    assert!(!b.run.join("exec/paddock-exec-test.sb").exists());
}

#[test]
fn a_read_only_home_is_readable_and_not_writable() {
    let b = setup();
    let cfg = ExecCfg {
        home: Home::ReadOnly,
        keychain: true,
        ..ExecCfg::default()
    };
    let secret = b.base.path().join("secret.txt");
    let (_, out, _) = sh(
        &b,
        &cfg,
        &format!(
            "cat '{0}' && echo READ; echo x >> '{0}' 2>/dev/null && echo WROTE || echo RO",
            secret.display()
        ),
    );
    assert!(out.contains("READ") && out.contains("RO"), "{out}");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret");
}

#[test]
fn the_network_resolves_names_and_connects() {
    let b = setup();
    // The Mac has no network: nothing to check.
    let online = std::process::Command::new("/usr/bin/curl")
        .args(["-sI", "-m", "10", "https://example.com"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !online {
        eprintln!("skipped: no network");
        return;
    }
    let (code, out, err) = sh(
        &b,
        &ExecCfg::default(),
        "curl -sI -m 10 https://example.com | head -1",
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.starts_with("HTTP"), "{out}{err}");
    let cfg = ExecCfg {
        network: false,
        ..ExecCfg::default()
    };
    let (code, _, _) = sh(&b, &cfg, "curl -sI -m 5 https://example.com");
    assert_ne!(code, 0, "a command without network connected");
}

#[test]
fn the_profile_applies() {
    // sandbox-exec exits 65 on a profile that it cannot apply: that would be
    // a setup failure, never the command's exit.
    let b = setup();
    let (code, _, err) = sh(&b, &ExecCfg::default(), "exit 0");
    assert_eq!(code, 0, "{err}");
    assert!(crate::execbox::setup_failure(Isolation::Seatbelt, code, &err).is_none());
}
