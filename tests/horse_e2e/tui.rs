//! The TUI channel without a sandbox. The fake Claude Code TUI writes its
//! first turn as a child process. The test then does what the hooks and
//! the channel server of the sandbox do (`paddock-host horse safe-point`
//! and `paddock-host channel`): it sends their requests to the same host
//! code that the broker calls, and writes the turn that the pushed entry
//! starts.

use super::*;
use paddock::broker::{Request, Status, PROTOCOL};
use paddock::horse::device::Access;
use paddock::horse::live::channel::{self, CustodyCtx, Timing};
use paddock::horse::live::{harness::Control, tui_state};
use paddock::horse::{claude, run};
use std::process::Stdio;

const NATIVE: &str = "01934b20-2222-7777-8888-123456789abc";

/// A `horse.safe_point` or `horse.channel_wait` request of the sandbox.
fn ask(access: &Access, args: serde_json::Value) -> serde_json::Value {
    let req = Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: serde_json::from_value(args).unwrap(),
        reason: String::new(),
    };
    let resp = tui_state::handle(Some(access), &req, &|_| {});
    assert_eq!(resp.status, Status::Done, "{}", resp.message);
    resp.result
}

/// One turn of the fake TUI: its user row and its reply.
fn turn(file: &Path, cwd: &str, n: usize, parent: &str, text: &str) {
    use std::io::Write as _;
    let row = |ty: &str, message: serde_json::Value, uuid: &str, parent: &str| {
        serde_json::json!({
            "parentUuid": parent, "isSidechain": false, "type": ty, "message": message,
            "uuid": uuid, "timestamp": "2026-11-06T10:00:00.000Z", "userType": "external",
            "entrypoint": "cli", "cwd": cwd, "sessionId": NATIVE, "version": claude::VERSION,
            "gitBranch": "main"
        })
        .to_string()
    };
    let user = serde_json::json!({"role": "user", "content": [{"type": "text", "text": text}]});
    let reply = serde_json::json!({
        "id": format!("msg_{n}"), "type": "message", "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [{"type": "text", "text": format!("turn {n} done")}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 4, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": 2}
    });
    let (u, a) = (format!("u{n}"), format!("a{n}"));
    let mut f = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    writeln!(f, "{}", row("user", user, &u, parent)).unwrap();
    writeln!(f, "{}", row("assistant", reply, &a, &u)).unwrap();
}

/// An interactive Claude Code run in horse mode gets an entry between its
/// turns through the channel, and the entry lands in its session.
pub fn walk(t: &Path, store: &Store) {
    let rh = run::begin(store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    store
        .lock()
        .unwrap()
        .bind_native(&rh.session, NATIVE)
        .unwrap();
    let home = t.join("tui-home");
    let project = t.join("project");
    let cwd = project.display().to_string();
    let file = claude::transcript_path(&home.join(".claude"), &cwd, NATIVE);
    let access = Access {
        store: store.clone(),
        horse: rh.horse.clone(),
        session: rh.session.clone(),
    };

    // The custody, as `lifecycle::start` arms it for a channel version.
    let runs = tui_state::runs();
    let signals = runs.subscribe(&tui_state::run_key(&rh.horse, &rh.session));
    let (close, control) = std::sync::mpsc::channel();
    let ctx = CustodyCtx {
        store: store.clone(),
        horse: rh.horse.clone(),
        session: rh.session.clone(),
        native: NATIVE.into(),
        config_dir: home.join(".claude"),
        cwd: cwd.clone(),
        version: claude::VERSION.into(),
        run: "walk-tui".into(),
        runs,
        ledger: Box::new(|_| {}),
        control,
        timing: Timing {
            tick: Duration::from_millis(20),
            ..Timing::default()
        },
    };
    let custody = std::thread::spawn(move || channel::run(ctx, signals));

    // The fake TUI writes turn 1, then waits for `done`.
    std::fs::create_dir_all(&home).unwrap();
    let mut tui = Command::new(fixture("claude-tui"))
        .arg(format!("--session-id={NATIVE}"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .current_dir(&project)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let rep = || report(t);
    wait_until("turn 1 of the TUI", &rep, || {
        std::fs::read_to_string(&file).is_ok_and(|s| s.lines().count() >= 2)
    });

    // An entry that the host appends while the TUI is busy waits.
    let entry = horse(t, &["append", &rh.session, "--text", "from the tui walk"]);
    let entry = entry.trim();
    let wait = serde_json::json!({"op": "channel_wait", "since": 0, "timeout_ms": 200});
    assert_eq!(ask(&access, wait)["entries"], serde_json::json!([]));
    // The Stop hook of turn 1: now the channel server gets the entry.
    ask(
        &access,
        serde_json::json!({"op": "safe_point", "kind": "stop"}),
    );
    let wait = serde_json::json!({"op": "channel_wait", "since": 0, "timeout_ms": 5000});
    let got = ask(&access, wait);
    let entries = got["entries"].as_array().unwrap().clone();
    // The DNA entry of the new session first, then the appended one.
    assert_eq!(entries.len(), 2, "{got}");
    assert_eq!(entries[0]["role"], "system", "{got}");
    assert_eq!(entries[1]["id"], entry, "{got}");
    assert_eq!(entries[1]["text"], "from the tui walk", "{got}");
    // The TUI takes each push as a turn.
    ask(
        &access,
        serde_json::json!({"op": "safe_point", "kind": "prompt"}),
    );
    let mut parent = "a1".to_string();
    for (k, e) in entries.iter().enumerate() {
        let id = e["id"].as_str().unwrap();
        let pushed = format!("<channel source=\"paddock\" entry_id=\"{id}\">queued</channel>");
        turn(&file, &cwd, k + 2, &parent, &pushed);
        parent = format!("a{}", k + 2);
    }
    ask(
        &access,
        serde_json::json!({"op": "safe_point", "kind": "stop"}),
    );
    wait_until("the TUI entry landed", &rep, || {
        delivered(store, &rh.session, "from the tui walk")
            .iter()
            .any(|d| d.landed_seq.is_some())
    });

    std::fs::write(project.join("done"), "").unwrap();
    assert!(tui.wait().unwrap().success());
    close.send(Control::Close).unwrap();
    close.send(Control::Close).unwrap();
    let r = custody.join().unwrap().unwrap();
    assert_eq!(r.released, 2, "{r:?}");
    assert_eq!(r.landed, 2, "{r:?}");
    assert_eq!(r.stops, 2, "{r:?}");
    assert!(r.not_landed.is_empty(), "{r:?}");
    assert!(store.inbox(&rh.session).unwrap().is_empty());
    let users = rows(store, &rh.session)
        .iter()
        .filter(|r| r.contains("\"type\":\"user\""))
        .count();
    assert_eq!(users, 3, "{}", rep());
}
