#![cfg(target_os = "linux")]
//! The mail hooks in a real sandbox: `paddock run claude` in bubblewrap
//! with a fake `claude` that never reads its mail. It checks that its
//! arguments carry paddock's Stop hook, and ends its turn as Claude Code
//! does: it runs that hook with the Stop input until the hook blocks the
//! stop, then once more with `stop_hook_active: true`, and once more after
//! that with no new mail. The test is its starter. Temporary home, XDG and
//! mise folders only; no vendor program. Skipped where the host cannot run
//! bubblewrap or mount FUSE.

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$*" in *'paddock-host mail hook stop'*) echo "HOOKED yes" ;; *) echo "HOOKED no" ;; esac
stop() { printf '{"hook_event_name":"Stop","session_id":"s","stop_hook_active":%s}' "$1" | paddock-host mail hook stop; }
paddock-host mail status working >/dev/null
i=0; out=
while [ $i -lt 240 ]; do
  out=$(stop false)
  [ -n "$out" ] && break
  i=$((i + 1)); sleep 0.25
done
echo "STOP1 $out" | tr '\n' ' '; echo
echo "STOP2 [$(stop true)]"
echo "STOP3 [$(stop false)]"
paddock-host mail send "done here" >/dev/null
"#;

fn fuse_works() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fuse")
        .is_ok()
}

fn write_exec(p: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    tools: PathBuf,
}

fn paddock(w: &World) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    for (k, _) in std::env::vars() {
        if k.starts_with("MISE_") || k.starts_with("XDG_") || k.starts_with("PADDOCK") {
            c.env_remove(&k);
        }
    }
    let r = &w.root;
    c.env("HOME", r.join("home"))
        .env(
            "PATH",
            format!("{}:/usr/local/bin:/usr/bin:/bin", w.tools.display()),
        )
        .env("XDG_CONFIG_HOME", r.join("cfg"))
        .env("XDG_DATA_HOME", r.join("data"))
        .env("XDG_STATE_HOME", r.join("state"))
        .env("XDG_CACHE_HOME", r.join("cache"))
        .env("XDG_RUNTIME_DIR", r.join("run"))
        .env("MISE_DATA_DIR", r.join("mise-data"))
        .env("MISE_CONFIG_DIR", r.join("cfg/mise"))
        .env("MISE_CACHE_DIR", r.join("mise-cache"))
        .env("MISE_STATE_DIR", r.join("mise-state"))
        .env("MISE_OFFLINE", "1")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("CLAUDE_CONFIG_DIR")
        .current_dir(r.join("home/proj"));
    c
}

#[test]
fn a_claude_run_in_a_sandbox_gets_its_mail_through_the_stop_hook() {
    if !common::bwrap_works() || !fuse_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    // Not in /tmp: the sandbox has its own /tmp.
    let tmp = common::tempdir();
    let root = tmp.path().to_path_buf();
    let tools = root.join("tools/claude/1.0/bin");
    write_exec(&tools.join("claude"), FAKE_CLAUDE);
    std::fs::create_dir_all(root.join("home/proj")).unwrap();
    std::fs::create_dir_all(root.join("home/.claude")).unwrap();
    std::fs::write(root.join("home/.claude/.credentials.json"), "{}").unwrap();
    std::fs::create_dir_all(root.join("run")).unwrap();
    std::fs::create_dir_all(root.join("cfg/paddock")).unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "backend = \"bwrap\"\n[network]\nmode = \"none\"\n",
    )
    .unwrap();
    let w = World {
        _tmp: tmp,
        root,
        tools,
    };
    let child = paddock(&w)
        .args(["run", "--no-yolo", "-y", "claude"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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
    let lines = BufReader::new(watch.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let mut sent = None;
    for l in lines.map_while(Result::ok) {
        if l.contains("status: working") && sent.is_none() {
            let out = paddock(&w)
                .args(["mail", "send", &id, "hello through the hook"])
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            sent = Some(String::from_utf8_lossy(&out.stdout).into_owned());
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
    let sent = sent.expect("the agent never said working");
    // The start recorded the hook delivery of this run.
    assert!(sent.contains("at the end of its turn"), "{sent}");
    assert_eq!(line("HOOKED"), "yes");
    let stop1 = line("STOP1");
    assert!(stop1.contains(r#""decision":"block""#), "{stop1}");
    assert!(stop1.contains("hello through the hook"), "{stop1}");
    assert_eq!(line("STOP2"), "[]");
    assert_eq!(line("STOP3"), "[]");
    let all = seen.join("\n");
    assert!(
        all.contains("event delivered: #") && all.contains("at the end of its turn (stop hook)"),
        "{all}"
    );
    assert!(all.contains("agent text, say: done here"), "{all}");
    assert_eq!(
        all.matches("event delivered").count(),
        1,
        "delivered once:\n{all}"
    );
}
