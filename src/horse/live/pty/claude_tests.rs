//! The Claude Code TUI custody through the pty, over a scripted screen, a
//! recording writer in place of the pty, hooks marked as the broker marks
//! them, and a transcript that the test writes as Claude Code would. The
//! test steps the custodian as the driver does: a pump, then the oldest
//! inbox entry when the session is at a safe point.

use super::*;
use crate::horse::live::channel::{CustodyCtx as TuiCtx, Timing};
use crate::horse::live::claude::fake::{assistant_message, chain, user_message, CWD, INTERRUPTED};
use crate::horse::live::pty::Screen;
use crate::horse::live::tui_state::{Runs, SafePoint};
use crate::relay::Tap as _;
use std::path::PathBuf;
use std::sync::Mutex;

const NATIVE: &str = "01934b20-1111-7777-8888-123456789abc";

/// The idle screen of Claude Code: the input row between two rules, the
/// prompt, the first letter of the placeholder inverse (the cursor that
/// Claude Code draws), the rest dim.
pub(crate) const IDLE: &[u8] = "\x1b[2J\x1b[H\u{2500}\u{2500}\u{2500}\u{2500}\r\n\u{276f} \x1b[7mT\x1b[0m\x1b[2mry \"fix lint errors\"\x1b[0m\r\n\u{2500}\u{2500}\u{2500}\u{2500}\r\n  ? for shortcuts".as_bytes();
/// A turn runs: the spinner row with the busy text, and the input row.
pub(crate) const BUSY: &[u8] = "\x1b[2J\x1b[H\u{273b} Thinking\u{2026} (3s \u{b7} esc to interrupt)\r\n\u{2500}\u{2500}\u{2500}\u{2500}\r\n\u{276f} \x1b[7m \x1b[0m\r\n\u{2500}\u{2500}\u{2500}\u{2500}".as_bytes();

pub(crate) fn claude_idle() -> PtyIdle {
    let p = crate::profile::builtin("claude").unwrap();
    PtyIdle::of(&p, "claude", codec::VERSION).unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    session: String,
    file: PathBuf,
    runs: &'static Runs,
    key: String,
    tap: Arc<PtyTap>,
    keys: Arc<Mutex<Vec<Vec<u8>>>>,
    ledger: Arc<Mutex<Vec<String>>>,
    n: usize,
    parent: Option<String>,
}

fn timing() -> Timing {
    Timing {
        tick: Duration::from_millis(1),
        idle_after: Duration::ZERO,
        busy_ingest: Duration::from_secs(600),
        land_wait: Duration::from_secs(600),
    }
}

fn world(t: Timing) -> (World, Custodian) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let s = store
        .new_session(&h.id, codec::binding(Some(NATIVE.into())))
        .unwrap();
    let config_dir = dir.path().join("home/.claude");
    let file = codec::transcript_path(&config_dir, CWD, NATIVE);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let runs: &'static Runs = Box::leak(Box::default());
    let key = run_key(&h.id, &s.id);
    let signals = runs.subscribe(&key);
    let tap = PtyTap::new();
    let keys = Arc::new(Mutex::new(Vec::new()));
    let k = keys.clone();
    tap.attach_with(24, 80, move |b: &[u8]| {
        k.lock().unwrap().push(b.to_vec());
        Ok(())
    });
    let ledger = Arc::new(Mutex::new(Vec::new()));
    let l = ledger.clone();
    let (_close, control) = std::sync::mpsc::channel();
    let mut idle = claude_idle();
    idle.quiet = Duration::ZERO;
    idle.submit_gap = Duration::ZERO;
    let tui = TuiCtx {
        store: store.clone(),
        horse: h.id.clone(),
        session: s.id.clone(),
        native: NATIVE.into(),
        config_dir,
        cwd: CWD.into(),
        version: codec::VERSION.into(),
        run: "sandbox-1".into(),
        runs,
        ledger: Box::new(move |line: &str| l.lock().unwrap().push(line.into())),
        control,
        timing: t,
    };
    let c = Custodian::new(tui, idle, tap.clone(), signals).unwrap();
    let w = World {
        _dir: dir,
        store,
        session: s.id,
        file,
        runs,
        key,
        tap,
        keys,
        ledger,
        n: 0,
        parent: None,
    };
    (w, c)
}

