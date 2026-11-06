//! The TUI custody over `fake-claude-tui`: a fake interactive Claude Code
//! in threads of the test. It reads the command line that [`prepare`]
//! gives, runs the `Stop` and `UserPromptSubmit` hooks of its
//! `--settings` as the host gets them (`horse.safe_point`), starts the
//! MCP server of its `--mcp-config` (`paddock-host channel`, in a thread,
//! on pipes, with the broker handler of the host as its broker), reads
//! the channel notifications, and writes its transcript as Claude Code
//! does. It contacts nothing.

use super::*;
use crate::broker::{Request, Response, PROTOCOL};
use crate::horse::claude::CHANNEL_VERSIONS;
use crate::horse::device::Access;
use crate::horse::live::claude::fake::{assistant_message, chain, user_message, CWD};
use crate::horse::live::tui_state::{self, SAFE_POINT};
use crate::hostcli::horse::channel as server;
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Write as _};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

const NATIVE: &str = "01934b20-1111-7777-8888-123456789abc";

fn timing() -> Timing {
    Timing {
        tick: Duration::from_millis(20),
        idle_after: Duration::from_millis(200),
        busy_ingest: Duration::from_millis(100),
        land_wait: Duration::from_millis(400),
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The host side: a store with the run's horse and session, the TUI
/// states, the ledger, and the custody loop in its thread.
struct Host {
    _dir: tempfile::TempDir,
    store: Store,
    access: Access,
    runs: &'static Runs,
    ledger: Arc<Mutex<Vec<String>>>,
    config_dir: PathBuf,
    close: Sender<Control>,
    join: Option<std::thread::JoinHandle<Result<Report>>>,
}

impl Host {
    fn start() -> Host {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store")).unwrap();
        let h = store.new_default_horse().unwrap();
        let s = store
            .new_session(&h.id, codec::binding(Some(NATIVE.into())))
            .unwrap();
        let runs: &'static Runs = Box::leak(Box::default());
        let ledger = Arc::new(Mutex::new(Vec::new()));
        let config_dir = dir.path().join("home/.claude");
        let (close, control) = channel();
        let l2 = ledger.clone();
        let ctx = CustodyCtx {
            store: store.clone(),
            horse: h.id.clone(),
            session: s.id.clone(),
            native: NATIVE.into(),
            config_dir: config_dir.clone(),
            cwd: CWD.into(),
            version: codec::VERSION.into(),
            run: "tui-run".into(),
            runs,
            ledger: Box::new(move |l: &str| l2.lock().unwrap().push(l.to_string())),
            control,
            timing: timing(),
        };
        let signals = runs.subscribe(&run_key(&h.id, &s.id));
        let join = std::thread::spawn(move || run(ctx, signals));
        Host {
            _dir: dir,
            access: Access {
                store: store.clone(),
                horse: h.id,
                session: s.id,
            },
            store,
            runs,
            ledger,
            config_dir,
            close,
            join: Some(join),
        }
    }

    /// The broker of the host, for the sandbox of this run.
    fn broker(&self) -> impl Fn(&Request) -> Response + Send + Sync + 'static {
        let (runs, access, ledger) = (self.runs, self.access.clone(), self.ledger.clone());
        move |req: &Request| {
            let record = |l: &str| ledger.lock().unwrap().push(l.to_string());
            tui_state::handle_in(runs, Some(&access), req, &record)
        }
    }

    fn state(&self) -> tui_state::TuiState {
        self.runs
            .state(&run_key(&self.access.horse, &self.access.session))
    }

    fn queue(&self, text: &str) -> crate::horse::inbox::Entry {
        self.store
            .queue_append(&self.access.session, "user", text)
            .unwrap()
    }

    fn deliveries(&self) -> Vec<crate::horse::inbox::Delivery> {
        self.store.deliveries(&self.access.session).unwrap()
    }

    fn landed(&self, id: &str) -> bool {
        self.deliveries()
            .iter()
            .any(|d| d.entry.id == id && d.landed_seq.is_some())
    }

    fn lines(&self, prefix: &str) -> usize {
        self.ledger
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with(prefix))
            .count()
    }

    /// The end of the run.
    fn end(mut self) -> (Report, Host) {
        self.close.send(Control::Close).unwrap();
        self.close.send(Control::Close).unwrap();
        let r = self.join.take().unwrap().join().unwrap().unwrap();
        (r, self)
    }
}

