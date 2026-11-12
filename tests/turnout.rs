//! The full command path uses fake systemd and fake cargo in isolated XDG directories.
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn paddock(root: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(args)
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("PATH", path)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .output()
        .unwrap()
}
fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn branch_checks_report_through_the_cli_without_real_systemd() {
    let temp = paddock::view::guard::tempdir();
    let root = temp.path();
    for dir in ["home", "bin", "repo", "config/paddock"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let bin = root.join("bin");
    executable(&bin.join("systemctl"), "#!/bin/sh\nexit 0\n");
    executable(
        &bin.join("systemd-run"),
        "#!/bin/sh\nwhile [ \"${1#--}\" != \"$1\" ]; do shift; done\nexec \"$@\"\n",
    );
    executable(
        &bin.join("cargo"),
        "#!/bin/sh\necho \"test result: ok. 2 passed; 0 failed; jobs=$CARGO_BUILD_JOBS\"\n",
    );
    let repo = root.join("repo");
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["commit", "--allow-empty", "-m", "first"]);
    std::fs::write(root.join("config/paddock/config.toml"), format!("[turnout.host-checks]\nharness='none'\ncheck='echo work'\ntrigger='git'\nproject='{}'\nbranch='main'\nsuites=['first','second']\n[turnout.simple]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n", repo.display())).unwrap();
    assert!(ok(paddock(root, &["turnout", "ls"])).contains("host-checks"));
    ok(paddock(root, &["turnout", "install", "host-checks"]));
    let timer =
        std::fs::read_to_string(root.join("config/systemd/user/paddock-turnout-host-checks.timer"))
            .unwrap();
    assert!(timer.contains("RandomizedDelaySec=60s\nPersistent=true"));
    ok(paddock(root, &["turnout", "run", "host-checks"]));
    let mail = ok(paddock(root, &["mail", "ls", "turnout-host-checks"]));
    assert!(mail.contains("first: passed"), "{mail}");
    assert!(mail.contains("second: passed"), "{mail}");
    assert!(mail.contains("2 passed"), "{mail}");
    let log = ok(paddock(root, &["turnout", "log", "host-checks"]));
    assert!(log.contains("completed"), "{log}");
    ok(paddock(root, &["turnout", "install", "simple"]));
    assert!(root
        .join("config/systemd/user/paddock-turnout-recovery@simple.service")
        .exists());
    let pending = root.join("state/paddock/turnout/pending-simple");
    std::fs::write(&pending, "deferred").unwrap();
    ok(paddock(root, &["turnout", "retry", "simple"]));
    assert!(!pending.exists());
    assert!(ok(paddock(root, &["turnout", "log", "simple"])).contains("completed"));
    ok(paddock(root, &["turnout", "pause"]));
    ok(paddock(root, &["turnout", "run", "host-checks"]));
    assert_eq!(
        ok(paddock(root, &["turnout", "log", "host-checks"]))
            .lines()
            .count(),
        2
    );
    ok(paddock(root, &["turnout", "resume"]));
    ok(paddock(root, &["turnout", "remove", "host-checks"]));
    ok(paddock(root, &["turnout", "remove", "simple"]));
    std::fs::write(
        root.join("paddock.toml"),
        "[turnout.evil]\ncheck = 'echo work'\n",
    )
    .unwrap();
    assert!(!paddock(root, &["turnout", "ls"]).status.success());
    std::fs::remove_file(root.join("paddock.toml")).unwrap();
    std::fs::write(root.join("config/paddock/config.toml"), "not = [valid").unwrap();
    ok(paddock(root, &["turnout", "pause"]));
    assert!(root.join("state/paddock/turnout/paused-all").exists());
}