impl World {
    fn screen(&self, b: &[u8]) {
        self.tap.output_bytes(b);
    }
    fn hook(&self, kind: SafePoint) {
        self.runs.mark(&self.key, kind);
    }
    fn rows(&mut self, rows: &[(&str, serde_json::Value)]) {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .unwrap();
        for (ty, m) in rows {
            self.n += 1;
            let uuid = format!("r{}", self.n);
            f.write_all(&chain(ty, &uuid, self.parent.as_deref(), m, NATIVE))
                .unwrap();
            f.write_all(b"\n").unwrap();
            self.parent = Some(uuid);
        }
    }
    /// The start of a turn on `text`: its hook, the busy screen, its row.
    fn begin(&mut self, text: &str) {
        self.hook(SafePoint::Prompt);
        self.screen(BUSY);
        self.rows(&[("user", user_message(text))]);
    }
    /// The end of the turn: the reply, the idle screen, the `Stop` hook.
    fn end(&mut self) {
        let n = self.n;
        self.rows(&[("assistant", assistant_message(n, "ok"))]);
        self.screen(IDLE);
        self.hook(SafePoint::Stop);
    }
    fn typed(&self) -> Vec<Vec<u8>> {
        self.keys.lock().unwrap().clone()
    }
    fn landed(&self) -> usize {
        self.store
            .deliveries(&self.session)
            .unwrap()
            .iter()
            .filter(|d| d.landed_seq.is_some())
            .count()
    }
    fn lines(&self, prefix: &str) -> usize {
        let l = self.ledger.lock().unwrap();
        l.iter().filter(|l| l.starts_with(prefix)).count()
    }
}

/// One pass of the driver. True when an entry went out.
fn pass(w: &World, c: &mut Custodian) -> bool {
    c.pump(Duration::from_millis(1)).unwrap();
    if !c.safe(Some(&w.session)) {
        return false;
    }
    // The driver takes an urgent entry first, else the oldest.
    let inbox = w.store.inbox(&w.session).unwrap();
    let next = inbox.iter().find(|e| e.interrupt).or(inbox.first());
    match next {
        Some(e) => {
            c.deliver(Input::Entry(e.clone())).unwrap();
            true
        }
        None => false,
    }
}

#[test]
fn the_claude_profile_has_the_table_for_the_codec_version_only() {
    let idle = claude_idle();
    assert_eq!(idle.prompt, "\u{276f}");
    assert_eq!(idle.busy, vec!["esc to interrupt".to_string()]);
    assert!(idle.empty_input && idle.bracketed_paste);
    assert_eq!(idle.quiet, Duration::from_millis(1500));
    assert_eq!(
        (idle.interrupt_key, idle.submit_key),
        (b"\x1b".to_vec(), b"\r".to_vec())
    );
    let p = crate::profile::builtin("claude").unwrap();
    let e = PtyIdle::of(&p, "claude", "2.1.318").unwrap_err();
    assert_eq!(
        format!("{e:#}"),
        "horse: no pty delivery pattern for claude 2.1.318"
    );
}

#[test]
fn the_cursor_that_claude_code_draws_is_not_text_of_the_user() {
    let p = claude_idle();
    let quiet = p.quiet;
    let s = |b: &[u8]| Screen::from_bytes(24, 80, b);
    assert!(is_idle(&s(IDLE), &p, quiet));
    assert!(!is_idle(&s(BUSY), &p, quiet), "the busy row");
    // The placeholder is gone: the cursor is an inverse blank.
    let hidden = "\x1b[H\u{276f} \x1b[7m \x1b[0m".as_bytes();
    assert!(is_idle(&s(hidden), &p, quiet));
    // A draft, with the cursor after it or on its last letter.
    let draft = "\x1b[H\u{276f} fix\x1b[7m \x1b[0m".as_bytes();
    assert!(!is_idle(&s(draft), &p, quiet));
    let on = "\x1b[H\u{276f} fi\x1b[7mx\x1b[0m".as_bytes();
    assert!(!is_idle(&s(on), &p, quiet));
    // One letter with the cursor on it: no dim text follows it.
    let one = "\x1b[H\u{276f} \x1b[7mx\x1b[0m".as_bytes();
    assert!(!is_idle(&s(one), &p, quiet));
}