/// `fake-claude-tui`: what Claude Code does with the command line of a
/// TUI run in horse mode.
struct FakeTui {
    hooks: Vec<(String, String)>,
    file: PathBuf,
    /// The notifications of the channel server.
    pushes: std::sync::mpsc::Receiver<Value>,
    to_server: std::io::PipeWriter,
    rows: Vec<Vec<u8>>,
    parent: Option<String>,
    n: usize,
    broker: Box<dyn Fn(&Request) -> Response + Send + Sync>,
}

impl FakeTui {
    fn start(host: &Host) -> FakeTui {
        let mut command = vec!["claude".to_string()];
        let setup = prepare(NATIVE, &CHANNEL_VERSIONS[0]);
        apply(&mut command, &setup).unwrap();
        let value = |flag: &str| {
            let eq = format!("{flag}=");
            command
                .iter()
                .find_map(|a| a.strip_prefix(&eq).map(str::to_string))
                .or_else(|| {
                    let at = command.iter().position(|a| a == flag)?;
                    command.get(at + 1).cloned()
                })
                .unwrap_or_else(|| panic!("no {flag} in {command:?}"))
        };
        assert_eq!(value("--session-id"), NATIVE);
        assert_eq!(value("--channels"), "server:paddock");
        assert_eq!(
            value("--dangerously-load-development-channels"),
            "server:paddock"
        );
        let settings: Value = serde_json::from_str(&value("--settings")).unwrap();
        let mut hooks = Vec::new();
        for (event, groups) in settings["hooks"].as_object().unwrap() {
            for g in groups.as_array().unwrap() {
                for h in g["hooks"].as_array().unwrap() {
                    hooks.push((event.clone(), h["command"].as_str().unwrap().to_string()));
                }
            }
        }
        // The MCP server of the channel, as its config names it.
        let mcp: Value = serde_json::from_str(&value("--mcp-config")).unwrap();
        let s = &mcp["mcpServers"]["paddock"];
        assert_eq!(
            (s["command"].as_str(), s["args"].clone()),
            (Some("paddock-host"), json!(["channel"]))
        );
        let broker = Arc::new(host.broker());
        let (from_client, to_server) = std::io::pipe().unwrap();
        let (from_server, to_client) = std::io::pipe().unwrap();
        let out: Arc<Mutex<dyn std::io::Write + Send>> = Arc::new(Mutex::new(to_client));
        let b2 = broker.clone();
        let call: server::Call = Arc::new(move |req: &Request| Ok(b2(req)));
        let cfg = server::Config {
            wait: Duration::from_millis(100),
            backoff: Duration::from_millis(10),
            attempts: 3,
        };
        std::thread::spawn(move || server::serve(BufReader::new(from_client), out, call, cfg));
        let (tx, pushes) = channel();
        let (init_tx, init_rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(from_server).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["method"] == server::NOTIFICATION {
                    if tx.send(v).is_err() {
                        break;
                    }
                } else {
                    let _ = init_tx.send(v);
                }
            }
        });
        let mut tui = FakeTui {
            hooks,
            file: codec::transcript_path(&host.config_dir, CWD, NATIVE),
            pushes,
            to_server,
            rows: Vec::new(),
            parent: None,
            n: 0,
            broker: Box::new(move |r: &Request| broker(r)),
        };
        tui.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "fake-claude-tui"}}}));
        let init = init_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(init["result"]["capabilities"]["experimental"]["claude/channel"].is_object());
        tui.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        std::fs::create_dir_all(tui.file.parent().unwrap()).unwrap();
        tui
    }

    fn send(&mut self, v: Value) {
        let mut l = serde_json::to_vec(&v).unwrap();
        l.push(b'\n');
        self.to_server.write_all(&l).unwrap();
    }

    /// Run the hooks of `event` as Claude Code would: each command once.
    fn hook(&self, event: &str) {
        let mut ran = 0;
        for (e, cmd) in &self.hooks {
            if e != event {
                continue;
            }
            // paddock's status hooks are not in this command line.
            let kind = cmd
                .strip_prefix("paddock-host horse safe-point ")
                .and_then(|k| k.strip_suffix(" || true"))
                .unwrap_or_else(|| panic!("an unknown hook {cmd:?}"));
            let req = Request {
                v: PROTOCOL,
                cap: "horse".into(),
                args: [
                    ("op".to_string(), json!(SAFE_POINT)),
                    ("kind".to_string(), json!(kind)),
                ]
                .into_iter()
                .collect(),
                reason: String::new(),
            };
            assert_eq!((self.broker)(&req).status, crate::broker::Status::Done);
            ran += 1;
        }
        assert_eq!(ran, 1, "one {event} hook");
    }

    fn write_rows(&mut self, rows: &[Vec<u8>]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .unwrap();
        for r in rows {
            f.write_all(r).unwrap();
            f.write_all(b"\n").unwrap();
        }
        self.rows.extend(rows.iter().cloned());
    }

    /// One turn on `text`, with or without its hooks.
    fn turn(&mut self, text: &str, hooks: bool) {
        self.n += 1;
        let (u, a) = (format!("u{}", self.n), format!("a{}", self.n));
        if hooks {
            self.hook("UserPromptSubmit");
        }
        let user = chain(
            "user",
            &u,
            self.parent.as_deref(),
            &user_message(text),
            NATIVE,
        );
        self.write_rows(&[user]);
        let reply = chain(
            "assistant",
            &a,
            Some(&u),
            &assistant_message(self.n, "ok"),
            NATIVE,
        );
        self.write_rows(&[reply]);
        self.parent = Some(a);
        if hooks {
            self.hook("Stop");
        }
    }

    /// The next channel push, as Claude Code shows it to the model.
    fn push(&self, within: Duration) -> Option<String> {
        let v = self.pushes.recv_timeout(within).ok()?;
        let p = &v["params"];
        let meta: String = p["meta"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| format!(" {k}=\"{}\"", v.as_str().unwrap()))
            .collect();
        Some(format!(
            "<channel source=\"paddock\"{meta}>\n{}\n</channel>",
            p["content"].as_str().unwrap()
        ))
    }
}

