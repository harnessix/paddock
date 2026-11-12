use super::adopt;
use super::fixtures::*;
use super::hookcmd;
use super::hooks::Removed;
use super::*;
use serde_json::{json, Value};

struct Host {
    _t: tempfile::TempDir,
    store: Store,
    home: PathBuf,
    state: PathBuf,
}

const EXE: &str = "/usr/bin/paddock";

fn adopted(harness: &str) -> Host {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let state = t.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    let h = Host {
        _t: t,
        store,
        home,
        state,
    };
    adopt::adopt(
        &h.store,
        &h.state,
        harness,
        &h.home,
        &[],
        Some(Path::new(EXE)),
    )
    .unwrap();
    h
}

/// The output of a hook, if it gave one.
fn run_hook(
    store: &Store,
    state: &Path,
    harness: &str,
    event: &str,
    input: &[u8],
) -> Option<Value> {
    let mut out = Vec::new();
    hookcmd::run(
        store,
        state,
        harness,
        event,
        input,
        &adopt::Bounds::default(),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    (!text.is_empty()).then(|| serde_json::from_str(&text).unwrap())
}

fn hook(h: &Host, harness: &str, event: &str, input: Value) -> Option<Value> {
    run_hook(
        &h.store,
        &h.state,
        harness,
        event,
        input.to_string().as_bytes(),
    )
}

#[test]
fn adoption_puts_its_hooks_in_and_off_takes_them_out() {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let state = t.path().join("state");
    let settings = "{\"model\": \"opus\"}\n";
    let p = home.join(".claude/settings.json");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, settings).unwrap();
    write_claude(&home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    let sessions_before = tree(&home.join(".claude/projects"));
    adopt::adopt(
        &store,
        &state,
        claude::HARNESS,
        &home,
        &[],
        Some(Path::new(EXE)),
    )
    .unwrap();
    let a = adopt::load(&state).unwrap().harnesses[claude::HARNESS].clone();
    let i = a.hooks.clone().unwrap();
    assert_eq!(i.entries.len(), 3);
    let d: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(
        d["hooks"]["Stop"][0]["hooks"][0]["command"],
        "/usr/bin/paddock horse hook claude Stop --mark paddock-horse"
    );
    // The sessions on the host did not change; only the hook file did.
    assert_eq!(tree(&home.join(".claude/projects")), sessions_before);
    let (_, removed) = adopt::off(&state, claude::HARNESS).unwrap().unwrap();
    assert_eq!(removed, Some(Removed::Restored));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), settings);
    assert_eq!(tree(&home.join(".claude/projects")), sessions_before);
}

#[test]
fn a_refused_hook_file_stops_the_adoption_before_anything_changes() {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let state = t.path().join("state");
    let p = home.join(".claude/settings.json");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "not json").unwrap();
    write_claude(&home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    assert!(adopt::adopt(
        &store,
        &state,
        claude::HARNESS,
        &home,
        &[],
        Some(Path::new(EXE))
    )
    .is_err());
    assert!(adopt::load(&state).unwrap().harnesses.is_empty());
    assert!(store.sessions().unwrap().is_empty());
    assert!(store.horses().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "not json");
    // Without hooks, the adoption works.
    adopt::adopt(&store, &state, claude::HARNESS, &home, &[], None).unwrap();
    assert!(adopt::load(&state).unwrap().harnesses[claude::HARNESS]
        .hooks
        .is_none());
}

#[test]
fn a_second_adopt_sweeps_when_the_hook_file_is_not_json_any_more() {
    let h = adopted(claude::HARNESS);
    let p = h.home.join(".claude/settings.json");
    let before = adopt::load(&h.state).unwrap().harnesses[claude::HARNESS]
        .hooks
        .clone()
        .unwrap();
    // The user breaks the JSON of the settings file, then a new session
    // comes, and the user adopts again.
    std::fs::write(&p, "{\"model\": ").unwrap();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    let s = adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[],
        Some(Path::new(EXE)),
    )
    .unwrap();
    // The sweep ran: the new session came in.
    assert_eq!(s.imported.iter().filter(|i| i.new).count(), 1);
    assert!(h
        .store
        .host_session(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .is_some());
    // The adopt tells why the hook entries did not change.
    assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
    assert!(
        s.warnings[0].contains(".claude/settings.json"),
        "{:?}",
        s.warnings
    );
    // Nothing changed the file or the record of the entries.
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"model\": ");
    assert_eq!(
        adopt::load(&h.state).unwrap().harnesses[claude::HARNESS].hooks,
        Some(before)
    );
}

