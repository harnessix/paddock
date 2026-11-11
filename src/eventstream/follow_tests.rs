use super::*;
use crate::eventstream::schema::{self, StreamState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A writer that the test reads while the follower writes.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// A reader that left: every write fails.
struct Gone;

impl Write for Gone {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fast(json: bool) -> Opts {
    Opts {
        json,
        disk_every: Duration::from_millis(200),
        tick: Duration::from_millis(50),
    }
}

#[test]
fn started_then_events_then_disk_and_lost_lines() {
    let t = tempfile::tempdir().unwrap();
    let bus = Bus::at(t.path().join("events"));
    let out = Shared::default();
    let stop = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(true));
    let worker = {
        let (bus, mut out, stop, busy) = (bus.clone(), out.clone(), stop.clone(), busy.clone());
        std::thread::spawn(move || {
            let disk = move || {
                busy.load(Ordering::SeqCst).then(|| Disk {
                    dir: "/m".into(),
                    free: 42,
                })
            };
            follow(
                &bus,
                &mut out,
                &|| stop.load(Ordering::SeqCst),
                &disk,
                fast(true),
            )
        })
    };
    wait_for("the socket", || bus.followers().len() == 1);
    bus.publish(Event::Golden(schema::Golden {
        state: schema::GoldenState::Removed,
        version: None,
        bytes: 7,
    }));
    wait_for("the golden line", || {
        out.lines().iter().any(|l| l.contains("\"golden\""))
    });
    wait_for("a disk line", || {
        out.lines().iter().any(|l| l.contains("\"disk\""))
    });
    // A missed line of a publisher: the follower says lost once.
    let (_, sock) = bus.followers().remove(0);
    std::fs::write(crate::eventstream::bus::lost_marker(&sock), "").unwrap();
    wait_for("the lost line", || {
        out.lines().iter().any(|l| l.contains("\"lost\""))
    });
    busy.store(false, Ordering::SeqCst);
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap().unwrap();
    assert!(bus.followers().is_empty(), "the socket goes at the end");
    let lines = out.lines();
    let first: Line = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(
        first.event,
        Event::Stream(Stream {
            state: StreamState::Started
        })
    );
    for l in &lines {
        let parsed: Line = serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}"));
        assert_eq!(parsed.v, schema::V);
    }
    assert_eq!(lines.iter().filter(|l| l.contains("\"lost\"")).count(), 1);
}

#[test]
fn no_disk_line_while_nothing_runs() {
    let t = tempfile::tempdir().unwrap();
    let bus = Bus::at(t.path().join("events"));
    let mut out = Shared::default();
    let begin = Instant::now();
    let stop = || begin.elapsed() > Duration::from_millis(500);
    follow(&bus, &mut out, &stop, &|| None, fast(true)).unwrap();
    let lines = out.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
}

#[test]
fn the_follower_ends_when_its_reader_goes() {
    let t = tempfile::tempdir().unwrap();
    let bus = Bus::at(t.path().join("events"));
    // Never told to stop: the failed write of the first line ends it.
    follow(&bus, &mut Gone, &|| false, &|| None, fast(true)).unwrap();
    assert!(bus.followers().is_empty());
}

#[test]
fn text_form_of_a_line() {
    let l = Line::at(
        0,
        Event::Disk(Disk {
            dir: "/m".into(),
            free: 5,
        }),
    );
    assert_eq!(
        human(l.to_text().trim_end()),
        "1970-01-01T00:00:00Z disk dir=/m free=5"
    );
    assert_eq!(human("not json"), "not json");
    let mut out = Shared::default();
    assert!(emit(&mut out, &l.to_text(), false));
    assert_eq!(out.lines(), ["1970-01-01T00:00:00Z disk dir=/m free=5"]);
}

#[test]
fn disk_now_is_quiet_without_a_session_or_a_build() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::env::HostEnv::from_parts(
        t.path().join("home"),
        "u".into(),
        501,
        20,
        [(
            "XDG_DATA_HOME".to_string(),
            t.path().join("data").display().to_string(),
        )]
        .into_iter()
        .collect(),
    );
    assert_eq!(disk_now(&env), None);
    // A build holds the setup lock: the free space of the folder, which
    // does not exist yet, comes from its nearest existing parent.
    let layout = crate::macvm::layout::Layout::new(&env);
    std::fs::create_dir_all(&layout.root).unwrap();
    let f = std::fs::File::create(&layout.setup_lock).unwrap();
    use std::os::fd::AsRawFd;
    assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
    let d = disk_now(&env).expect("a build runs");
    assert_eq!(d.dir, layout.root);
    assert!(d.free > 0);
    assert_eq!(
        existing_ancestor(&t.path().join("a/b/c")),
        t.path().to_path_buf()
    );
}

#[test]
fn detects_a_closed_pipe_without_writing() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let write = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    assert!(!reader_gone(write.as_raw_fd()));
    drop(read);
    assert!(reader_gone(write.as_raw_fd()));
}
