//! `paddock horse` on a Codex session with a warm side branch: the log
//! marks the live nodes, no codec projects them, and `fork --warm` and
//! `fork --close` say what they need when no run holds the session.
//! Temporary folders only; no codex, no network.

use paddock::horse::codex::{self, VERSION};
use paddock::horse::store::{Origin, Store};
use std::path::Path;
use std::process::{Command, Output};

const THREAD: &str = "019a5f00-0000-7000-8000-0000000000bb";

fn cmd(tmp: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .arg("horse")
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.join("home"))
        .env("XDG_DATA_HOME", tmp.join("data"))
        .env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CONFIG_HOME", tmp.join("config"))
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env("XDG_RUNTIME_DIR", tmp.join("run"))
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn row(ty: &str, payload: &str) -> String {
    format!(r#"{{"timestamp":"2026-11-06T00:00:00.000Z","type":"{ty}","payload":{payload}}}"#)
}

#[test]
fn a_side_branch_is_logged_as_live_and_never_projected() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let dir = t.join("home/.codex/sessions/2026/11/06");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("rollout-2026-11-06T00-00-00-{THREAD}.jsonl"));
    let rows = [
        row(
            "session_meta",
            &format!(r#"{{"id":"{THREAD}","cli_version":"{VERSION}","cwd":"/w"}}"#),
        ),
        row(
            "response_item",
            r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}"#,
        ),
    ];
    std::fs::write(&file, rows.join("\n") + "\n").unwrap();
    let out = cmd(
        t,
        &[
            "ingest-codex",
            file.to_str().unwrap(),
            "--codex-version",
            VERSION,
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let parent = text(&out.stdout).split(':').next().unwrap().to_string();

    // A side branch as the custody leaves it: a fork at the head with one
    // live row on top, bound to the ephemeral thread.
    let vars = [
        ("XDG_DATA_HOME", t.join("data")),
        ("XDG_STATE_HOME", t.join("state")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.display().to_string()))
    .collect();
    let env = paddock::env::HostEnv::from_parts(t.join("home"), "test".into(), 0, 0, vars);
    let st = Store::open_host(&env).unwrap();
    let s = st.session(&parent).unwrap();
    let head = s.head.clone().unwrap();
    let side = st.fork(&parent, &head, None).unwrap();
    let o = Origin {
        harness: codex::HARNESS,
        container: codex::CONTAINER,
        version: VERSION,
    };
    let live = row(
        "response_item",
        r#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"side answer"}]}"#,
    );
    let h = st
        .put_live(&o, Some(&head), "live:2", live.as_bytes())
        .unwrap();
    st.update_session(&side.id, |x| {
        x.head = Some(h.clone());
        x.binding.native_id = Some("019a5f00-0000-7000-8000-0000000000f1".into());
    })
    .unwrap();

    let out = cmd(t, &["log", &side.id]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = text(&out.stdout);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 3, "{log}");
    assert!(lines[2].ends_with("  live"), "{log}");
    assert!(lines[2].starts_with(h.short()), "{log}");
    assert!(
        !lines[0].ends_with("live") && !lines[1].ends_with("live"),
        "{log}"
    );

    let home = t.join("other/.codex");
    let out = cmd(
        t,
        &[
            "project-codex",
            &side.id,
            "--config-dir",
            home.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(
        err.contains(&format!(
            "session {} cannot be projected: node {} is not a native original; persist it with the harness first",
            side.id,
            h.short()
        )),
        "{err}"
    );
    assert!(!home.join("sessions").exists());
    // The parent still projects.
    let out = cmd(
        t,
        &[
            "project-codex",
            &parent,
            "--config-dir",
            home.to_str().unwrap(),
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));

    // No run holds the session: --warm says so, --close has nothing to do.
    let out = cmd(t, &["fork", &parent, "--warm"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains("is in no live horse-mode run"), "{err}");
    assert!(err.contains("starts cold"), "{err}");
    let out = cmd(t, &["fork", &side.id, "--close"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("its custody ended already"));
    let out = cmd(t, &["fork", &parent, "--close"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("is no side branch"));
    // --at is still needed for a plain fork, and --warm takes none.
    assert!(!cmd(t, &["fork", &parent]).status.success());
    assert!(!cmd(t, &["fork", &parent, "--warm", "--at", "row:0"])
        .status
        .success());
}
