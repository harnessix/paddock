//! The worked example `mac` against a stand-in Mac: an sshd of the test
//! user on the loopback (tests/common/sshd.rs), the tables of
//! `paddock device add --example mac` with `connect` pointed at it, and the
//! real `build.sh`.
//!
//! Three layers. The first runs on every machine with sshd and rsync:
//! `paddock device check mac --send` with the supervisor in a thread
//! (`PADDOCK_TEST_DEVICE_LAUNCHER=thread`), which proves the script, the
//! connect relay, the agent, the stream and the exit 65 mapping. The
//! second needs bubblewrap and FUSE: two workers in a pasture build their
//! worktree through `paddock-host device mac -`, with the sandbox rules
//! proven by `paddock doctor --escape --device mac`. The third needs
//! bubblewrap and FUSE but no sshd: the same tables with `connect` to a
//! TCP echo and a script that only copies, so the sandbox rules are proven
//! on a machine without sshd too.

mod common;

use common::device_world::{can_run, echo_server, fuse_works, wait_for, World};
use common::sshd::{missing_tool, write_crate, Sshd, CRATE_NAME};
use paddock::devices::examples::MacExample;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// The tables of the example, pointed at the stand-in: the sshd port, the
/// test's key and host key file, the script in the world, the stand-in
/// home as `MAC_DIR`, and `-F /dev/null` so the user's ssh configuration
/// stays out. `mode = "allow"` instead of `ask`: no user answers a card
/// in a test, and the audit line then names the pasture grant.
fn example_tables(w: &World, sshd: &Sshd) -> String {
    let kh_dir = w.root.join("kh");
    std::fs::create_dir_all(&kh_dir).unwrap();
    let kh = kh_dir.join("known_hosts_mac");
    std::fs::write(&kh, sshd.known_hosts_line("mac.local")).unwrap();
    let script = w.root.join("dev/build.sh");
    std::fs::write(&script, paddock::devices::examples::BUILD_SH).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut ex = MacExample::new("mac.local", &sshd.user, &w.root.join("home"));
    ex.key = sshd.key_pub.display().to_string();
    ex.known_hosts = kh.display().to_string();
    ex.script = script.display().to_string();
    ex.script_dir = w.root.join("dev").display().to_string();
    ex.connect_host = "127.0.0.1".into();
    ex.connect_port = sshd.port;
    ex.ssh_extra.push("-F /dev/null".into());
    ex.env_extra
        .insert("MAC_DIR".into(), sshd.mac_dir().display().to_string());
    format!(
        "{}\n[pastures.workers]\nmembers = [\"worker\"]\ndevices = [\"mac\"]\n",
        ex.tables().replace("mode = \"ask\"", "mode = \"allow\"")
    )
}

fn stand_in(w: &World) -> Option<Sshd> {
    if let Some(t) = missing_tool() {
        eprintln!("skipped: needs {t}");
        return None;
    }
    Sshd::start(&w.root.join("sshd"))
}

