//! The same-id branch swap, on disk and under a live thread held over the
//! fake app-server of live/codex/fake.rs. No codex, no vendor.

use super::request::{self, State};
use super::*;
use crate::horse::codex::{self as codec, ORIGIN, VERSION};
use crate::horse::live::codex::fake::{self, Fake, Seen, NEW_THREAD};
use crate::horse::live::codex::wire::{self, In};
use crate::horse::live::codex::{run, Codex, CustodyCtx, Policy};
use crate::horse::live::harness::{ChannelIo, Control, Ends, Input, Reloaded};
use crate::horse::{run as runs, Binding};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

const OTHER: &str = "019a5f00-0000-7000-8000-0000000000bb";

struct World {
    _t: tempfile::TempDir,
    st: Store,
    horse: String,
    root: PathBuf,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap().id;
    let root = t.path().join("home/.codex/sessions");
    std::fs::create_dir_all(&root).unwrap();
    World {
        _t: t,
        st,
        horse,
        root,
    }
}

impl World {
    fn fresh(&self) -> String {
        let b = Binding {
            harness: codec::HARNESS.into(),
            native_id: None,
            version: VERSION.into(),
            container: codec::CONTAINER.into(),
        };
        self.st.new_session(&self.horse, b).unwrap().id
    }

    fn home(&self) -> PathBuf {
        self.root.parent().unwrap().to_path_buf()
    }

    fn ctx(&self, session: &str) -> CustodyCtx {
        CustodyCtx {
            store: self.st.clone(),
            horse: self.horse.clone(),
            session: session.into(),
            cwd: fake::CWD.into(),
            sessions_root: self.root.clone(),
            run: "sbx-swap".into(),
            model: None,
            effort: None,
            tick: Duration::from_millis(10),
            reply_timeout: Duration::from_secs(10),
            close_timeout: Duration::from_secs(5),
            out: Box::new(std::io::sink()),
        }
    }

    fn fake(&self, seen: &Arc<Mutex<Seen>>, step: u64) -> Fake {
        Fake {
            root: self.root.clone(),
            resume_as: None,
            approval: wire::APPROVE_COMMAND,
            step: Duration::from_millis(step),
            unsubscribe: true,
            persist_forks: false,
            raw_items: true,
            server_version: crate::horse::codex::VERSION,
            seen: seen.clone(),
        }
    }

    fn rollout(&self) -> Vec<u8> {
        std::fs::read(fake::rollout_path(&self.root, NEW_THREAD)).unwrap()
    }
}

fn bypass() -> Policy {
    Policy {
        approval: "never".into(),
        sandbox: "danger-full-access".into(),
    }
}

/// A custody held in this thread, so the test calls `swap_live` itself.
fn custody(w: &World, sid: &str, fk: Fake) -> Codex<UnixStream> {
    let (pad_w, fake_r) = UnixStream::pair().unwrap();
    let (fake_w, pad_r) = UnixStream::pair().unwrap();
    std::thread::spawn(move || fk.serve(fake_r, fake_w));
    let (tx, rx) = std::sync::mpsc::channel::<In>();
    wire::read_server(pad_r, tx);
    Codex::start(pad_w, rx, w.ctx(sid), bypass()).unwrap()
}

/// Run one user turn to its end.
fn turn(w: &World, c: &mut Codex<UnixStream>, sid: &str, text: &str) {
    let e = w.st.queue_append(sid, "user", text).unwrap();
    c.deliver(Input::Entry(e)).unwrap();
    idle(c, sid);
}

fn idle(c: &mut Codex<UnixStream>, sid: &str) {
    let end = Instant::now() + Duration::from_secs(10);
    // Busy at once: the request of the turn is on the way.
    while !c.safe(Some(sid)) {
        assert!(Instant::now() < end, "the turn did not end");
        c.pump(Duration::from_millis(10)).unwrap();
    }
}

fn opts(interrupt: bool) -> LiveOpts {
    LiveOpts {
        interrupt,
        wait: Duration::from_secs(10),
        tick: Duration::from_millis(10),
    }
}

