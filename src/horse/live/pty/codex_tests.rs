//! The Codex TUI custody over a scripted screen, a recording writer in
//! place of the pty, and a rollout file that the test writes as Codex
//! would. The test steps the custodian as the driver does: a pump, then
//! the oldest inbox entry when the session is at a safe point.

use super::*;
use crate::horse::codex as codec;
use crate::horse::live::codex::fake::{message, meta, rollout_path, row, write_rollout};
use crate::horse::live::pty::tests::{codex_idle, BUSY, IDLE};
use crate::horse::Binding;
use serde_json::json;
use std::sync::Mutex;

const THREAD: &str = "019a5f00-0000-7000-8000-000000000001";
const OTHER: &str = "019a5f00-0000-7000-8000-0000000000aa";

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    session: String,
    root: PathBuf,
    runs: &'static Runs,
    key: String,
    tap: Arc<PtyTap>,
    keys: Arc<Mutex<Vec<Vec<u8>>>>,
    ledger: Arc<Mutex<Vec<String>>>,
}

fn timing() -> Timing {
    Timing {
        tick: Duration::from_millis(1),
        busy_ingest: Duration::from_secs(600),
        land_wait: Duration::from_secs(600),
        missed_stop: Duration::from_secs(600),
        no_stop: Duration::from_secs(600),
    }
}

fn world(t: Timing, attach: bool) -> (World, Custodian) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let s = store
        .new_session(
            &h.id,
            Binding {
                harness: codec::HARNESS.into(),
                native_id: None,
                version: codec::VERSION.into(),
                container: codec::CONTAINER.into(),
            },
        )
        .unwrap();
    let root = dir.path().join("home/.codex/sessions");
    let runs: &'static Runs = Box::leak(Box::default());
    let key = run_key(&h.id, &s.id);
    let signals = runs.subscribe(&key);
    let tap = PtyTap::new();
    let keys = Arc::new(Mutex::new(Vec::new()));
    if attach {
        let k = keys.clone();
        tap.attach_with(24, 80, move |b: &[u8]| {
            k.lock().unwrap().push(b.to_vec());
            Ok(())
        });
    }
    let ledger = Arc::new(Mutex::new(Vec::new()));
    let l = ledger.clone();
    let (_close, control) = std::sync::mpsc::channel();
    let mut idle = codex_idle();
    idle.quiet = Duration::ZERO;
    idle.submit_gap = Duration::ZERO;
    let ctx = CustodyCtx {
        store: store.clone(),
        horse: h.id.clone(),
        session: s.id.clone(),
        sessions_root: root.clone(),
        version: codec::VERSION.into(),
        run: "sandbox-1".into(),
        runs,
        ledger: Box::new(move |line: &str| l.lock().unwrap().push(line.into())),
        control,
        timing: t,
        idle,
        tap: tap.clone(),
    };
    let c = Custodian::new(ctx, signals).unwrap();
    let w = World {
        _dir: dir,
        store,
        session: s.id,
        root,
        runs,
        key,
        tap,
        keys,
        ledger,
    };
    (w, c)
}

impl World {
    fn screen(&self, b: &[u8]) {
        self.tap.output_bytes(b);
    }
    fn stop(&self, thread: Option<&str>) {
        self.runs.mark_thread(&self.key, SafePoint::Stop, thread);
    }
    /// The rollout of the thread with its first turn: a user row, a reply,
    /// the end of the turn.
    fn first_turn(&self) {
        write_rollout(
            &self.root,
            THREAD,
            &[
                meta(THREAD),
                row("response_item", message("user", "input_text", "hello")),
                row(
                    "event_msg",
                    json!({"type": "user_message", "message": "hello"}),
                ),
                row("response_item", message("assistant", "output_text", "hi")),
                row(
                    "event_msg",
                    json!({"type": "task_complete", "turn_id": "t1"}),
                ),
            ],
        );
    }
    fn append(&self, rows: &[Vec<u8>]) {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(rollout_path(&self.root, THREAD))
            .unwrap();
        for r in rows {
            f.write_all(r).unwrap();
            f.write_all(b"\n").unwrap();
        }
    }
    /// The rows of a turn that an entry started.
    fn turn_of(&self, text: &str, end: &str) {
        self.append(&[
            row("response_item", message("user", "input_text", text)),
            row(
                "event_msg",
                json!({"type": "user_message", "message": text}),
            ),
            row("event_msg", json!({"type": end, "turn_id": "t2"})),
        ]);
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
}

/// One pass of the driver: a pump, then the oldest entry at a safe point.
fn pass(w: &World, c: &mut Custodian) -> bool {
    c.pump(Duration::from_millis(1)).unwrap();
    if !c.safe(Some(&w.session)) {
        return false;
    }
    match w.store.inbox(&w.session).unwrap().into_iter().next() {
        Some(e) => {
            c.deliver(Input::Entry(e)).unwrap();
            true
        }
        None => false,
    }
}

#[test]
fn a_stop_without_an_idle_screen_does_not_deliver() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.store.queue_append(&w.session, "user", "queued").unwrap();
    w.screen(BUSY);
    w.stop(Some(THREAD));
    for _ in 0..5 {
        assert!(!pass(&w, &mut c));
    }
    assert!(w.typed().is_empty());
    // The stop bound the session to its thread and ingested the turn.
    let s = w.store.session(&w.session).unwrap();
    assert_eq!(s.binding.native_id.as_deref(), Some(THREAD));
    assert_eq!(w.store.session_nodes(&s).unwrap().len(), 5);
    assert_eq!(c.report().stops, 1);
    // The screen goes idle: now it delivers.
    w.screen(IDLE);
    assert!(pass(&w, &mut c));
}

