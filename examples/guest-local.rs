//! The local Mac check of the guest agent, without a guest VM:
//! the agent runs on this Mac with its real AppKit and Accessibility
//! code, the check plays the host over a local socket pair, and TextEdit
//! is the app. `packaging/macos/checks/macvm-agent.sh` runs it.
//!
//! It prints one `PASS name` or `FAIL name: why` line per check and exits
//! 1 when any check failed. It needs Accessibility for the terminal that
//! runs it, and TextEdit must not run before it starts. It touches only
//! temporary folders, a temporary text file, and the input sources: the
//! German layout is enabled for one check and disabled again when it was
//! not enabled before.

fn main() {
    std::process::exit(mac::run());
}

#[cfg(not(target_os = "macos"))]
mod mac {
    pub fn run() -> i32 {
        eprintln!("guest-local runs only on macOS");
        2
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use paddock::guest::{input, macos::Mac, main_loop, update};
    use paddock::macvm::proto::{
        read_line, write_line, Call, ConfigureStep, ErrorCode, Event, FileHeader, Frame, Outcome,
        Request, VERSION,
    };
    use std::collections::VecDeque;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const TOKEN: &str = "guestchecktok0123456789abcdef012";
    const TEXTEDIT: &str = "/System/Applications/TextEdit.app";
    const GERMAN: &str = "com.apple.keylayout.German";

    struct Host {
        s: UnixStream,
        req: u64,
        seen: Vec<Event>,
    }

    impl Host {
        fn new(s: UnixStream) -> Self {
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            Host {
                s,
                req: 0,
                seen: Vec::new(),
            }
        }

        fn next(&mut self) -> Result<Event, String> {
            let ev: Event = read_line(&mut self.s)
                .map_err(|e| format!("{e:#}"))?
                .ok_or_else(|| "the agent closed the line".to_string())?;
            self.seen.push(ev.clone());
            Ok(ev)
        }

        fn until(
            &mut self,
            limit: Duration,
            pred: impl Fn(&Event) -> bool,
        ) -> Result<Event, String> {
            let start = Instant::now();
            loop {
                if start.elapsed() > limit {
                    return Err(format!("no matching event within {limit:?}"));
                }
                let ev = self.next()?;
                if pred(&ev) {
                    return Ok(ev);
                }
            }
        }

        fn send(&mut self, request: Request) -> u64 {
            self.req += 1;
            let _ = write_line(
                &mut self.s,
                &Call {
                    req: self.req,
                    request,
                },
            );
            self.req
        }

        fn reply(&mut self, req: u64, limit: Duration) -> Result<Outcome, String> {
            match self.until(limit, |e| matches!(e, Event::Reply(r) if r.req == req))? {
                Event::Reply(r) => Ok(r.outcome),
                _ => unreachable!(),
            }
        }

        fn ok(&mut self, req: u64, limit: Duration) -> Result<(), String> {
            match self.reply(req, limit)? {
                Outcome::Ok { .. } => Ok(()),
                Outcome::Error { code, text } => Err(format!("{code:?}: {text}")),
            }
        }

        fn error_code(&mut self, req: u64, limit: Duration) -> Result<ErrorCode, String> {
            match self.reply(req, limit)? {
                Outcome::Ok { .. } => Err("the request succeeded".into()),
                Outcome::Error { code, .. } => Ok(code),
            }
        }

        fn handshake(&mut self) -> Result<(), String> {
            match self.next()? {
                Event::Hello(h) if h.v == VERSION => {}
                other => return Err(format!("expected hello, got {}", other.kind())),
            }
            let req = self.send(Request::Token {
                token: TOKEN.into(),
                v: VERSION,
                bench: false,
            });
            self.ok(req, Duration::from_secs(5))
        }
    }

    struct Report(Vec<bool>);

    impl Report {
        fn check(&mut self, name: &str, r: Result<String, String>) {
            match r {
                Ok(detail) => {
                    println!(
                        "PASS {name}{}",
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!(": {detail}")
                        }
                    );
                    self.0.push(true);
                }
                Err(why) => {
                    println!("FAIL {name}: {why}");
                    self.0.push(false);
                }
            }
        }
    }