/// The node of the last `task_complete` row before the head: the end of
/// the turn before the last one.
fn end_of_first_turn(st: &Store, sid: &str) -> Hash {
    let nodes = st.session_nodes(&st.session(sid).unwrap()).unwrap();
    let ends: Vec<&Node> = nodes
        .iter()
        .filter(|n| String::from_utf8_lossy(&n.raw).contains("\"task_complete\""))
        .collect();
    assert_eq!(ends.len(), 2, "two turns ran");
    ends[0].hash.clone()
}

fn file_of(st: &Store, head: &Hash) -> Vec<u8> {
    let mut b = Vec::new();
    for n in st.chain(Some(head)).unwrap() {
        b.extend_from_slice(&n.raw);
        b.push(b'\n');
    }
    b
}

fn kinds(st: &Store, horse: &str) -> Vec<String> {
    runs::events(st, horse)
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A [`Live`] that checks, when the file is written, that the verdict of
/// the swap is saved already.
struct Spy<'a> {
    c: &'a mut Codex<UnixStream>,
    st: Store,
    verdict_first: bool,
}

impl Live for Spy<'_> {
    fn pump(&mut self, t: Duration) -> Result<Pump> {
        self.c.pump(t)
    }
    fn main_session(&self) -> Option<String> {
        self.c.main_session()
    }
    fn sessions(&self) -> Vec<crate::horse::live::LiveSession> {
        self.c.sessions()
    }
    fn safe(&self, s: Option<&str>) -> bool {
        self.c.safe(s)
    }
    fn deliver(&mut self, i: Input) -> Result<()> {
        self.c.deliver(i)
    }
    fn interrupt(&mut self) -> Result<bool> {
        self.c.interrupt()
    }
    fn close_input(&mut self) -> Result<()> {
        self.c.close_input()
    }
    fn finish(&mut self, e: crate::horse::live::harness::End) -> Result<()> {
        self.c.finish(e)
    }
    fn reloads(&self) -> bool {
        self.c.reloads()
    }
    fn reload(
        &mut self,
        session: &str,
        write: &mut dyn FnMut(&Path) -> Result<()>,
    ) -> Result<Reloaded> {
        let st = self.st.clone();
        let session_id = session.to_string();
        let flag = &mut self.verdict_first;
        self.c.reload(session, &mut |root: &Path| {
            let last = st.guard_state(&session_id)?.last;
            *flag = last.is_some_and(|v| v.op.starts_with("a swap to"));
            write(root)
        })
    }
}

