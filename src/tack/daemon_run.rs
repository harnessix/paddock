//! tackd with a started program: the start, and the time until the
//! program ends.

use super::*;

impl Server {
    /// Start an allowed program and answer. Returns the running program
    /// for `supervise`, or None when nothing runs.
    pub(super) fn start(
        &self,
        conn: &UnixStream,
        peer: Peer,
        req: &Start,
        d: Decision,
        cfg: Option<Tack>,
        fds: Vec<OwnedFd>,
    ) -> io::Result<Option<Box<dyn Program>>> {
        let fail = |msg: String| -> io::Result<Option<Box<dyn Program>>> {
            self.counts.errors.fetch_add(1, Ordering::Relaxed);
            self.audit_start(peer, &req.program, &d, "failed", msg.clone());
            let mut w = conn;
            proto::write_frame(&mut w, &proto::envelope(Answer::Failed { message: msg }))?;
            Ok(None)
        };
        let Some(starter) = &self.starter else {
            return fail("this tackd decides but starts no program on this system yet".into());
        };
        if fds.len() != req.fds() {
            return fail(format!(
                "the request names {} standard streams and {} came",
                req.fds(),
                fds.len()
            ));
        }
        let Some(cfg) = cfg else {
            return fail("your configuration could not be read again".into());
        };
        {
            let m = self.running.lock().unwrap_or_else(|e| e.into_inner());
            let all: usize = m.values().sum();
            let mine = m.get(&peer.uid).copied().unwrap_or(0);
            if all >= self.max_running || mine >= self.max_running_per_uid {
                drop(m);
                return fail(format!(
                    "too many programs run through tackd ({mine} of yours, {all} in all): try again when some end"
                ));
            }
        }
        match starter.start(peer, req, &d, &cfg, fds) {
            Err(e) => fail(e),
            Ok(p) => {
                self.counts.started.fetch_add(1, Ordering::Relaxed);
                let note = p.note();
                self.audit_start(
                    peer,
                    &req.program,
                    &d,
                    "started",
                    format!("pid {}; {note}", p.pid()),
                );
                let mut w = conn;
                proto::write_frame(
                    &mut w,
                    &proto::envelope(Answer::Started {
                        pid: p.pid(),
                        decision: d,
                        protection: crate::tack::UNPROTECTED.to_string(),
                    }),
                )?;
                Ok(Some(p))
            }
        }
    }

    /// Stay with a started program until it ends: report what happens to
    /// it, pass on the signals of the stand-in. A stand-in that goes away
    /// before the program ends was killed (it never leaves on its own):
    /// the program gets SIGKILL, as the program itself would have.
    pub(super) fn supervise(
        &self,
        conn: UnixStream,
        peer: Peer,
        req: &Start,
        d: &Decision,
        p: Box<dyn Program>,
    ) {
        {
            let mut m = self.running.lock().unwrap_or_else(|e| e.into_inner());
            *m.entry(peer.uid).or_insert(0) += 1;
        }
        let _slot = RunSlot(self, peer.uid);
        let p: Arc<dyn Program> = Arc::from(p);
        let _ = conn.set_read_timeout(None);
        let _ = conn.set_write_timeout(Some(self.deadline));
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = conn.try_clone();
        let watcher = {
            let p = p.clone();
            let ended = ended.clone();
            let conn = conn.try_clone();
            std::thread::Builder::new()
                .name("tackd-wait".into())
                .stack_size(256 * 1024)
                .spawn(move || loop {
                    let e = p.next();
                    let last = matches!(e, Event::Exited { .. } | Event::Killed { .. });
                    if let Ok(c) = &conn {
                        let mut w = c;
                        let _ = proto::write_frame(&mut w, &proto::envelope(e.clone()));
                    }
                    if last {
                        ended.store(true, Ordering::SeqCst);
                        // Wake the reader: the stand-in may not close.
                        if let Ok(c) = &conn {
                            let _ = c.shutdown(std::net::Shutdown::Both);
                        }
                        return e;
                    }
                })
        };
        let watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                eprintln!("tackd: cannot start a thread: {e}; killing pid {}", p.pid());
                let _ = p.signal(libc::SIGKILL, false);
                let _ = p.next();
                return;
            }
        };
        if let Ok(mut r) = reader {
            while let Ok(body) = proto::read_small_frame(&mut r, proto::MAX_EVENT) {
                match proto::parse_after::<Control>(&body) {
                    Ok(Control::Signal { signal, group }) if proto::FORWARDED.contains(&signal) => {
                        if let Err(e) = p.signal(signal, group) {
                            log::debug!("tackd: signal {signal} to {}: {e}", p.pid());
                        }
                    }
                    Ok(Control::Signal { signal, .. }) => {
                        eprintln!(
                            "tackd: uid {} asked to send signal {signal}, which a stand-in does not pass on",
                            peer.uid
                        );
                    }
                    Err(e) => {
                        eprintln!("tackd: a bad message from uid {}: {e}", peer.uid);
                        break;
                    }
                }
            }
        }
        if !ended.load(Ordering::SeqCst) {
            // The stand-in is gone, or broke the protocol.
            let _ = p.signal(libc::SIGKILL, false);
        }
        let last = watcher.join().unwrap_or(Event::Exited { code: -1 });
        let how = match last {
            Event::Exited { code } => format!("pid {} ended with exit code {code}", p.pid()),
            Event::Killed { signal, core } => format!(
                "pid {} ended by {}{}",
                p.pid(),
                proto::signal_name(signal),
                if core { " (core dumped)" } else { "" }
            ),
            _ => format!("pid {} ended", p.pid()),
        };
        self.audit_start(peer, &req.program, d, "ended", how);
    }
}