#[test]
fn off_keeps_the_adoption_when_the_hook_file_cannot_change() {
    let h = adopted(claude::HARNESS);
    let p = h.home.join(".claude/settings.json");
    // The user makes the file a symlink after the adoption.
    let target = h.home.join("dotfiles.json");
    std::fs::rename(&p, &target).unwrap();
    std::os::unix::fs::symlink(&target, &p).unwrap();
    assert!(adopt::off(&h.state, claude::HARNESS).is_err());
    assert!(adopt::load(&h.state)
        .unwrap()
        .harnesses
        .contains_key(claude::HARNESS));
}

#[test]
fn a_stop_hook_reads_the_turn_and_delivers_the_queue() {
    let h = adopted(claude::HARNESS);
    // A new host session: its first turn ends.
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let input = json!({"hook_event_name": "Stop", "session_id": CLAUDE_A,
        "transcript_path": h.home.join(claude_rel("p", CLAUDE_A))});
    assert_eq!(hook(&h, claude::HARNESS, "Stop", input.clone()), None);
    let s = h
        .store
        .host_session(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .expect("the turn is in the store");
    assert_eq!(h.store.session_nodes(&s).unwrap().len(), 2);

    // Two queued entries go out at the next Stop, oldest first.
    h.store
        .queue_append(&s.id, "user", "The tests pass on main.")
        .unwrap();
    h.store.queue_append(&s.id, "note", "Use staging.").unwrap();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 4));
    let out = hook(&h, claude::HARNESS, "Stop", input.clone()).unwrap();
    assert_eq!(out["decision"], "block");
    let reason = out["reason"].as_str().unwrap();
    let a = reason.find("The tests pass on main.").unwrap();
    let b = reason.find("<paddock-append role=\"note\">").unwrap();
    assert!(a < b, "{reason}");
    assert!(h.store.inbox(&s.id).unwrap().is_empty());
    assert_eq!(h.store.deliveries(&s.id).unwrap().len(), 2);
    assert_eq!(
        h.store
            .session_nodes(&h.store.session(&s.id).unwrap())
            .unwrap()
            .len(),
        4
    );
    // The queue is empty: the session stops as usual.
    assert_eq!(hook(&h, claude::HARNESS, "Stop", input), None);

    // A prompt of the user takes the queue into the context.
    h.store
        .queue_append(&s.id, "user", "Also check the docs.")
        .unwrap();
    let out = hook(
        &h,
        claude::HARNESS,
        "UserPromptSubmit",
        json!({"session_id": CLAUDE_A, "prompt": "go on"}),
    )
    .unwrap();
    assert_eq!(
        out["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert_eq!(
        out["hookSpecificOutput"]["additionalContext"],
        "Also check the docs."
    );
    assert!(h.store.inbox(&s.id).unwrap().is_empty());
}

#[test]
fn a_new_host_session_starts_with_the_dna_of_its_horse() {
    let h = adopted(claude::HARNESS);
    let a = adopt::load(&h.state).unwrap().harnesses[claude::HARNESS].clone();
    let dna = crate::horse::dna::get_or_default(&h.store, &h.store.horse(&a.horse).unwrap().dna);
    let want = crate::horse::dna::render_entry(&dna, "start");
    let out = hook(
        &h,
        claude::HARNESS,
        "SessionStart",
        json!({"session_id": CLAUDE_A, "source": "startup"}),
    )
    .unwrap();
    assert_eq!(out["hookSpecificOutput"]["additionalContext"], want);
    for source in ["resume", "clear", "compact"] {
        assert_eq!(
            hook(
                &h,
                claude::HARNESS,
                "SessionStart",
                json!({"session_id": CLAUDE_A, "source": source})
            ),
            None,
            "{source}"
        );
    }
    let c = adopted(codex::HARNESS);
    let out = hook(&c, codex::HARNESS, "SessionStart", json!({})).unwrap();
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
}

