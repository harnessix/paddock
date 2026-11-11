#![cfg(target_os = "linux")]
//! The run mailbox in a real sandbox: a fake harness in bubblewrap uses
//! `paddock-host mail` and the files in `/dev/paddock/mail`, and the test
//! is its starter with `paddock mail send` and `watch`. Skipped where the
//! host cannot run bubblewrap or mount FUSE.

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SCRIPT: &str = r#"
if [ -e /dev/paddock/mail/in ]; then echo "LS $(ls /dev/paddock/mail | tr '\n' ' ')"; else echo NODEV; fi
paddock-host mail send "from the agent" >/dev/null; echo "SEND_EXIT $?"
echo "plain line" > /dev/paddock/mail/out; echo "OUT_EXIT $?"
paddock-host mail status working >/dev/null; echo "STATUS_EXIT $?"
out=$(paddock-host mail wait --timeout 60); e=$?
echo "WAIT_EXIT $e"
echo "WAIT_TEXT $(printf '%s' "$out" | tail -n1)"
exec 3<>/dev/paddock/mail/in; printf '{"timeout":1}' >&3
if cat <&3 >/dev/null 2>/tmp/err; then echo RAW_OK; else echo "RAW_ERR $(cat /tmp/err)"; fi
exec 3<&-
head -c 66560 /dev/zero | tr '\0' x | paddock-host mail send - 2>/tmp/big; echo "BIG_EXIT $?"
echo "BIG_ERR $(cat /tmp/big)"
"#;

fn fuse_works() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fuse")
        .is_ok()
}

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

fn paddock(w: &World) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.env("HOME", w.root.join("home"))
        .env("XDG_CONFIG_HOME", w.root.join("cfg"))
        .env("XDG_DATA_HOME", w.root.join("data"))
        .env("XDG_STATE_HOME", w.root.join("state"))
        .env("XDG_CACHE_HOME", w.root.join("cache"))
        .env("XDG_RUNTIME_DIR", w.root.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("PADDOCK_CALLER")
        .env("LANG", "C")
        .env("LC_ALL", "C");
    c
}

#[test]
fn an_agent_in_a_sandbox_and_its_starter_talk_through_the_mailbox() {
    if !common::bwrap_works() || !fuse_works() {
        eprintln!("skipped: needs bwrap with a user namespace and FUSE");
        return;
    }
    let tmp = common::tempdir();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let cfg = root.join("cfg/paddock/harness.d");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(root.join("project")).unwrap();
    std::fs::create_dir_all(root.join("run")).unwrap();
    std::fs::write(
        cfg.join("mailfake.toml"),
        format!(
            "name = \"mailfake\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n",
            serde_json::to_string(SCRIPT).unwrap()
        ),
    )
    .unwrap();
    let w = World { _tmp: tmp, root };
    let child = paddock(&w)
        .args([
            "run",
            "mailfake",
            "--backend",
            "bwrap",
            "--network",
            "none",
            "--env",
            "clean",
            "-y",
        ])
        .arg(w.root.join("project"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The run makes its mailbox at its start.
    let start = Instant::now();
    let id = loop {
        let out = paddock(&w).args(["mail", "ls", "--json"]).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
        if let Some(id) = v.get(0).and_then(|r| r["id"].as_str()) {
            break id.to_string();
        }
        assert!(start.elapsed() < Duration::from_secs(60), "no mailbox");
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut watch = paddock(&w)
        .args(["mail", "watch", &id, "--timeout", "120s"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(watch.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let mut sent = false;
    for l in lines.by_ref().map_while(Result::ok) {
        if l.contains("status: working") && !sent {
            let out = paddock(&w)
                .args(["mail", "send", &id, "hello agent"])
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            sent = true;
        }
        let end = l.contains("event ended");
        seen.push(l);
        if end {
            break;
        }
    }
    assert_eq!(watch.wait().unwrap().code(), Some(0), "{seen:#?}");
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    let line = |k: &str| -> String {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{k} ")))
            .unwrap_or_else(|| panic!("no {k} in:\n{text}\n{err}"))
            .to_string()
    };
    assert_eq!(line("LS").trim(), "in out");
    assert_eq!(line("SEND_EXIT"), "0");
    assert_eq!(line("OUT_EXIT"), "0");
    assert_eq!(line("STATUS_EXIT"), "0");
    assert_eq!(line("WAIT_EXIT"), "0");
    assert_eq!(line("WAIT_TEXT"), "hello agent");
    assert!(line("RAW_ERR").contains("Timer expired"), "{text}");
    assert_eq!(line("BIG_EXIT"), "10");
    assert!(line("BIG_ERR").contains("--ref .paddock/"), "{text}");
    let all = seen.join("\n");
    for want in [
        "event started:",
        "agent text, say: from the agent",
        "agent text, say: plain line",
        "agent text, status: working",
        "event ended: the sandbox ended with exit code 0",
    ] {
        assert!(all.contains(want), "{want} missing in:\n{all}");
    }
}