#[test]
fn a_swap_waits_for_the_turn_and_reloads_the_same_thread_from_the_file() {
    let w = world();
    let sid = w.fresh();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut c = custody(&w, &sid, w.fake(&seen, 40));
    turn(&w, &mut c, &sid, "first");
    turn(&w, &mut c, &sid, "second");
    let old = w.st.session(&sid).unwrap().head.unwrap();
    let to = end_of_first_turn(&w.st, &sid);
    // A third turn runs when the swap comes.
    let e = w.st.queue_append(&sid, "user", "third").unwrap();
    c.deliver(Input::Entry(e)).unwrap();
    assert!(!c.safe(Some(&sid)));
    let mut spy = Spy {
        c: &mut c,
        st: w.st.clone(),
        verdict_first: false,
    };
    let rep = swap_live(&w.st, &sid, &to, &mut spy, &opts(false)).unwrap();
    assert!(
        spy.verdict_first,
        "the verdict was not saved before the write"
    );
    // The swap came only after the third turn ended.
    {
        let s = seen.lock().unwrap();
        let unsub = s
            .requests
            .iter()
            .position(|r| r.0 == wire::THREAD_UNSUBSCRIBE)
            .expect("no thread/unsubscribe");
        assert!(!s.requests[unsub].2, "thread/unsubscribe during a turn");
        assert_eq!(s.requests[unsub + 1].0, wire::THREAD_RESUME);
        // The resume read the swapped file, not the copy in memory.
        assert_eq!(s.attached, 0);
        assert_eq!(s.resumed.last().unwrap(), &file_of(&w.st, &to));
    }
    assert_eq!(rep.thread, NEW_THREAD);
    assert_eq!(rep.file.thread, NEW_THREAD);
    assert!(rep.verdict.keeps, "{}", rep.verdict.message());
    assert_eq!(w.rollout(), file_of(&w.st, &to));
    // The head is the branch head; the old head (after the third turn) is
    // kept in a session of its own.
    let s = w.st.session(&sid).unwrap();
    assert_eq!(s.head.as_ref(), Some(&to));
    assert_eq!(s.binding.native_id.as_deref(), Some(NEW_THREAD));
    let kept = w.st.session(rep.file.kept.as_ref().unwrap()).unwrap();
    assert_ne!(
        kept.head.as_ref(),
        Some(&old),
        "the third turn is in the old branch"
    );
    assert!(w
        .st
        .chain(kept.head.as_ref())
        .unwrap()
        .iter()
        .any(|n| n.hash == old));
    assert_eq!(kept.binding.native_id, None);
    // One event, with the thread.
    let ev = runs::events(&w.st, &w.horse).unwrap();
    let sw: Vec<_> = ev.iter().filter(|e| e["type"] == "swap.live").collect();
    assert_eq!(sw.len(), 1);
    assert_eq!(sw[0]["session"], sid.as_str());
    assert_eq!(sw[0]["to"], to.as_str());
    assert_eq!(sw[0]["thread"], NEW_THREAD);
    assert!(sw[0]["from"].is_string());
    // The custody goes on: the next turn lands on the branch.
    turn(&w, &mut c, &sid, "fourth");
    let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
    assert!(nodes.iter().any(|n| n.hash == to));
    assert!(String::from_utf8_lossy(&nodes.last().unwrap().raw).contains("task_complete"));
    assert!(nodes
        .iter()
        .any(|n| String::from_utf8_lossy(&n.raw).contains("echo: fourth")));
    assert!(!nodes
        .iter()
        .any(|n| String::from_utf8_lossy(&n.raw).contains("echo: second")));
    assert_eq!(seen.lock().unwrap().steers, 0);
}

#[test]
fn interrupt_stops_the_running_turn_before_the_swap() {
    let w = world();
    let sid = w.fresh();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut c = custody(&w, &sid, w.fake(&seen, 20));
    turn(&w, &mut c, &sid, "first");
    turn(&w, &mut c, &sid, "second");
    let to = end_of_first_turn(&w.st, &sid);
    let e = w.st.queue_append(&sid, "user", "[long] work").unwrap();
    c.deliver(Input::Entry(e)).unwrap();
    // Without --interrupt, a turn that does not end refuses the swap.
    let short = LiveOpts {
        wait: Duration::from_millis(200),
        ..opts(false)
    };
    let before = w.rollout();
    match swap_live(&w.st, &sid, &to, &mut c, &short) {
        Err(Failure::Refused(e)) => assert!(format!("{e:#}").contains("did not end"), "{e:#}"),
        other => panic!("{other:?}"),
    }
    assert!(w.rollout().starts_with(&before));
    let rep = swap_live(&w.st, &sid, &to, &mut c, &opts(true)).unwrap();
    let s = seen.lock().unwrap();
    let methods = s.methods();
    let int = methods
        .iter()
        .position(|m| m == wire::TURN_INTERRUPT)
        .unwrap();
    let unsub = methods
        .iter()
        .position(|m| m == wire::THREAD_UNSUBSCRIBE)
        .unwrap();
    assert!(int < unsub);
    assert!(!s.requests[unsub].2);
    assert_eq!(rep.thread, NEW_THREAD);
    assert_eq!(w.rollout(), file_of(&w.st, &to));
}