#[test]
fn an_idle_screen_without_a_stop_after_a_newer_user_row_does_not_deliver() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    assert!(c.safe(None), "idle after the stop");
    // The user starts a turn in the TUI; the screen is idle-looking again.
    w.append(&[
        row("response_item", message("user", "input_text", "mine")),
        row(
            "event_msg",
            json!({"type": "user_message", "message": "mine"}),
        ),
    ]);
    w.store.queue_append(&w.session, "user", "queued").unwrap();
    for _ in 0..5 {
        assert!(!pass(&w, &mut c));
    }
    assert!(w.typed().is_empty());
    // Its stop comes: the entry goes out.
    w.append(&[row("event_msg", json!({"type": "task_complete"}))]);
    w.stop(Some(THREAD));
    assert!(pass(&w, &mut c));
}

#[test]
fn a_stop_and_an_idle_screen_deliver_once_and_the_rollout_lands_it() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.screen(IDLE);
    let e = w
        .store
        .queue_append(&w.session, "user", "from paddock")
        .unwrap();
    w.stop(Some(THREAD));
    assert!(pass(&w, &mut c));
    for _ in 0..5 {
        assert!(!pass(&w, &mut c), "delivered twice");
    }
    assert_eq!(
        w.typed(),
        vec![b"\x1b[200~from paddock\x1b[201~".to_vec(), b"\r".to_vec()]
    );
    assert!(w.store.inbox(&w.session).unwrap().is_empty());
    assert_eq!(w.landed(), 0);
    w.turn_of("from paddock", "task_complete");
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    assert_eq!(w.landed(), 1);
    assert_eq!(c.report().landed, 1);
    assert_eq!(c.report().typed, 1);
    let d = &w.store.deliveries(&w.session).unwrap()[0];
    assert_eq!(d.entry.id, e.id);
    assert_eq!(d.landed_seq, Some(5));
    assert!(w
        .ledger
        .lock()
        .unwrap()
        .contains(&format!("pty.deliver {}", e.id)));
    c.finish(End::Closed).unwrap();
    assert!(c.report().not_landed.is_empty());
}

#[test]
fn a_stop_of_another_thread_is_ignored() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    write_rollout(&w.root, OTHER, &[meta(OTHER)]);
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    w.store.queue_append(&w.session, "user", "one").unwrap();
    assert!(pass(&w, &mut c));
    // A turn of another thread ends while the entry runs.
    w.stop(Some(OTHER));
    for _ in 0..3 {
        assert!(!pass(&w, &mut c));
    }
    assert_eq!(c.report().other_stops, 1);
    // Only the stop of its own thread counted.
    assert_eq!(c.report().stops, 1);
}

#[test]
fn a_turn_end_in_the_file_covers_a_missed_stop() {
    let mut t = timing();
    t.missed_stop = Duration::ZERO;
    let (w, mut c) = world(t, true);
    w.first_turn();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    w.store.queue_append(&w.session, "user", "one").unwrap();
    assert!(pass(&w, &mut c));
    w.store.queue_append(&w.session, "user", "two").unwrap();
    // No notify after this turn; the rollout says it ended.
    w.turn_of("one", "task_complete");
    assert!(pass(&w, &mut c), "the second entry waits for no stop");
    assert_eq!(c.report().missed_stops, 1);
}

#[test]
fn an_urgent_entry_sends_the_interrupt_key_and_goes_out_after_the_abort() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    // The user starts a long turn.
    w.append(&[row(
        "event_msg",
        json!({"type": "user_message", "message": "long"}),
    )]);
    w.screen(BUSY);
    let e = w
        .store
        .queue_entry(&w.session, "user", "stop now", true)
        .unwrap();
    for _ in 0..3 {
        assert!(!pass(&w, &mut c));
    }
    assert_eq!(w.typed(), vec![b"\x1b".to_vec()], "one interrupt key");
    assert_eq!(c.report().interrupts, 1);
    // Codex ends the turn as aborted, and the input row comes back.
    w.append(&[row(
        "event_msg",
        json!({"type": "turn_aborted", "reason": "interrupted"}),
    )]);
    w.screen(IDLE);
    assert!(pass(&w, &mut c));
    let d = w.store.deliveries(&w.session).unwrap();
    assert_eq!(d[0].entry.id, e.id);
    assert!(d[0].entry.interrupt);
}