    fn sh(cmd: &str) -> (bool, String) {
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", cmd])
            .output();
        match out {
            Ok(o) => (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).trim().to_string(),
            ),
            Err(e) => (false, e.to_string()),
        }
    }

    fn default_bool(key: &str) -> bool {
        sh(&format!(
            "/usr/bin/defaults read com.apple.universalaccess {key} 2>/dev/null"
        ))
        .1 == "1"
    }

    struct Setup {
        support: PathBuf,
        log: PathBuf,
        text_file: PathBuf,
        files: Arc<Mutex<Vec<UnixStream>>>,
    }

    pub fn run() -> i32 {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            println!("FAIL setup: not on the main thread");
            return 1;
        };
        if sh("/usr/bin/pgrep -x TextEdit").0 {
            println!("FAIL setup: TextEdit runs already; quit it, then run the check again");
            return 1;
        }
        let tmp = std::env::temp_dir().join(format!("paddock-guest-{}", std::process::id()));
        let support = tmp.join("support");
        std::fs::create_dir_all(support.join("current")).unwrap();
        std::fs::copy(
            "/usr/bin/true",
            support.join("current").join("paddock-guest"),
        )
        .unwrap();
        std::fs::write(support.join("launch"), "#!/bin/sh\n").unwrap();
        let log = tmp.join("paddock-guest.log");
        let _ = paddock::guest::log::init(log.clone());
        let text_file = tmp.join("guest-check.txt");
        std::fs::write(&text_file, "paddock\n").unwrap();
        std::env::set_var("PADDOCK_GUEST_SUPPORT", &support);
        std::env::set_var("PADDOCK_GUEST_LOG", &log);
        let _ = std::fs::remove_file(std::env::temp_dir().join("paddock-guest-app"));
        let files: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
        let setup = Setup {
            support: support.clone(),
            log,
            text_file,
            files: files.clone(),
        };

        let (a1, b1) = UnixStream::pair().unwrap();
        let (a2, b2) = UnixStream::pair().unwrap();
        let dials = Arc::new(Mutex::new(VecDeque::from(vec![a1, a2])));
        let host = std::thread::spawn(move || host_thread(b1, b2, setup));
        let files_dial = move || {
            let (a, b) = UnixStream::pair()?;
            files.lock().unwrap().push(b);
            Ok(a)
        };
        let mut mac = Mac::with_files(mtm, files_dial);
        let dial = || {
            dials
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| std::io::Error::other("no more"))
        };
        // Session 1 ends with agent.restart, session 2 with agent.update.
        let code1 = main_loop::run(&mut mac, dial);
        let code2 = main_loop::run(&mut mac, dial);
        let mut report = host.join().unwrap_or_else(|_| Report(vec![false]));
        report.check(
            "exit codes",
            if code1 == 0 && code2 == 0 {
                Ok(String::new())
            } else {
                Err(format!("{code1} and {code2}, expected 0 and 0"))
            },
        );
        let current =
            std::fs::read(support.join("current").join("paddock-guest")).unwrap_or_default();
        let prev = std::fs::read(support.join("prev").join("paddock-guest")).unwrap_or_default();
        report.check(
            "update",
            if current == b"#!/bin/sh\nexit 0\n"
                && prev == std::fs::read("/usr/bin/true").unwrap_or_default()
            {
                Ok("current/ is the new file, prev/ the old".into())
            } else {
                Err("current/ or prev/ is not the expected file".into())
            },
        );
        let _ = std::fs::remove_dir_all(&tmp);
        if report.0.iter().all(|p| *p) {
            0
        } else {
            1
        }
    }

    fn host_thread(b1: UnixStream, b2: UnixStream, setup: Setup) -> Report {
        let mut report = Report(Vec::new());
        let mut host = Host::new(b1);
        let t0 = Instant::now();
        report.check(
            "dial",
            host.handshake()
                .map(|_| format!("hello after {} ms", t0.elapsed().as_millis())),
        );
        let full = host.until(Duration::from_secs(5), |e| matches!(e, Event::Windows(_)));
        report.check(
            "empty list before the launch",
            full.and_then(|e| match e {
                Event::Windows(w) if w.order.is_empty() => Ok(String::new()),
                Event::Windows(w) => Err(format!(
                    "{} windows followed before any launch",
                    w.order.len()
                )),
                _ => unreachable!(),
            }),
        );
        let req = host.send(Request::AppLaunch {
            path: TEXTEDIT.into(),
            args: vec![setup.text_file.to_string_lossy().into_owned()],
            env: Default::default(),
        });
        let pid = match host.reply(req, Duration::from_secs(30)) {
            Ok(Outcome::Ok {
                data: Some(paddock::macvm::proto::ReplyData::Launched { pid }),
            }) => pid,
            other => {
                report.check("launch", Err(format!("{other:?}")));
                return report;
            }
        };
        report.check("launch", Ok(format!("TextEdit pid {pid}")));
        let doc = host.until(Duration::from_secs(15), |e| {
            matches!(e, Event::Windows(w) if w.changed.iter().any(|x| x.title == "guest-check.txt"))
        });
        let window = match doc {
            Ok(Event::Windows(w)) => w
                .changed
                .iter()
                .find(|x| x.title == "guest-check.txt")
                .cloned()
                .unwrap(),
            Ok(_) => unreachable!(),
            Err(e) => {
                report.check(
                    "title",
                    Err(format!("no window titled guest-check.txt: {e}")),
                );
                return report;
            }
        };
        report.check(
            "title",
            Ok(format!(
                "window {} of {} titled by kAXTitleAttribute",
                window.id, window.app
            )),
        );
        // move
        let frame = Frame::new(100.0, 100.0, window.frame.w, window.frame.h);
        let req = host.send(Request::WindowMove {
            id: window.id,
            frame,
        });
        let moved = host.ok(req, Duration::from_secs(5)).and_then(|_| {
            let t = Instant::now();
            host.until(Duration::from_secs(2), |e| {
                matches!(e, Event::Windows(w) if w.changed.iter().any(|x| x.id == window.id && x.frame.near(&frame, 2.0)))
            })
            .map(|_| t.elapsed())
        });
        report.check(
            "move",
            moved.and_then(|d| {
                if d <= Duration::from_millis(50) {
                    Ok(format!("event after {} ms", d.as_millis()))
                } else {
                    Err(format!("event after {} ms, limit 50", d.as_millis()))
                }
            }),
        );
        // restart: the list before, a restart, the list after.
        let before: Vec<(u32, Frame)> = match host.seen.iter().rev().find_map(|e| match e {
            Event::Windows(_) => Some(()),
            _ => None,
        }) {
            Some(()) => snapshot(&host.seen),
            None => Vec::new(),
        };
        let req = host.send(Request::AgentRestart);
        let _ = host.ok(req, Duration::from_secs(5));
        drop(host);
        let mut host = Host::new(b2);
        let t = Instant::now();
        let hello = host.handshake();
        let after = hello
            .and_then(|_| host.until(Duration::from_secs(5), |e| matches!(e, Event::Windows(_))))
            .map(|e| match e {
                Event::Windows(w) => w
                    .changed
                    .iter()
                    .map(|x| (x.id, x.frame))
                    .collect::<Vec<_>>(),
                _ => unreachable!(),
            });
        report.check(
            "restart",
            after.and_then(|after| {
                let mut a = after.clone();
                let mut b = before.clone();
                a.sort_by_key(|x| x.0);
                b.sort_by_key(|x| x.0);
                if t.elapsed() > Duration::from_secs(2) {
                    Err(format!("the new hello took {} ms", t.elapsed().as_millis()))
                } else if a.len() == b.len()
                    && a.iter()
                        .zip(&b)
                        .all(|(x, y)| x.0 == y.0 && x.1.near(&y.1, 1.0))
                {
                    Ok(format!(
                        "{} windows, same ids and frames, hello after {} ms",
                        a.len(),
                        t.elapsed().as_millis()
                    ))
                } else {
                    Err(format!("before {b:?}, after {a:?}"))
                }
            }),
        );
        // hang
        let _ = sh(&format!("/bin/kill -STOP {pid}"));
        let req = host.send(Request::WindowMove {
            id: window.id,
            frame,
        });
        let _ping = host.send(Request::Ping { t: 1 });
        let tp = Instant::now();
        let pong = host
            .until(Duration::from_millis(500), |e| {
                matches!(e, Event::Pong { .. })
            })
            .map(|_| tp.elapsed());
        let tm = Instant::now();
        let code = host
            .error_code(req, Duration::from_secs(3))
            .map(|c| (c, tm.elapsed()));
        let _ = sh(&format!("/bin/kill -CONT {pid}"));
        report.check(
            "hang",
            pong.and_then(|p| code.map(|c| (p, c)))
                .and_then(|(p, (c, d))| {
                    if c != ErrorCode::Timeout {
                        Err(format!("window.move gave {c:?}, not Timeout"))
                    } else if d > Duration::from_secs(2) {
                        Err(format!("Timeout after {} ms, limit 2000", d.as_millis()))
                    } else if p > Duration::from_millis(100) {
                        Err(format!("pong after {} ms, limit 100", p.as_millis()))
                    } else {
                        Ok(format!(
                            "Timeout after {} ms, pong after {} ms",
                            d.as_millis(),
                            p.as_millis()
                        ))
                    }
                }),
        );
        // guard
        let req = host.send(Request::Configure {
            password: "x".into(),
            steps: vec![ConfigureStep {
                name: "n".into(),
                as_root: false,
                argv: vec!["/usr/bin/true".into()],
                verify: vec![],
            }],
        });
        let c1 = host.error_code(req, Duration::from_secs(5));
        let req = host.send(Request::DataPrepare {
            password: "x".into(),
            data_id: "d".into(),
        });
        let c2 = host.error_code(req, Duration::from_secs(5));
        report.check(
            "guard",
            match (c1, c2) {
                (Ok(ErrorCode::NotInSession), Ok(ErrorCode::NotInSession)) => {
                    Ok("configure and data.prepare reply NotInSession".into())
                }
                other => Err(format!("{other:?}")),
            },
        );
        // input
        let original = input::current().unwrap_or_default();
        let enabled_before = input::enabled();
        let (rm, ic) = (
            default_bool("reduceMotion"),
            default_bool("increaseContrast"),
        );
        let req = host.send(Request::InputSet {
            sources: vec![original.clone(), GERMAN.into()],
            current: GERMAN.into(),
            reduce_motion: rm,
            increase_contrast: ic,
        });
        let set = host.reply(req, Duration::from_secs(5));
        let req = host.send(Request::AxWatch { on: true });
        let _ = host.ok(req, Duration::from_secs(5));
        let req = host.send(Request::WindowFocus { id: window.id });
        let _ = host.ok(req, Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(500));
        let (typed, _) =
            sh("/usr/bin/osascript -e 'tell application \"System Events\" to key code 6'");
        let value = host.until(
            Duration::from_secs(5),
            |e| matches!(e, Event::AxValue { text, .. } if text.contains('y')),
        );
        let _ = sh("/usr/bin/osascript -e 'tell application \"System Events\" to key code 51'");
        let req = host.send(Request::InputSet {
            sources: vec![original.clone()],
            current: original.clone(),
            reduce_motion: rm,
            increase_contrast: ic,
        });
        let _ = host.ok(req, Duration::from_secs(5));
        if !enabled_before.iter().any(|s| s == GERMAN) {
            input::disable(GERMAN);
        }
        report.check(
            "input",
            match (set, typed, value) {
                (Ok(Outcome::Ok { .. }), true, Ok(_)) => {
                    Ok("key code 6 typed y under the German layout".into())
                }
                (Ok(Outcome::Ok { .. }), false, _) => {
                    Err("osascript could not type: the terminal needs Accessibility".into())
                }
                (set, _, value) => Err(format!("input.set {set:?}, ax.value {value:?}")),
            },
        );
        // log
        let req = host.send(Request::LogFetch);
        let fetched = host.ok(req, Duration::from_secs(10)).and_then(|_| {
            let mut server = setup
                .files
                .lock()
                .unwrap()
                .pop()
                .ok_or("the agent made no guest-files connection")?;
            server
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let header: FileHeader = read_line(&mut server)
                .map_err(|e| format!("{e:#}"))?
                .ok_or("no header")?;
            let mut body = Vec::new();
            server.read_to_end(&mut body).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&body);
            if header.token != TOKEN {
                return Err("the header carries another token".into());
            }
            if text.contains(TOKEN) {
                return Err("the log holds the token".into());
            }
            if body.len() as u64 != header.size {
                return Err(format!("{} bytes, header says {}", body.len(), header.size));
            }
            Ok(format!(
                "{} bytes of {}, no token in it",
                body.len(),
                setup.log.display()
            ))
        });
        report.check("log", fetched);
        // quit
        let req = host.send(Request::AppQuit {
            app: "com.apple.TextEdit".into(),
            force: false,
        });
        let quit = host.ok(req, Duration::from_secs(15)).or_else(|_| {
            let req = host.send(Request::AppQuit {
                app: "com.apple.TextEdit".into(),
                force: true,
            });
            host.ok(req, Duration::from_secs(10))
        });
        let exited = host.until(Duration::from_secs(10), |e| {
            matches!(e, Event::AppExited { .. })
        });
        report.check("quit", quit.and_then(|_| exited.map(|e| e.kind())));
        // update
        let new = setup.support.join("new-agent");
        std::fs::write(&new, "#!/bin/sh\nexit 0\n").unwrap();
        let sha = update::sha256_file(&new).unwrap();
        let req = host.send(Request::AgentUpdate {
            path: new,
            sha256: sha,
            version: "check".into(),
        });
        report.check(
            "update reply",
            host.ok(req, Duration::from_secs(10)).map(|_| String::new()),
        );
        report
    }

    /// The ids and frames of the last known list.
    fn snapshot(seen: &[Event]) -> Vec<(u32, Frame)> {
        let mut out: Vec<(u32, Frame)> = Vec::new();
        for e in seen {
            if let Event::Windows(w) = e {
                for x in &w.changed {
                    out.retain(|(id, _)| *id != x.id);
                    out.push((x.id, x.frame));
                }
                out.retain(|(id, _)| !w.removed.contains(id));
                out.retain(|(id, _)| w.order.contains(id));
            }
        }
        out
    }
}