/// The folders `paddock/<caller>` on the stand-in Mac.
fn copies(sshd: &Sshd) -> Vec<String> {
    std::fs::read_dir(sshd.mac_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

fn no_body_anywhere(w: &World) {
    let a = w.audit();
    assert!(!a.contains(CRATE_NAME), "the audit holds a body:\n{a}");
    assert!(!a.contains("Finished"), "the audit holds a body:\n{a}");
    for log in std::fs::read_dir(w.root.join("state/paddock/devices/log/mac"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let t = std::fs::read_to_string(log.path()).unwrap_or_default();
        assert!(!t.contains(CRATE_NAME), "the device log holds a body:\n{t}");
    }
}

/// The audit lines of the example: the device line with the grant (a
/// run writes it; `device check` sends from the host without a gate), the
/// connect line with the port, the credential use.
fn audit_has_the_example(w: &World, port: u16, grant: Option<&str>) {
    let a = w.audit();
    if let Some(grant) = grant {
        let dev = a
            .lines()
            .filter(|l| l.contains("\"capability\":\"device\"") && l.contains("\"device\":\"mac\""))
            .collect::<Vec<_>>();
        assert!(!dev.is_empty(), "no device line in:\n{a}");
        let v: serde_json::Value = serde_json::from_str(dev[0]).unwrap();
        assert_eq!(v["run"], "sandbox", "{}", dev[0]);
        assert!(
            dev.iter().any(|l| l.contains(grant)),
            "no line with {grant} in:\n{}",
            dev.join("\n")
        );
    }
    let con = a
        .lines()
        .find(|l| l.contains("\"capability\":\"device-connect\""))
        .unwrap_or_else(|| panic!("no connect line in:\n{a}"));
    let v: serde_json::Value = serde_json::from_str(con).unwrap();
    assert_eq!(v["device"], "mac");
    assert_eq!(v["port"], port);
    assert!(v["bytes_in"].as_u64().unwrap() > 0, "{con}");
    assert!(
        a.contains("ssh agent ready (device)"),
        "no credential line in:\n{a}"
    );
    assert!(
        a.lines()
            .any(|l| l.contains("\"capability\":\"credentials\"") && l.contains(" sign ")),
        "no signature line in:\n{a}"
    );
}

#[test]
fn the_script_builds_on_the_stand_in_mac_through_the_supervisor() {
    let w = World::new("");
    let Some(sshd) = stand_in(&w) else {
        return;
    };
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        example_tables(&w, &sshd),
    )
    .unwrap();
    write_crate(&w.root.join("project"));
    let check = |req: &str| {
        let out = w
            .paddock()
            .env("PADDOCK_TEST_DEVICE_LAUNCHER", "thread")
            .env("SSH_AUTH_SOCK", &sshd.agent_sock)
            .args(["device", "check", "mac", "--send", req])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "check --send {req}:\n{text}\n{}\nsshd log:\n{}",
            String::from_utf8_lossy(&out.stderr),
            sshd.log_text()
        );
        text
    };
    let logs = || {
        std::fs::read_dir(w.root.join("state/paddock/devices/log/mac"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n----\n")
    };
    // check: the cargo output of the crate, streamed, exit 0.
    let out = check("check");
    assert!(
        out.contains(&format!("Checking {CRATE_NAME}")),
        "{out}\ndevice log:\n{}\nsshd log:\n{}",
        logs(),
        sshd.log_text()
    );
    assert!(out.contains("Finished"), "{out}");
    assert!(out.contains("http 200"), "{out}");
    // The copy on the stand-in: paddock/<caller> with the crate, no .git.
    let c = copies(&sshd);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].starts_with("check-"), "{c:?}");
    assert!(sshd.mac_dir().join(&c[0]).join("Cargo.toml").exists());
    // test: the test of the crate runs there.
    let out = check("test");
    assert!(out.contains("test result: ok. 1 passed"), "{out}");
    // A wrong request: exit 65 of the script is `rejected`, EINVAL.
    let out = check("nope");
    assert!(out.contains("failed: rejected (Invalid)"), "{out}");
    let log = logs();
    assert!(log.contains("request must be test or check"), "{log}");
    audit_has_the_example(&w, sshd.port, None);
    no_body_anywhere(&w);
}

/// The harness of the workers: the checks of the agent sandbox, then the
/// requests. `WORKER` is replaced by the worker's name.
const WORKER: &str = r#"
if [ ! -e /dev/paddock/mac ]; then echo NODEV; exit 0; fi
if [ -S "${SSH_AUTH_SOCK:-}" ]; then echo AGENT_SOCKET_IN_AGENT_SANDBOX; else echo NO_AGENT_SOCKET; fi
if ls "$HOME/.ssh" 2>/dev/null | grep -q .; then echo SSH_FILES_IN_AGENT_SANDBOX; else echo NO_SSH_FILES; fi
printf check | paddock-host device mac - > out.txt; echo "CHECK_EXIT $?"
grep -c "Checking\|Finished" out.txt
printf nope | paddock-host device mac - 2> err.txt; echo "NOPE_EXIT $?"
cat err.txt
"#;

fn run_worker(w: &World, project: &Path) -> std::process::Child {
    w.paddock()
        .args([
            "run",
            "worker",
            "--backend",
            "bwrap",
            "--network",
            "none",
            "--env",
            "clean",
            "--pasture",
            "workers",
            "-y",
        ])
        .arg(project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn escape_probe(w: &World, project: &Path, sshd: Option<&Sshd>) -> String {
    let mut c = w.paddock();
    if let Some(s) = sshd {
        c.env("SSH_AUTH_SOCK", &s.agent_sock);
    }
    let out = c
        .current_dir(project)
        .args(["doctor", "--escape", "--device", "mac"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "doctor --escape --device mac:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("device sandbox escape probe: every check passed"),
        "{text}"
    );
    for check in [
        "ok home-empty",
        "ok host-loopback-unreachable",
        "ok no-interface-but-lo",
        "ok no-paddock-host",
        "ok caller-read-only",
        "ok connect-port-2222-reachable",
        "ok ssh-dir-only-ro-files",
    ] {
        assert!(text.contains(check), "missing {check}:\n{text}");
    }
    if sshd.is_some() {
        assert!(text.contains("ok key-files-absent"), "{text}");
        assert!(text.contains("ok agent-socket-present"), "{text}");
        assert!(text.contains("ok agent-filters"), "{text}");
    }
    text
}

#[test]
fn workers_in_a_pasture_build_their_worktree_on_the_stand_in_mac() {
    if !can_run() {
        return;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return;
    }
    let w = World::new("");
    let Some(sshd) = stand_in(&w) else {
        return;
    };
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        example_tables(&w, &sshd),
    )
    .unwrap();
    w.harness("worker", WORKER);
    let projects: Vec<PathBuf> = ["project", "project2"]
        .iter()
        .map(|p| {
            let d = w.root.join(p);
            write_crate(&d);
            d
        })
        .collect();
    // The agent of the test holds the key: the paddock process of a run
    // reads SSH_AUTH_SOCK where it starts.
    let children: Vec<_> = projects
        .iter()
        .map(|p| {
            w.paddock()
                .env("SSH_AUTH_SOCK", &sshd.agent_sock)
                .args([
                    "run",
                    "worker",
                    "--backend",
                    "bwrap",
                    "--network",
                    "none",
                    "--env",
                    "clean",
                    "--pasture",
                    "workers",
                    "-y",
                ])
                .arg(p)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut outputs = Vec::new();
    for c in children {
        let out = c.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{text}\n{}\nsshd log:\n{}",
            String::from_utf8_lossy(&out.stderr),
            sshd.log_text()
        );
        outputs.push(text);
    }
    if outputs.iter().any(|t| t.contains("NODEV")) {
        eprintln!("skipped: the sandbox has no /dev/paddock here");
        return;
    }
    for text in &outputs {
        assert!(text.contains("NO_AGENT_SOCKET"), "{text}");
        assert!(text.contains("NO_SSH_FILES"), "{text}");
        assert!(text.contains("CHECK_EXIT 0"), "{text}");
        assert!(text.contains("\n2\n"), "two cargo lines:\n{text}");
        assert!(text.contains("NOPE_EXIT 8"), "{text}");
        assert!(text.contains("the device rejected the request"), "{text}");
    }
    // Each worker got its own instance and its own folder on the stand-in.
    let c = copies(&sshd);
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(c
        .iter()
        .all(|d| sshd.mac_dir().join(d).join("Cargo.toml").exists()));
    let instances: Vec<String> = w
        .audit()
        .lines()
        .filter(|l| l.contains("\"capability\":\"device\"") && l.contains("\"device\":\"mac\""))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["instance"].as_str().map(str::to_string))
        .collect();
    let mut distinct = instances.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 2, "two instances in {instances:?}");
    audit_has_the_example(&w, sshd.port, Some("allowed by grant pasture workers"));
    no_body_anywhere(&w);
    // The instances ended with the runs.
    wait_for("the instances to end", Duration::from_secs(5), || {
        w.state_files("mac").is_empty()
    });
    // The escape probe, in the device sandbox of the example.
    escape_probe(&w, &projects[0], Some(&sshd));
}

#[test]
fn without_sshd_the_sandbox_rules_hold_with_a_tcp_echo_and_a_copy() {
    if !can_run() {
        return;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return;
    }
    let (port, _echo) = echo_server();
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    let script = w.script(
        "copy.sh",
        "read -r what || true\ncase \"$what\" in check) ;; *) echo \"request must be check\" >&2; exit 65 ;; esac\ncp -R \"$PADDOCK_CALLER_MOUNT/.\" ./copy\nls copy | tr '\\n' ' '\necho\n/usr/bin/python3 -c 'import socket\ns=socket.socket(); s.settimeout(3); s.connect((\"127.0.0.1\", 2222)); s.sendall(b\"ping\"); print(s.recv(100).decode())'",
    );
    let kh = w.root.join("kh");
    std::fs::create_dir_all(&kh).unwrap();
    std::fs::write(kh.join("known_hosts_mac"), "mac.local ssh-ed25519 AAAA\n").unwrap();
    let mut ex = MacExample::new("mac.local", "me", &w.root.join("home"));
    ex.known_hosts = kh.join("known_hosts_mac").display().to_string();
    ex.script = script;
    ex.script_dir = dev;
    ex.connect_host = "127.0.0.1".into();
    ex.connect_port = port;
    let tables = ex
        .tables()
        .replace("mode = \"ask\"", "mode = \"allow\"")
        .replace("credentials = [\"mac-ssh\"]\n", "");
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!("{tables}\n[pastures.workers]\nmembers = [\"worker\"]\ndevices = [\"mac\"]\n"),
    )
    .unwrap();
    w.harness(
        "worker",
        "if [ ! -e /dev/paddock/mac ]; then echo NODEV; exit 0; fi\nprintf check | paddock-host device mac -; echo \"CHECK_EXIT $?\"",
    );
    let project = w.root.join("project");
    write_crate(&project);
    let out = run_worker(&w, &project).wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if text.contains("NODEV") {
        eprintln!("skipped: the sandbox has no /dev/paddock here");
        return;
    }
    assert!(text.contains("Cargo.toml"), "{text}");
    assert!(text.contains("pong:ping"), "{text}");
    assert!(text.contains("CHECK_EXIT 0"), "{text}");
    no_body_anywhere(&w);
    escape_probe(&w, &project, None);
}