#[test]
fn a_typed_entry_that_starts_no_turn_goes_out_once_more_then_stays_not_landed() {
    let mut t = timing();
    t.land_wait = Duration::ZERO;
    let (w, mut c) = world(t, true);
    w.first_turn();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    let e = w.store.queue_append(&w.session, "user", "lost").unwrap();
    assert!(pass(&w, &mut c));
    // Nothing comes: the entry goes back into the inbox, and out again
    // in the same pass, as the TUI is idle.
    assert!(pass(&w, &mut c), "the retry goes out");
    assert_eq!(c.report().retried, 1);
    assert!(!pass(&w, &mut c), "no third time");
    assert_eq!(c.report().not_landed, vec![e.id.clone()]);
    assert_eq!(w.typed().len(), 4, "typed twice");
    let ledger = w.ledger.lock().unwrap().clone();
    assert!(
        ledger.contains(&format!("pty.retry {}", e.id)),
        "{ledger:?}"
    );
    assert!(ledger.contains(&format!("pty.lost {}", e.id)), "{ledger:?}");
}

#[test]
fn without_the_relay_nothing_is_typed_and_the_entry_stays_queued() {
    let (w, mut c) = world(timing(), false);
    w.first_turn();
    w.stop(Some(THREAD));
    w.store.queue_append(&w.session, "user", "queued").unwrap();
    for _ in 0..3 {
        assert!(!pass(&w, &mut c));
    }
    assert_eq!(w.store.inbox(&w.session).unwrap().len(), 1);
    // The relay ends while an entry would go out: it stays queued.
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    w.store.queue_append(&w.session, "user", "queued").unwrap();
    assert!(c.safe(None));
    crate::relay::Tap::finish(&*w.tap);
    let e = w.store.inbox(&w.session).unwrap().remove(0);
    c.deliver(Input::Entry(e)).unwrap();
    assert_eq!(w.store.inbox(&w.session).unwrap().len(), 1);
    assert!(w.store.deliveries(&w.session).unwrap().is_empty());
    assert!(!c.safe(None));
}

#[test]
fn the_channel_server_gets_no_entry_and_a_swap_is_refused() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    pass(&w, &mut c);
    assert!(w.runs.state(&w.key).typed);
    let e = w.store.queue_append(&w.session, "user", "x").unwrap();
    let mut st = w.runs.state(&w.key);
    assert!(crate::horse::live::tui_state::release_pending(&mut st, vec![e]).is_empty());
    let head = w.store.session(&w.session).unwrap().head.unwrap();
    let opts = crate::horse::codex::swap::LiveOpts {
        interrupt: false,
        wait: Duration::from_millis(10),
        tick: Duration::from_millis(1),
    };
    let r = crate::horse::codex::swap::swap_live(&w.store, &w.session, &head, &mut c, &opts);
    match r {
        Err(crate::horse::codex::swap::Failure::Refused(e)) => {
            assert!(format!("{e:#}").contains("TUI"), "{e:#}")
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn an_idle_screen_with_no_stop_ever_says_so_once_and_keeps_the_entries() {
    let (w, mut c) = world(
        Timing {
            no_stop: Duration::ZERO,
            ..timing()
        },
        true,
    );
    w.first_turn();
    w.screen(IDLE);
    // No entry waits: nothing to say.
    for _ in 0..3 {
        assert!(!pass(&w, &mut c));
    }
    assert!(w.ledger.lock().unwrap().is_empty());
    w.store.queue_append(&w.session, "user", "queued").unwrap();
    for _ in 0..3 {
        assert!(!pass(&w, &mut c));
    }
    assert_eq!(*w.ledger.lock().unwrap(), vec!["pty.no_stop".to_string()]);
    assert!(w.typed().is_empty());
    assert_eq!(w.store.inbox(&w.session).unwrap().len(), 1);
    // The stop comes late: the entry goes out.
    w.stop(Some(THREAD));
    assert!(pass(&w, &mut c));
}

#[test]
fn unknown_nested_content_blocks_typing_and_keeps_the_entry() {
    let (w, mut c) = world(timing(), true);
    w.first_turn();
    w.append(&[row(
        "response_item",
        json!({"type":"message","role":"user","content":[{"type":"future_content"}]}),
    )]);
    let entry = w.store.queue_append(&w.session, "user", "queued").unwrap();
    w.screen(IDLE);
    w.stop(Some(THREAD));
    c.pump(Duration::from_millis(1)).unwrap();
    assert!(c.safe(Some(&w.session)));
    let error = c.deliver(Input::Entry(entry.clone())).unwrap_err();
    assert!(format!("{error:#}").contains("line 6"));
    assert!(w.typed().is_empty());
    assert!(w
        .store
        .inbox(&w.session)
        .unwrap()
        .iter()
        .any(|e| e.id == entry.id));
}
