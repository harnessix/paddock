//! A run in horse mode does not end while a hitched child of its main
//! session owes its result: the driver over the dsh custody and its fake,
//! with the end of the user's input at the start (a run with no input).
//! The child starts before the run (it exists at the first safe point), or
//! while the harness is busy before it reads its first input after the
//! handshake, as the fake dsh of the walk in sandboxes does it.

use super::*;
use crate::horse::hitch::{owed::owed, wire};
use crate::horse::live::{custody, fake as dsh_fake, rpc};
use crate::horse::{dsh, run};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

const TICK: Duration = Duration::from_millis(10);
const RUN: &str = "sbx-hitch";
const MAIN: &str = "session-5eab0000-3333-4333-8444-555555555555";

struct Rig {
    _t: tempfile::TempDir,
    st: Store,
    parent: run::RunHorse,
    child: Arc<Mutex<Option<wire::Hitched>>>,
    /// The first line that paddock sent after the handshake.
    first: Arc<Mutex<Option<String>>>,
    seen: Arc<Mutex<dsh_fake::Seen>>,
    ctl: Sender<Control>,
    join: Option<std::thread::JoinHandle<Result<End>>>,
}

/// The input of the fake dsh. Before the fake reads the first line after
/// the handshake, `hitch` runs once with that line: the harness was busy
/// (in `paddock-host spawn --hitch`) while paddock wrote it.
struct Late<R> {
    r: std::io::BufReader<R>,
    lines: usize,
    buf: Vec<u8>,
    pos: usize,
    hitch: Option<Box<dyn FnOnce(String) + Send>>,
}