#[test]
fn another_thread_after_the_reload_ends_the_custody_with_the_file_swapped() {
    let w = world();
    let sid = w.fresh();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut fk = w.fake(&seen, 20);
    // The first resume is the reload: the custody started a new thread.
    fk.resume_as = Some(OTHER.into());
    let mut c = custody(&w, &sid, fk);
    turn(&w, &mut c, &sid, "first");
    turn(&w, &mut c, &sid, "second");
    let to = end_of_first_turn(&w.st, &sid);
    match swap_live(&w.st, &sid, &to, &mut c, &opts(false)) {
        Err(Failure::Custody(e)) => {
            let m = format!("{e:#}");
            assert!(m.contains(OTHER), "{m}");
            assert!(m.contains("next run resumes from the file"), "{m}");
        }
        other => panic!("{other:?}"),
    }
    // The file and the store hold the branch, whole.
    assert_eq!(w.rollout(), file_of(&w.st, &to));
    assert_eq!(w.st.session(&sid).unwrap().head.as_ref(), Some(&to));
    assert!(!kinds(&w.st, &w.horse).contains(&"swap.live".to_string()));
    // The next run resumes from the file: a fresh ingest keeps it.
    let p = fake::rollout_path(&w.root, NEW_THREAD);
    let l = w.st.lock().unwrap();
    let i = codec::ingest_run_locked(&l, &p, VERSION, &w.horse).unwrap();
    assert_eq!((i.session, i.added), (sid, 0));
}