#[test]
fn a_hook_of_a_harness_that_is_not_adopted_does_nothing() {
    let h = adopted(claude::HARNESS);
    write_codex(&h.home, CODEX_A, &codex_rows(CODEX_A, 1));
    assert_eq!(hook(&h, codex::HARNESS, "Stop", json!({})), None);
    assert!(h
        .store
        .host_session(codex::HARNESS, CODEX_A)
        .unwrap()
        .is_none());
    // Bad input is no session.
    assert_eq!(
        run_hook(&h.store, &h.state, claude::HARNESS, "Stop", b"not json"),
        None
    );
    // The error log is private.
    hookcmd::log_error(&h.state, "claude", "Stop", &anyhow::anyhow!("x\ny"));
    let log = adopt::dir(&h.state).join("hook.log");
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .ends_with("claude Stop: x y\n"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// A writer that fails: the harness went away (its timeout killed the
/// hook) before the output was out.
struct Gone;

impl std::io::Write for Gone {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }
}

/// An adopted Claude Code host session with `n` queued entries.
fn queued(n: usize) -> (Host, crate::horse::Session, Value) {
    let h = adopted(claude::HARNESS);
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let input =
        json!({"session_id": CLAUDE_A, "transcript_path": h.home.join(claude_rel("p", CLAUDE_A))});
    assert_eq!(hook(&h, claude::HARNESS, "Stop", input.clone()), None);
    let s = h
        .store
        .host_session(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .unwrap();
    for i in 0..n {
        h.store
            .queue_append(&s.id, "user", &format!("entry {i}"))
            .unwrap();
    }
    (h, s, input)
}

#[test]
fn an_entry_stays_queued_when_the_output_does_not_reach_the_harness() {
    let (h, s, input) = queued(2);
    let r = hookcmd::run(
        &h.store,
        &h.state,
        claude::HARNESS,
        "Stop",
        input.to_string().as_bytes(),
        &adopt::Bounds::default(),
        &mut Gone,
    );
    assert!(r.is_err());
    assert_eq!(h.store.inbox(&s.id).unwrap().len(), 2);
    assert!(h.store.deliveries(&s.id).unwrap().is_empty());
    // The next event gives them.
    let out = hook(&h, claude::HARNESS, "Stop", input).unwrap();
    assert!(out["reason"].as_str().unwrap().contains("entry 1"));
    assert!(h.store.inbox(&s.id).unwrap().is_empty());
    assert_eq!(h.store.deliveries(&s.id).unwrap().len(), 2);
}

#[test]
fn a_hook_does_not_wait_for_a_long_sweep() {
    let (h, s, input) = queued(1);
    // A large first adopt (or a --sync) holds the lock of the sweeps.
    let held = adopt::Locked::open(&h.state).unwrap();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 4));
    let (tx, rx) = std::sync::mpsc::channel();
    let (store, state) = (h.store.clone(), h.state.clone());
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let bounds = adopt::Bounds {
            wait: std::time::Duration::from_millis(100),
            budget: std::time::Duration::from_secs(5),
        };
        let r = hookcmd::run(
            &store,
            &state,
            claude::HARNESS,
            "Stop",
            input.to_string().as_bytes(),
            &bounds,
            &mut out,
        );
        let _ = tx.send((r.is_ok(), out));
    });
    let (ok, out) = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the hook returns while the lock is held");
    assert!(ok);
    // The queue goes out without the read.
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out["decision"], "block");
    assert_eq!(
        h.store
            .session_nodes(&h.store.session(&s.id).unwrap())
            .unwrap()
            .len(),
        2
    );
    let log = std::fs::read_to_string(adopt::dir(&h.state).join("hook.log")).unwrap();
    assert!(log.contains("another sweep holds the lock"), "{log}");
    drop(held);
}

/// A writer that stops at its first write: it says so on `entered`, and
/// goes on when `go` gets a message. A hook that writes into it stops
/// after it read the inbox and before it records the deliveries.
struct Gate {
    entered: Option<std::sync::mpsc::Sender<()>>,
    go: std::sync::mpsc::Receiver<()>,
    buf: Vec<u8>,
}