#[test]
fn an_entry_goes_out_only_after_the_stop_of_the_turn_and_lands() {
    let (mut w, mut c) = world(timing());
    // The TUI state is busy until the first Stop, also on an idle screen.
    w.screen(IDLE);
    w.store.queue_append(&w.session, "user", "first").unwrap();
    assert!(!pass(&w, &mut c), "no Stop yet");
    w.begin("hi");
    let e = w.store.queue_append(&w.session, "user", "queued").unwrap();
    for _ in 0..3 {
        assert!(!pass(&w, &mut c), "a turn runs");
    }
    // The reply is there but the Stop is not, and the screen is busy.
    w.rows(&[("assistant", assistant_message(1, "ok"))]);
    w.screen(BUSY);
    w.screen(IDLE);
    w.screen(BUSY);
    assert!(!pass(&w, &mut c), "the screen shows a turn");
    assert!(w.typed().is_empty());
    w.screen(IDLE);
    w.hook(SafePoint::Stop);
    assert!(pass(&w, &mut c));
    assert_eq!(
        w.typed(),
        vec![b"\x1b[200~first\x1b[201~".to_vec(), b"\r".to_vec()]
    );
    // Typed: busy until the Stop of its turn, also with the old screen.
    assert!(!pass(&w, &mut c));
    w.begin("first");
    w.end();
    assert!(pass(&w, &mut c), "the next entry goes out");
    assert_eq!(w.typed()[2], b"\x1b[200~queued\x1b[201~".to_vec());
    w.begin("queued");
    w.end();
    pass(&w, &mut c);
    assert_eq!(w.landed(), 2);
    let d = w.store.deliveries(&w.session).unwrap();
    assert!(d
        .iter()
        .any(|d| d.entry.id == e.id && d.landed_seq.is_some()));
    assert_eq!(w.lines("pty.deliver"), 2);
    let r = c.report();
    assert_eq!((r.tui.released, r.tui.landed, r.interrupts), (2, 2, 0));
    assert!(!r.broken);
}

#[test]
fn an_urgent_entry_sends_the_interrupt_key_then_goes_out_at_idle() {
    let (mut w, mut c) = world(timing());
    w.begin("hi");
    w.end();
    pass(&w, &mut c);
    // The user starts a long turn, and an urgent entry comes.
    w.begin("long");
    let plain = w.store.queue_append(&w.session, "user", "later").unwrap();
    let e = w
        .store
        .queue_entry(&w.session, "user", "stop now", true)
        .unwrap();
    for _ in 0..3 {
        assert!(!pass(&w, &mut c));
    }
    assert_eq!(w.typed(), vec![b"\x1b".to_vec()], "one interrupt key");
    assert_eq!(w.lines("pty.interrupt"), 1);
    // Claude Code ends the turn with its interrupt row and runs no Stop
    // hook. The row says idle, then the screen does.
    w.rows(&[("user", user_message(INTERRUPTED))]);
    assert!(!pass(&w, &mut c), "the screen is still busy");
    w.screen(IDLE);
    assert!(pass(&w, &mut c));
    // The urgent entry goes first, then the older plain one.
    let d = w.store.deliveries(&w.session).unwrap();
    assert_eq!(d[0].entry.id, e.id);
    assert!(d[0].entry.interrupt);
    w.begin("stop now");
    w.end();
    assert!(pass(&w, &mut c));
    let d = w.store.deliveries(&w.session).unwrap();
    assert!(d.iter().any(|d| d.entry.id == plain.id));
    assert_eq!(c.report().interrupts, 1);
    assert_eq!(c.report().tui.missed_hooks, 1, "the Stop of the interrupt");
}

#[test]
fn a_typed_entry_that_starts_no_turn_goes_out_once_more() {
    let mut t = timing();
    t.land_wait = Duration::ZERO;
    let (mut w, mut c) = world(t);
    w.begin("hi");
    w.end();
    pass(&w, &mut c);
    let e = w.store.queue_append(&w.session, "user", "lost").unwrap();
    assert!(pass(&w, &mut c));
    // No turn came: the entry is back in the inbox, and goes out again.
    let mut again = false;
    for _ in 0..5 {
        again |= pass(&w, &mut c);
    }
    assert!(again);
    assert_eq!(w.lines("pty.retry"), 1);
    assert_eq!(c.report().tui.retried, 1);
    let inbox = w.store.inbox(&w.session).unwrap();
    assert!(inbox.iter().all(|i| i.id != e.id));
}

#[test]
fn a_relay_that_is_gone_keeps_the_entry_queued() {
    let (mut w, mut c) = world(timing());
    w.begin("hi");
    w.end();
    pass(&w, &mut c);
    w.tap.finish();
    let e = w.store.queue_append(&w.session, "user", "kept").unwrap();
    assert!(pass(&w, &mut c));
    assert!(w.store.deliveries(&w.session).unwrap().is_empty());
    assert_eq!(w.store.inbox(&w.session).unwrap()[0].id, e.id);
    assert!(c.report().broken);
    assert!(!c.safe(None), "nothing more is typed");
}