#[test]
fn a_server_without_unsubscribe_refuses_the_swap_and_keeps_the_custody() {
    let w = world();
    let sid = w.fresh();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut fk = w.fake(&seen, 20);
    fk.unsubscribe = false;
    let mut c = custody(&w, &sid, fk);
    turn(&w, &mut c, &sid, "first");
    turn(&w, &mut c, &sid, "second");
    let head = w.st.session(&sid).unwrap().head;
    let before = w.rollout();
    let to = end_of_first_turn(&w.st, &sid);
    match swap_live(&w.st, &sid, &to, &mut c, &opts(false)) {
        Err(Failure::Refused(e)) => {
            assert!(format!("{e:#}").contains("cannot drop the thread"), "{e:#}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(w.rollout(), before);
    assert_eq!(w.st.session(&sid).unwrap().head, head);
    assert_eq!(seen.lock().unwrap().resumed.len(), 0);
    turn(&w, &mut c, &sid, "third");
    assert!(String::from_utf8_lossy(&w.rollout()).contains("echo: third"));
}

#[test]
fn a_request_from_another_process_is_served_by_the_driver_at_idle() {
    let w = world();
    let sid = w.fresh();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fk = w.fake(&seen, 30);
    let (pad_w, fake_r) = UnixStream::pair().unwrap();
    let (fake_w, pad_r) = UnixStream::pair().unwrap();
    std::thread::spawn(move || fk.serve(fake_r, fake_w));
    let (ctl, control) = std::sync::mpsc::channel();
    let io = ChannelIo {
        from: pad_r,
        to: pad_w,
        control,
    };
    let ctx = w.ctx(&sid);
    let join = std::thread::spawn(move || run(io, ctx, bypass(), Ends::ByRun));
    for t in ["first", "second"] {
        w.st.queue_append(&sid, "user", t).unwrap();
    }
    let ends = || {
        String::from_utf8_lossy(&w.rollout())
            .matches("task_complete")
            .count()
    };
    let end = Instant::now() + Duration::from_secs(10);
    while !fake::rollout_path(&w.root, NEW_THREAD).exists() || ends() < 2 {
        assert!(Instant::now() < end, "two turns did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The custody ingests after `turn/completed`: wait for the head.
    let to = loop {
        let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
        let n = nodes
            .iter()
            .filter(|n| String::from_utf8_lossy(&n.raw).contains("task_complete"))
            .count();
        if n == 2 {
            break end_of_first_turn(&w.st, &sid);
        }
        assert!(Instant::now() < end, "the second turn was not ingested");
        std::thread::sleep(Duration::from_millis(5));
    };
    let r = request::ask(&w.st, &sid, &to, false, Duration::from_secs(10)).unwrap();
    // A second request waits for the first one.
    assert!(request::ask(&w.st, &sid, &to, false, Duration::from_secs(1)).is_err());
    let answer = loop {
        if let Some(a) = request::answer(&w.st, &sid, &r.id).unwrap() {
            break a;
        }
        assert!(Instant::now() < end, "no answer");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(answer.state, State::Done, "{answer:?}");
    assert_eq!(answer.thread.as_deref(), Some(NEW_THREAD));
    assert!(answer.verdict.is_some());
    request::remove(&w.st, &sid, &r.id).unwrap();
    assert!(request::read(&w.st, &sid).unwrap().is_none());
    assert_eq!(w.st.session(&sid).unwrap().head.as_ref(), Some(&to));
    ctl.send(Control::Close).unwrap();
    join.join().unwrap().unwrap();
    assert!(kinds(&w.st, &w.horse).contains(&"swap.live".to_string()));
}

#[test]
fn a_request_of_a_process_that_is_gone_is_dropped() {
    let w = world();
    let sid = w.fresh();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut c = custody(&w, &sid, w.fake(&seen, 20));
    turn(&w, &mut c, &sid, "first");
    turn(&w, &mut c, &sid, "second");
    let head = w.st.session(&sid).unwrap().head;
    let to = end_of_first_turn(&w.st, &sid);
    let mut r = request::ask(&w.st, &sid, &to, false, Duration::from_secs(1)).unwrap();
    // A pid that no process has.
    r.pid = i32::MAX as u32;
    w.st.write_ref(
        &format!("swap-requests/{sid}.json"),
        &serde_json::to_vec(&r).unwrap(),
    )
    .unwrap();
    request::serve(&w.st, &mut c, Duration::from_millis(10)).unwrap();
    assert!(request::read(&w.st, &sid).unwrap().is_none());
    assert_eq!(w.st.session(&sid).unwrap().head, head);
}

/// A stopped session: a projected rollout and a branch of its own.
fn stopped(w: &World) -> (String, Hash, Hash) {
    let rows = [
        fake::meta(NEW_THREAD),
        fake::row(
            "response_item",
            fake::message("user", "input_text", "question"),
        ),
        fake::row(
            "event_msg",
            serde_json::json!({ "type": "task_complete", "turn_id": "t1" }),
        ),
        fake::row(
            "response_item",
            fake::message("user", "input_text", "the old way"),
        ),
    ];
    let p = fake::write_rollout(&w.root, NEW_THREAD, &rows);
    let sid = codec::ingest(&w.st, &p, VERSION, Some(&w.horse))
        .unwrap()
        .session;
    let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
    let fork = nodes[2].hash.clone();
    let other = fake::row(
        "response_item",
        fake::message("user", "input_text", "the other way"),
    );
    let to =
        w.st.put_native(&ORIGIN, Some(&fork), "row:3", &other)
            .unwrap();
    (sid, nodes[3].hash.clone(), to)
}

#[test]
fn a_stopped_session_swaps_on_disk_and_keeps_both_branches() {
    let w = world();
    let (sid, old, to) = stopped(&w);
    let v = judge(&w.st, &sid, &to).unwrap();
    assert!(!v.keeps);
    assert!(v.lost_tokens > 0);
    let f = swap_file(&w.st, &sid, &to, &w.home()).unwrap();
    assert_eq!(f.thread, NEW_THREAD);
    assert_eq!(f.from.as_ref(), Some(&old));
    assert_eq!(w.rollout(), file_of(&w.st, &to));
    // The old file stays next to it.
    assert_eq!(
        std::fs::read(f.backup.as_ref().unwrap()).unwrap(),
        file_of(&w.st, &old)
    );
    let kept = f.kept.clone().unwrap();
    assert_eq!(w.st.session(&kept).unwrap().head.as_ref(), Some(&old));
    // Back again: the branch left behind gets a session of its own. Once
    // more: the old head is the head of the first kept session already.
    let back = swap_file(&w.st, &sid, &old, &w.home()).unwrap();
    assert!(back.kept.is_some());
    let again = swap_file(&w.st, &sid, &to, &w.home()).unwrap();
    assert_eq!(again.kept, None);
    let horse = w.st.horse(&w.horse).unwrap();
    assert_eq!(horse.sessions.len(), 3, "{:?}", horse.sessions);
    // The swapped file ingests into the same session without a change.
    let i = codec::ingest(
        &w.st,
        &fake::rollout_path(&w.root, NEW_THREAD),
        VERSION,
        None,
    )
    .unwrap();
    assert_eq!((i.session, i.added), (sid.clone(), 0));
    assert!(swap_file(&w.st, &sid, &to, &w.home())
        .unwrap_err()
        .to_string()
        .contains("already"));
}

#[test]
fn a_writer_of_the_thread_blocks_the_swap_on_disk() {
    let w = world();
    let (sid, _, to) = stopped(&w);
    let before = w.rollout();
    let locks = w.home().join("thread-writer-locks");
    std::fs::create_dir_all(&locks).unwrap();
    let lock = std::fs::File::create(locks.join(format!("{NEW_THREAD}.lock"))).unwrap();
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock on an fd that `lock` owns.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let head = w.st.session(&sid).unwrap().head;
    let e = swap_file(&w.st, &sid, &to, &w.home()).unwrap_err();
    assert!(e.to_string().contains("locked by a writer"), "{e:#}");
    assert_eq!(w.rollout(), before);
    assert_eq!(w.st.session(&sid).unwrap().head, head);
    assert!(w.st.read_ref(&journal_ref(&sid)).unwrap().is_none());
}

#[test]
fn a_crash_between_the_rename_and_the_head_move_is_finished_next_time() {
    let w = world();
    let (sid, old, to) = stopped(&w);
    // The file holds the branch, the journal says so, the head did not move.
    let path = fake::rollout_path(&w.root, NEW_THREAD);
    std::fs::write(&path, file_of(&w.st, &to)).unwrap();
    let j = Journal {
        from: Some(old.clone()),
        to: to.clone(),
        path,
    };
    w.st.write_ref(&journal_ref(&sid), &serde_json::to_vec(&j).unwrap())
        .unwrap();
    // The next swap (back to the old head) first finishes the journaled one.
    let f = swap_file(&w.st, &sid, &old, &w.home()).unwrap();
    assert_eq!(f.from.as_ref(), Some(&to));
    assert_eq!(w.rollout(), file_of(&w.st, &old));
    assert!(w.st.read_ref(&journal_ref(&sid)).unwrap().is_none());
    // A journal of a rename that never happened is dropped.
    w.st.write_ref(
        &journal_ref(&sid),
        &serde_json::to_vec(&Journal {
            from: Some(old.clone()),
            to: to.clone(),
            path: fake::rollout_path(&w.root, NEW_THREAD),
        })
        .unwrap(),
    )
    .unwrap();
    let f = swap_file(&w.st, &sid, &to, &w.home()).unwrap();
    assert_eq!(f.from.as_ref(), Some(&old));
}

#[test]
fn a_branch_of_another_thread_or_harness_is_refused() {
    let w = world();
    let (sid, _, _) = stopped(&w);
    let p = fake::write_rollout(&w.root, OTHER, &[fake::meta(OTHER)]);
    let other = codec::ingest(&w.st, &p, VERSION, Some(&w.horse))
        .unwrap()
        .session;
    let foreign = w.st.session(&other).unwrap().head.unwrap();
    let e = swap_file(&w.st, &sid, &foreign, &w.home()).unwrap_err();
    assert!(e.to_string().contains("does not continue thread"), "{e:#}");
    assert!(judge(&w.st, &sid, &foreign).is_err());
    let blob = w.st.put_blob("text/plain", b"x").unwrap();
    assert!(swap_file(&w.st, &sid, &blob, &w.home()).is_err());
    // A branch head by session id or by hash prefix.
    let s = w.st.session(&sid).unwrap();
    assert_eq!(resolve_branch(&w.st, &s, &other).unwrap(), foreign);
    assert_eq!(
        resolve_branch(&w.st, &s, &foreign.as_str()[..12]).unwrap(),
        foreign
    );
    assert!(resolve_branch(&w.st, &s, "000000000000").is_err());
}
