//! Context transforms of a sandbox: `paddock horse grant` on the host, and
//! a granted sandbox that forks its own session through the device file.

use super::{cmd, paddock};
#[cfg(target_os = "linux")]
use super::{host_store, run_dsh, sandbox_setup, sandbox_works};

#[test]
fn grant_list_and_revoke_on_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let out = paddock(root, &["grant", "sbx-demo", "--allow", "fork,append"]);
    assert!(
        out.starts_with(
            "sandbox sbx-demo: fork, append; 20 of 20 commands left; 0 of 0 tokens left"
        ),
        "{out}"
    );
    assert!(out.contains("--tokens N"), "{out}");
    let out = paddock(
        root,
        &[
            "grant",
            "sbx-demo",
            "--allow",
            "view",
            "--commands",
            "3",
            "--tokens",
            "500",
        ],
    );
    assert!(
        out.contains("view; 3 of 3 commands left; 500 of 500 tokens left"),
        "{out}"
    );
    assert_eq!(paddock(root, &["grants"]).lines().count(), 1);
    // A move needs a target horse, and a command must be known.
    for args in [
        &["grant", "sbx-demo", "--allow", "move"][..],
        &["grant", "sbx-demo", "--allow", "rewrite"][..],
        &["grant", "../x"][..],
    ] {
        let o = cmd(root, args).output().unwrap();
        assert!(!o.status.success(), "{args:?}");
    }
    assert_eq!(
        paddock(root, &["grant", "sbx-demo", "--revoke"]),
        "the grants of sandbox sbx-demo are gone\n"
    );
    assert_eq!(paddock(root, &["grants"]), "no ctl grant\n");
}

/// The fake dsh of the sandbox test, on the host with a stand-in for the
/// device: a `self/ctl` that refuses writes, as the device refuses an
/// ungranted command. The error text that the test reads must not depend
/// on the language of the user.
#[test]
fn the_fake_of_the_sandbox_test_reports_a_refusal_in_any_language() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root writes a read-only file");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dev = root.join("dev");
    let bin = root.join("bin");
    let work = root.join("work");
    for d in [&dev.join("self/sessions/s-demo"), &bin, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        dev.join("self/sessions/s-demo/binding"),
        r#"{"native_id":"session-demo"}"#,
    )
    .unwrap();
    let ctl = dev.join("self/ctl");
    std::fs::write(&ctl, "").unwrap();
    std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o400)).unwrap();
    let host = bin.join("paddock-host");
    std::fs::write(&host, "#!/bin/sh\ncase \"$1\" in horse) echo s-demo;; peek) echo '{\"native_id\":\"session-demo\"}';; esac\n").unwrap();
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(work.join("granted"), "").unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut child = Command::new("sh")
        .arg(super::fixture("dsh-ctl"))
        .args(["--profile", "sdk"])
        .env("PATH", path)
        .env("FAKE_HORSE_DEV", &dev)
        .env("LANG", "zh_CN.UTF-8")
        .env("LANGUAGE", "zh_CN:de_DE:fr_FR")
        .env("LC_MESSAGES", "zh_CN.UTF-8")
        .current_dir(&work)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize"}}"#).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    // The stand-in also refuses the second write.
    assert!(!out.status.success(), "{out:?}");
    let read = |name: &str| std::fs::read_to_string(work.join(name)).unwrap();
    assert_eq!(read("ungranted.txt"), "refused\n");
    for name in ["ungranted-error.txt", "granted-error.txt"] {
        let why = read(name);
        assert!(why.contains("ermission denied"), "{name}: {why}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn a_granted_sandbox_forks_its_own_session_through_the_device() {
    use paddock::horse::live;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = sandbox_setup("dsh-ctl");
    let root = tmp.path().to_path_buf();
    let run = {
        let root = root.clone();
        std::thread::spawn(move || run_dsh(&root))
    };
    let ready = root.join("project/sessions.txt");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let sid = std::fs::read_to_string(&ready).expect("the fake session did not start");
    let sid = sid.trim().to_string();
    let store = host_store(&root);
    // The sandbox id is the name of the live record of the run.
    let sandbox = live::records(&store)
        .unwrap()
        .into_iter()
        .find(|r| r.sessions.iter().any(|s| s.session == sid))
        .expect("no live record of the run")
        .run;
    paddock(
        &root,
        &["grant", &sandbox, "--allow", "fork", "--commands", "1"],
    );
    std::fs::write(root.join("project/granted"), "").unwrap();
    let out = run.join().unwrap();
    let read = |name: &str| std::fs::read_to_string(root.join("project").join(name));
    assert!(
        out.status.success(),
        "{out:?}\nerror of the granted write: {:?}\nanswer of the device: {:?}",
        read("granted-error.txt"),
        read("granted-answer.txt"),
    );
    assert_eq!(read("ungranted.txt").unwrap(), "refused\n");
    let why = read("ungranted-error.txt").unwrap();
    assert!(why.contains("ermission denied"), "{why}");
    let answer = read("ungranted-answer.txt").unwrap();
    assert!(answer.contains("has no ctl grant"), "{answer}");
    let forked = std::fs::read_to_string(root.join("project/forked.txt")).unwrap();
    let fork = store.session(forked.trim()).unwrap();
    let horse = store.session(&sid).unwrap().horse;
    assert_eq!(fork.horse, horse);
    assert_eq!(fork.fork_of.unwrap().session, sid);
    let grants = paddock(&root, &["grants"]);
    assert!(grants.contains("0 of 1 commands left"), "{grants}");
}