#[test]
fn an_entry_queued_during_a_turn_goes_out_after_stop_and_lands() {
    let host = Host::start();
    let mut tui = FakeTui::start(&host);
    // A turn of the user; an entry comes while it runs.
    tui.hook("UserPromptSubmit");
    let e = host.queue("from the host");
    tui.write_rows(&[chain("user", "u0", None, &user_message("hi"), NATIVE)]);
    assert_eq!(
        tui.push(Duration::from_millis(300)),
        None,
        "pushed during a turn"
    );
    assert!(host.deliveries().is_empty());
    tui.write_rows(&[chain(
        "assistant",
        "a0",
        Some("u0"),
        &assistant_message(0, "hello"),
        NATIVE,
    )]);
    tui.parent = Some("a0".into());
    tui.hook("Stop");
    // Now it goes out, and the TUI counts as busy again.
    let text = tui
        .push(Duration::from_secs(10))
        .expect("a push after stop");
    assert!(text.contains(&format!("entry_id=\"{}\"", e.id)), "{text}");
    assert!(text.contains("from the host"), "{text}");
    assert!(host.state().busy);
    assert_eq!(host.lines("channel.release"), 1);
    tui.turn(&text, true);
    wait_until("the landing", || host.landed(&e.id));
    let (r, host) = host.end();
    assert_eq!((r.released, r.landed, r.retried), (1, 1, 0), "{r:?}");
    assert!(r.not_landed.is_empty());
    assert_eq!((r.stops, r.prompts, r.missed_hooks), (2, 2, 0), "{r:?}");
    let s = host.store.session(&host.access.session).unwrap();
    assert_eq!(host.store.session_nodes(&s).unwrap().len(), 4);
    // The live record went with the custody.
    assert!(crate::horse::live::records(&host.store).unwrap().is_empty());
}

#[test]
fn a_dropped_push_is_retried_once_then_stays_not_landed() {
    let host = Host::start();
    let mut tui = FakeTui::start(&host);
    tui.turn("hi", true);
    let e = host.queue("lost twice");
    // The fake drops both pushes of the entry.
    assert!(tui.push(Duration::from_secs(10)).is_some());
    assert!(
        tui.push(Duration::from_secs(10)).is_some(),
        "no second push"
    );
    wait_until("the second miss", || host.lines("channel.lost") == 1);
    assert_eq!(host.lines("channel.release"), 2);
    assert_eq!(host.lines("channel.retry"), 1);
    assert_eq!(tui.push(Duration::from_millis(600)), None, "a third push");
    let d = host.deliveries();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(
        (d[0].entry.id.as_str(), d[0].landed_seq),
        (e.id.as_str(), None)
    );
    assert!(host.store.inbox(&host.access.session).unwrap().is_empty());
    // The TUI is idle again: the next entry goes out and lands.
    let next = host.queue("this one arrives");
    let text = tui.push(Duration::from_secs(10)).expect("the next push");
    assert!(text.contains("this one arrives"));
    tui.turn(&text, true);
    wait_until("the landing", || host.landed(&next.id));
    let (r, _host) = host.end();
    assert_eq!((r.released, r.landed, r.retried), (3, 1, 1), "{r:?}");
    assert_eq!(r.not_landed, vec![e.id]);
}