impl<R: Read> Read for Late<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
            if self.r.read_until(b'\n', &mut self.buf)? == 0 {
                return Ok(0);
            }
            self.lines += 1;
            if self.lines == 2 {
                if let Some(h) = self.hitch.take() {
                    h(String::from_utf8_lossy(&self.buf).into_owned());
                }
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A dsh run with one hitched child, its custody driven in a thread. The
/// end of the user's input comes `closes` times before the first pass.
/// With `late`, the child starts while the harness is busy after the
/// handshake, else before the run.
fn rig(closes: usize, late: bool) -> Rig {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let parent = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    st.lock()
        .unwrap()
        .bind_native(&parent.session, MAIN)
        .unwrap();
    let child = Arc::new(Mutex::new(None));
    let first = Arc::new(Mutex::new(None));
    if !late {
        *child.lock().unwrap() = Some(wire::begin(&st, &parent, "claude").unwrap());
    }
    let hitch: Box<dyn FnOnce(String) + Send> = {
        let (st, parent, child, first) = (st.clone(), parent.clone(), child.clone(), first.clone());
        Box::new(move |line| {
            if late {
                *child.lock().unwrap() = Some(wire::begin(&st, &parent, "claude").unwrap());
            }
            *first.lock().unwrap() = Some(line);
        })
    };
    let root = t.path().join("sessions");
    let seen = Arc::new(Mutex::new(dsh_fake::Seen::default()));
    let set = custody::Setup {
        store: st.clone(),
        horse: parent.horse.clone(),
        main: MAIN.into(),
        resume: false,
        sessions_root: Some(root.clone()),
        cwd: dsh_fake::CWD.into(),
        provider: crate::horse::live::PROVIDER.into(),
        model: crate::horse::live::MODEL.into(),
        effort: None,
        run: RUN.into(),
        tick: TICK,
    };
    let f = dsh_fake::Fake {
        root: Some(root),
        existing: vec![],
        step: Duration::from_millis(5),
        seen: seen.clone(),
    };
    let (ctl, control) = channel();
    for _ in 0..closes {
        ctl.send(Control::Close).unwrap();
    }
    let (st2, horse) = (st.clone(), parent.horse.clone());
    let join = std::thread::spawn(move || {
        let (pad, end) = UnixStream::pair()?;
        let input = end.try_clone()?;
        let input = Late {
            r: std::io::BufReader::new(input),
            lines: 0,
            buf: Vec::new(),
            pos: 0,
            hitch: Some(hitch),
        };
        std::thread::spawn(move || f.serve(input, end));
        let (tx, rx) = channel();
        rpc::read_runtime(pad.try_clone()?, tx);
        let mut c = custody::Custodian::start(set, pad, rx, std::io::sink())?;
        Driver::new(st2, &horse, RUN, TICK).drive(&mut c, &control)
    });
    Rig {
        _t: t,
        st,
        parent,
        child,
        first,
        seen,
        ctl,
        join: Some(join),
    }
}

impl Rig {
    fn prompts(&self) -> Vec<String> {
        let s = self.seen.lock().unwrap();
        s.prompts.iter().map(|p| p.1.clone()).collect()
    }

    fn child(&self) -> wire::Hitched {
        let c = self.child.lock().unwrap().clone();
        c.unwrap_or_else(|| panic!("the child did not start\n{}", self.report()))
    }

    /// What a failed check shows.
    fn report(&self) -> String {
        format!(
            "first line after the handshake {:?}\nprompts {:?}\ninbox {:?}\nowed {:?}",
            self.first.lock().unwrap(),
            self.prompts(),
            self.st.inbox(&self.parent.session).unwrap(),
            owed(&self.st, &self.parent.horse, &self.parent.session).unwrap()
        )
    }

    fn wait_for(&self, what: &str, mut f: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !f() {
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}\n{}",
                self.report()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn end(&mut self) -> End {
        self.join.take().unwrap().join().unwrap().unwrap()
    }
}

/// The run holds its end until the hitched child reported, and the DNA
/// entry goes out first.
fn holds_until_the_child_reported(mut r: Rig) {
    // The DNA entry of the new session goes out at the first safe point,
    // although the input ended before it.
    r.wait_for("the DNA entry", || {
        r.prompts()
            .first()
            .is_some_and(|p| p.contains("DNA of this horse"))
    });
    let child = r.child();
    std::thread::sleep(TICK * 20);
    assert!(
        !r.join.as_ref().unwrap().is_finished(),
        "the run ended before its hitched child reported\n{}",
        r.report()
    );
    assert_eq!(r.prompts().len(), 1, "{}", r.report());
    wire::finish_once(&r.st, &child, "done: all written").unwrap();
    assert_eq!(r.end(), End::Closed, "{}", r.report());
    let p = r.prompts();
    assert_eq!(p.len(), 2, "{}", r.report());
    assert!(
        p[1].contains("finished\ndone: all written"),
        "{}",
        r.report()
    );
    assert!(r.st.inbox(&r.parent.session).unwrap().is_empty());
    let d = r.st.deliveries(&r.parent.session).unwrap();
    assert!(d.iter().any(|d| d.entry.text.contains("all written")));
    assert!(owed(&r.st, &r.parent.horse, &r.parent.session)
        .unwrap()
        .is_empty());
}

#[test]
fn a_run_with_no_input_ends_only_after_its_hitched_child_reported() {
    holds_until_the_child_reported(rig(1, false));
}

#[test]
fn a_child_hitched_before_the_first_idle_also_holds_the_end() {
    let r = rig(1, true);
    r.wait_for("the first line after the handshake", || {
        r.first.lock().unwrap().is_some()
    });
    let first = r.first.lock().unwrap().clone().unwrap();
    assert!(
        first.contains("session/prompt") && first.contains("DNA of this horse"),
        "the queued DNA entry goes out before the end\n{}",
        r.report()
    );
    holds_until_the_child_reported(r);
}

#[test]
fn a_second_end_of_input_does_not_wait_for_the_hitched_child() {
    let mut r = rig(2, false);
    assert_eq!(r.end(), End::Closed, "{}", r.report());
    assert_eq!(
        owed(&r.st, &r.parent.horse, &r.parent.session).unwrap(),
        [r.child().session]
    );
    drop(r.ctl);
}