impl std::io::Write for Gate {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Some(tx) = self.entered.take() {
            tx.send(()).unwrap();
            self.go.recv().unwrap();
        }
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn two_hooks_of_one_session_give_each_entry_once() {
    let (h, s, input) = queued(3);
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel();
    let mut outs = Vec::new();
    std::thread::scope(|sc| {
        // The first hook read the queue and is in the middle of its output.
        let first = sc.spawn(|| {
            let mut out = Gate {
                entered: Some(entered_tx),
                go: go_rx,
                buf: Vec::new(),
            };
            hookcmd::run(
                &h.store,
                &h.state,
                claude::HARNESS,
                "Stop",
                input.to_string().as_bytes(),
                &adopt::Bounds::default(),
                &mut out,
            )
            .unwrap();
            out.buf
        });
        entered.recv().unwrap();
        // The second hook of the session comes now, with the queue still
        // unrecorded. The order is fixed by the gate, not by the threads.
        let mut out = Vec::new();
        hookcmd::run(
            &h.store,
            &h.state,
            claude::HARNESS,
            "Stop",
            input.to_string().as_bytes(),
            &adopt::Bounds {
                wait: std::time::Duration::from_millis(200),
                budget: std::time::Duration::from_secs(5),
            },
            &mut out,
        )
        .unwrap();
        outs.push(out);
        go.send(()).unwrap();
        outs.push(first.join().unwrap());
    });
    // The second hook gave nothing; the first gave each entry.
    assert!(
        outs[0].is_empty(),
        "{:?}",
        String::from_utf8_lossy(&outs[0])
    );
    let given: Vec<&Vec<u8>> = outs.iter().filter(|o| !o.is_empty()).collect();
    assert_eq!(given.len(), 1, "{outs:?}");
    assert_eq!(h.store.deliveries(&s.id).unwrap().len(), 3);
    assert!(h.store.inbox(&s.id).unwrap().is_empty());
    // A third hook finds the queue empty.
    assert_eq!(hook(&h, claude::HARNESS, "Stop", input), None);
}

#[test]
fn a_prompt_hook_reads_nothing_and_a_stop_hook_reads_its_own_session() {
    let (h, s, input) = queued(0);
    // Two sessions change: the one of the hook, and another one.
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 4));
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 1));
    assert_eq!(
        hook(
            &h,
            claude::HARNESS,
            "UserPromptSubmit",
            json!({"session_id": CLAUDE_A, "prompt": "go"})
        ),
        None
    );
    assert_eq!(
        h.store
            .session_nodes(&h.store.session(&s.id).unwrap())
            .unwrap()
            .len(),
        2
    );
    assert!(h
        .store
        .host_session(claude::HARNESS, CLAUDE_B)
        .unwrap()
        .is_none());
    assert_eq!(hook(&h, claude::HARNESS, "Stop", input), None);
    assert_eq!(
        h.store
            .session_nodes(&h.store.session(&s.id).unwrap())
            .unwrap()
            .len(),
        4
    );
    assert!(h
        .store
        .host_session(claude::HARNESS, CLAUDE_B)
        .unwrap()
        .is_none());
    // The other session comes in at the next full sweep.
    let w = adopt::sweep(&h.store, &h.state, Some(claude::HARNESS)).unwrap();
    assert_eq!(w[0].imported.len(), 1);
    assert!(h
        .store
        .host_session(claude::HARNESS, CLAUDE_B)
        .unwrap()
        .is_some());
}

#[test]
fn a_hook_sweep_stops_at_its_deadline_and_leaves_the_rest() {
    let h = adopted(claude::HARNESS);
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 1));
    let none = adopt::Bounds {
        wait: std::time::Duration::from_secs(5),
        budget: std::time::Duration::ZERO,
    };
    // Without a transcript path, the hook sweeps the adoption.
    let w = adopt::sweep_hook(&h.store, &h.state, claude::HARNESS, None, &none)
        .unwrap()
        .unwrap();
    assert!(w.left >= 1, "{w:?}");
    let w = adopt::sweep(&h.store, &h.state, None).unwrap();
    assert_eq!(w[0].imported.len() + w[0].unchanged, 2);
    for id in [CLAUDE_A, CLAUDE_B] {
        assert!(
            h.store.host_session(claude::HARNESS, id).unwrap().is_some(),
            "{id}"
        );
    }
}

mod crash_tests;
