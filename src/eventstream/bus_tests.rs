use super::*;
use crate::eventstream::bus::queue;
use crate::eventstream::schema::{Disk, Event, Stream, StreamState};
use std::time::Duration;

fn disk(free: u64) -> Event {
    Event::Disk(Disk {
        dir: "/m".into(),
        free,
    })
}

fn bus() -> (tempfile::TempDir, Bus) {
    let t = tempfile::tempdir().unwrap();
    let b = Bus::at(t.path().join("events"));
    (t, b)
}

#[test]
fn a_publish_without_followers_is_a_no_op() {
    let (_t, b) = bus();
    assert_eq!(b.send("{\"v\":1,\"kind\":\"disk\"}\n"), 0);
    b.publish(disk(1));
    assert!(!b.dir().exists(), "a publisher never makes the folder");
}

#[test]
fn two_followers_get_every_line_in_order() {
    let (_t, b) = bus();
    // Two followers in one process: the second takes another pid.
    let f1 = Follower::bind(&b, std::process::id()).unwrap();
    let (tx1, rx1) = queue();
    f1.serve(tx1).unwrap();
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let f2 = Follower::bind(&b, other.id()).unwrap();
    let (tx2, rx2) = queue();
    f2.serve(tx2).unwrap();
    assert_eq!(b.followers().len(), 2);
    for n in 0..5 {
        assert_eq!(b.send(&Line::at(n, disk(n)).to_text()), 2);
    }
    for rx in [&rx1, &rx2] {
        for n in 0..5u64 {
            let line = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let l: Line = serde_json::from_str(&line).unwrap();
            assert_eq!(l.event, disk(n));
        }
    }
    assert!(!f1.take_lost() && !f2.take_lost());
    let _ = other.kill();
    let _ = other.wait();
}

#[test]
fn the_socket_goes_with_the_follower() {
    let (_t, b) = bus();
    let f = Follower::bind(&b, std::process::id()).unwrap();
    let path = f.path().to_path_buf();
    assert!(path.exists());
    drop(f);
    assert!(!path.exists());
}

#[test]
fn a_dead_followers_socket_goes_at_the_next_publish() {
    let (_t, b) = bus();
    // A follower that was killed: its socket file stays, nobody listens.
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let pid = dead.id();
    dead.wait().unwrap();
    std::fs::create_dir_all(b.dir()).unwrap();
    let path = b.dir().join(format!("{pid}.sock"));
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    assert_eq!(b.send(&Line::at(1, disk(1)).to_text()), 0);
    assert!(!path.exists(), "the socket of a dead pid goes");
    assert!(!lost_marker(&path).exists());
}

#[test]
fn a_live_follower_that_refuses_is_marked_lost_not_removed() {
    let (_t, b) = bus();
    // A live pid (this test) whose socket has no listener: a stand-in for
    // a full backlog on macOS, where connect is refused.
    std::fs::create_dir_all(b.dir()).unwrap();
    let path = b.dir().join(format!("{}.sock", std::process::id()));
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert_eq!(b.send(&Line::at(1, disk(1)).to_text()), 0);
    assert!(path.exists());
    assert!(lost_marker(&path).exists());
    // The follower sees the marker once.
    let f = Follower::bind(&b, std::process::id());
    // bind takes over its own stale socket and clears the marker.
    let f = f.unwrap();
    assert!(!f.take_lost());
    mark_lost(f.path());
    assert!(f.take_lost());
    assert!(!f.take_lost());
}

#[test]
fn bad_lines_of_a_publisher_are_dropped_and_the_good_ones_pass() {
    let (_t, b) = bus();
    let f = Follower::bind(&b, std::process::id()).unwrap();
    let (tx, rx) = queue();
    f.serve(tx).unwrap();
    let good = Line::at(
        3,
        Event::Stream(Stream {
            state: StreamState::Lost,
        }),
    )
    .to_text();
    let text = format!(
        "not json\n\n{{\"no\":\"v\"}}\n{good}{}\n",
        "x".repeat(70_000)
    );
    let mut s = std::os::unix::net::UnixStream::connect(f.path()).unwrap();
    s.write_all(text.as_bytes()).unwrap();
    drop(s);
    let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(got, good.trim_end());
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn a_silent_publisher_does_not_hold_the_follower_forever() {
    let (_t, b) = bus();
    let f = Follower::bind(&b, std::process::id()).unwrap();
    let (tx, rx) = queue();
    f.serve(tx).unwrap();
    // Connects and writes nothing: the follower gives up after READ_WAIT.
    let _silent = std::os::unix::net::UnixStream::connect(f.path()).unwrap();
    let begin = std::time::Instant::now();
    assert_eq!(b.send(&Line::at(1, disk(9)).to_text()), 1);
    let got = rx.recv_timeout(READ_WAIT * 3).unwrap();
    assert!(got.contains("\"free\":9"), "{got}");
    assert!(begin.elapsed() < READ_WAIT * 3);
}

#[test]
fn the_bus_of_an_env_is_under_its_run_directory() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::env::HostEnv::from_parts(
        t.path().join("home"),
        "u".into(),
        501,
        20,
        [(
            "XDG_RUNTIME_DIR".to_string(),
            t.path().join("run").display().to_string(),
        )]
        .into_iter()
        .collect(),
    );
    let b = Bus::of(&env);
    if cfg!(target_os = "macos") {
        // A short run directory stays; a long one moves under /tmp.
        let short = b.dir() == t.path().join("run/paddock/events");
        assert!(
            short || b.dir().starts_with("/tmp/paddock-501"),
            "{:?}",
            b.dir()
        );
    } else {
        assert_eq!(b.dir(), t.path().join("run/paddock/events"));
    }
    assert!(b.dir().ends_with(SOCKET_ID));
}

#[test]
fn a_reader_that_lags_marks_the_follower_lost() {
    let (_t, b) = bus();
    let f = Follower::bind(&b, std::process::id()).unwrap();
    let (tx, rx) = queue();
    f.serve(tx).unwrap();
    // Nobody reads rx: the queue fills, the rest are dropped and marked.
    let line = Line::at(1, disk(1)).to_text();
    let text = line.repeat(QUEUE + 10);
    let mut s = std::os::unix::net::UnixStream::connect(f.path()).unwrap();
    s.write_all(text.as_bytes()).unwrap();
    drop(s);
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !lost_marker(f.path()).exists() {
        assert!(std::time::Instant::now() < end, "never marked lost");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f.take_lost());
    assert_eq!(rx.try_iter().count(), QUEUE);
}

#[test]
fn bind_clears_orphaned_lost_markers() {
    let (_t, b) = bus();
    std::fs::create_dir_all(b.dir()).unwrap();
    let marker = b.dir().join("123456789.lost");
    std::fs::write(&marker, "").unwrap();
    let _f = Follower::bind(&b, std::process::id()).unwrap();
    assert!(!marker.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_backlog_does_not_block_the_publisher() {
    let (_t, b) = bus();
    std::fs::create_dir_all(b.dir()).unwrap();
    let path = b.dir().join(format!("{}.sock", std::process::id()));
    use std::os::fd::AsRawFd;
    let listener = UnixListener::bind(&path).unwrap();
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
    let mut connections = Vec::new();
    loop {
        match connect_timeout(&path) {
            Ok(s) => connections.push(s),
            Err(_) => break,
        }
        assert!(connections.len() < 1024);
    }
    let start = std::time::Instant::now();
    assert_eq!(b.send("test\n"), 0);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(lost_marker(&path).exists());
}