#[test]
fn the_file_covers_missed_hooks() {
    let host = Host::start();
    let mut tui = FakeTui::start(&host);
    // No hook at all: the complete reply, unchanged, makes the TUI idle.
    tui.turn("hi", false);
    let e = host.queue("after a missed stop");
    let text = tui
        .push(Duration::from_secs(10))
        .expect("a push after the file said idle");
    assert!(text.contains("after a missed stop"));
    tui.turn(&text, false);
    wait_until("the landing", || host.landed(&e.id));
    // A prompt without its hook makes the TUI busy at once.
    wait_until("idle", || !host.state().busy);
    tui.write_rows(&[chain(
        "user",
        "u9",
        tui.parent.clone().as_deref(),
        &user_message("typed"),
        NATIVE,
    )]);
    wait_until("busy", || host.state().busy);
    let held = host.queue("held");
    assert_eq!(tui.push(Duration::from_millis(100)), None);
    let (r, host) = host.end();
    assert!(r.missed_hooks >= 3, "{r:?}");
    assert_eq!((r.stops, r.prompts), (0, 0), "{r:?}");
    assert_eq!(host.store.inbox(&host.access.session).unwrap(), vec![held]);
}

#[test]
fn a_rewritten_transcript_loses_no_node() {
    let host = Host::start();
    let mut tui = FakeTui::start(&host);
    for t in ["one", "two", "three"] {
        tui.turn(t, true);
    }
    let s = || host.store.session(&host.access.session).unwrap();
    wait_until("three turns in the store", || {
        host.store.session_nodes(&s()).unwrap().len() == 6
    });
    // Claude Code compacts: a new file (new inode) without the first rows.
    let kept: Vec<Vec<u8>> = tui.rows[4..].to_vec();
    let tmp = tui.file.with_extension("compact.tmp");
    std::fs::write(&tmp, [kept.join(&b"\n"[..]), b"\n".to_vec()].concat()).unwrap();
    std::fs::rename(&tmp, &tui.file).unwrap();
    tui.rows = kept;
    tui.turn("four", true);
    // A torn last line waits for its newline.
    let torn = chain(
        "user",
        "u5",
        tui.parent.as_deref(),
        &user_message("five"),
        NATIVE,
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&tui.file)
        .unwrap()
        .write_all(&torn[..10])
        .unwrap();
    tui.hook("Stop");
    wait_until("the fourth turn", || {
        host.store.session_nodes(&s()).unwrap().len() == 8
    });
    let (r, host) = host.end();
    assert_eq!(r.ingest_errors, 0, "{r:?}");
    let nodes = host
        .store
        .session_nodes(&host.store.session(&host.access.session).unwrap())
        .unwrap();
    let keys: Vec<String> = nodes.iter().filter_map(|n| n.meta.key.clone()).collect();
    for u in ["u1", "a1", "u2", "a2", "u3", "a3", "u4", "a4"] {
        assert!(keys.contains(&format!("uuid:{u}")), "{u} lost: {keys:?}");
    }
    assert_eq!(nodes.len(), 8);
}

#[test]
fn the_command_gets_the_channel_and_keeps_the_rest() {
    let pin = CHANNEL_VERSIONS[0];
    let s = prepare(NATIVE, &pin);
    let mut c: Vec<String> = [
        "claude",
        "--settings",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine"}]}]}}"#,
        "--model",
        "m",
    ]
    .map(String::from)
    .to_vec();
    apply(&mut c, &s).unwrap();
    assert_eq!(&c[1..4], &s.args[..3]);
    assert_eq!(c[c.len() - 2..], ["--model".to_string(), "m".to_string()]);
    let at = c.iter().position(|a| a == "--settings").unwrap();
    let v: Value = serde_json::from_str(&c[at + 1]).unwrap();
    assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "mine");
    assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], STOP_HOOK);
    assert_eq!(
        v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        PROMPT_HOOK
    );
    // A command that picks its own session or channel cannot get one.
    for flag in [
        "--resume",
        "-c",
        "--session-id=x",
        "--channels",
        "--mcp-config",
    ] {
        let mut c = vec!["claude".to_string(), flag.to_string()];
        assert!(conflict(&c).is_some(), "{flag}");
        assert!(apply(&mut c, &s).is_err());
        assert_eq!(c.len(), 2, "{flag}: the command changed");
    }
    assert_eq!(
        conflict(&["claude".into(), "--model".into(), "m".into()]),
        None
    );
}
