//! The host connection against the guest server on a fake screen, and
//! through a control socket served by `vm::bridge`.

use super::*;
use crate::guest::computer::serve;
use crate::guest::computer_fake::{self as fake, PNG};
use crate::macvm::computer::{Element, Scope};
use crate::macvm::proto::{Frame, Point};
use crate::vm::bridge::{self, Guest};
use std::sync::Arc;

fn conn() -> (
    Conn,
    Arc<std::sync::Mutex<fake::FakeState>>,
    Arc<std::sync::Mutex<crate::guest::computer::Scene>>,
) {
    let (server, st, sc) = fake::server(fake::scene());
    let (a, b) = UnixStream::pair().unwrap();
    std::thread::spawn(move || serve(server, b));
    (Conn::over(a).unwrap(), st, sc)
}

#[test]
fn the_calls_of_a_connection() {
    let (mut c, st, _) = conn();
    assert_eq!(c.hello.scale, 2.0);
    let s = c.status(Target::APP).unwrap();
    assert!(s.app_running);
    let (g, png) = c.screenshot(Target::APP, None, false).unwrap();
    assert_eq!(g.width, 1600);
    assert!(png.starts_with(PNG));
    let d = c
        .input(
            Target::DESKTOP,
            Input::Move {
                at: Point { x: 1.0, y: 1.0 },
            },
        )
        .unwrap();
    assert_eq!(d.events, 1);
    st.lock().unwrap().elements = vec![Element {
        id: "10:0".into(),
        role: "AXButton".into(),
        subrole: String::new(),
        label: "OK".into(),
        value: None,
        frame: Frame::new(1.0, 2.0, 3.0, 4.0),
        enabled: true,
        focused: false,
        depth: 1,
    }];
    let snap = c.ax(Target::APP, None, None).unwrap();
    assert_eq!(snap.elements.len(), 1);
    assert_eq!(c.press(Target::APP, "10:0", None).unwrap().1, "OK");
}

#[test]
fn a_refusal_is_a_failure_with_its_code() {
    let (mut c, _, _) = conn();
    let e = c
        .input(
            Target::APP,
            Input::Move {
                at: Point { x: 1.0, y: 1.0 },
            },
        )
        .unwrap_err();
    assert_eq!(e.code, Code::Outside);
    // The connection goes on after it.
    assert!(c.status(Target::DESKTOP).is_ok());
}

#[test]
fn the_frame_stream_through_the_source_trait() {
    let (mut c, st, _) = conn();
    let g = c.subscribe(Target::APP, 30.0, None, true).unwrap();
    assert_eq!(g.width, 1600);
    let f = FrameSource::next_frame(&mut c, Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(f.seq, 1);
    assert_eq!((f.width, f.height), (1600, 1200));
    // Nothing changes: no frame within the wait.
    assert_eq!(c.next_frame(Duration::from_millis(100)).unwrap(), None);
    st.lock().unwrap().pixels = vec![7];
    let f = c.next_frame(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(f.seq, 2);
    assert!(f.png.ends_with(&[7]));
    // A call while frames flow: the frames wait in the queue.
    st.lock().unwrap().pixels = vec![8];
    std::thread::sleep(Duration::from_millis(100));
    assert!(c.status(Target::APP).is_ok());
    let f = c.next_frame(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(f.seq, 3);
    assert_eq!(c.stop().unwrap(), 3);
    assert_eq!(c.next_frame(Duration::from_millis(100)).unwrap(), None);
    FrameSource::stop(&mut c).unwrap();
}

#[test]
fn a_closed_guest_is_unreachable() {
    let (a, b) = UnixStream::pair().unwrap();
    drop(b);
    assert_eq!(Conn::over(a).err().unwrap().code, Code::Unreachable);
}

#[test]
fn another_version_is_refused() {
    let (a, mut b) = UnixStream::pair().unwrap();
    b.write_all(b"{\"ev\":\"hello\",\"v\":99,\"display\":{\"w\":1,\"h\":1},\"scale\":1,\"screen_recording\":true,\"accessibility\":true}\n")
        .unwrap();
    let e = Conn::over(a).err().unwrap();
    assert_eq!(e.code, Code::Unsupported);
    assert!(e.text.contains("version 99"), "{}", e.text);
}

#[test]
fn a_payload_above_the_limit_is_refused() {
    let (a, mut b) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        b.write_all(b"{\"ev\":\"hello\",\"v\":1,\"display\":{\"w\":1,\"h\":1},\"scale\":1,\"screen_recording\":true,\"accessibility\":true}\n").unwrap();
        let mut line = String::new();
        let mut r = BufReader::new(b.try_clone().unwrap());
        r.read_line(&mut line).unwrap();
        let big = PAYLOAD_MAX + 1;
        b.write_all(format!("{{\"ev\":\"frame\",\"seq\":1,\"width\":1,\"height\":1,\"geometry\":{{\"origin\":{{\"x\":0,\"y\":0}},\"points\":{{\"w\":1,\"h\":1}},\"width\":1,\"height\":1}},\"bytes\":{big}}}\n").as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
    });
    let mut c = Conn::over(a).unwrap();
    let e = c.status(Target::DESKTOP).unwrap_err();
    assert_eq!(e.code, Code::Internal);
    assert!(e.text.contains("announced"), "{}", e.text);
}

/// A session control socket whose `connect guest-computer` reaches a
/// guest server on a fake screen.
struct FakeVm;

impl Guest for FakeVm {
    fn connect(&self, port: u32) -> anyhow::Result<std::os::fd::OwnedFd> {
        anyhow::ensure!(port == crate::vm::ports::GUEST_COMPUTER, "no such port");
        let (server, _, _) = fake::server(fake::scene());
        let (a, b) = UnixStream::pair()?;
        std::thread::spawn(move || serve(server, b));
        Ok(a.into())
    }

    fn stop(&self) {}
}

#[test]
fn open_goes_through_the_control_socket() {
    let t = tempfile::tempdir().unwrap();
    let sock = t.path().join("control.sock");
    bridge::serve_control(bridge::bind_control(&sock).unwrap(), Arc::new(FakeVm));
    let mut c = Conn::open(&sock).unwrap();
    assert_eq!(c.status(Target::APP).unwrap().windows.len(), 2);
    let (g, _) = c
        .screenshot(
            Target {
                scope: Scope::Desktop,
            },
            None,
            false,
        )
        .unwrap();
    assert_eq!(g.width, 2880);
}

#[test]
fn open_without_a_session_is_unreachable() {
    let t = tempfile::tempdir().unwrap();
    let e = Conn::open(&t.path().join("none.sock")).err().unwrap();
    assert_eq!(e.code, Code::Unreachable);
    assert!(e.text.contains("paddock app list"), "{}", e.text);
}

/// A guest whose session refuses the port, as an old shim does.
struct NoPort;

impl Guest for NoPort {
    fn connect(&self, _port: u32) -> anyhow::Result<std::os::fd::OwnedFd> {
        anyhow::bail!("nothing listens")
    }

    fn stop(&self) {}
}

#[test]
fn a_refused_port_says_why() {
    let t = tempfile::tempdir().unwrap();
    let sock = t.path().join("control.sock");
    bridge::serve_control(bridge::bind_control(&sock).unwrap(), Arc::new(NoPort));
    let e = Conn::open(&sock).err().unwrap();
    assert_eq!(e.code, Code::Unreachable);
    assert!(e.text.contains("nothing listens"), "{}", e.text);
}

const HELLO: &[u8] = b"{\"ev\":\"hello\",\"v\":1,\"display\":{\"w\":1,\"h\":1},\"scale\":1,\"screen_recording\":true,\"accessibility\":true}\n";

fn frame_line(seq: u64, bytes: usize) -> String {
    format!("{{\"ev\":\"frame\",\"seq\":{seq},\"width\":1,\"height\":1,\"geometry\":{{\"origin\":{{\"x\":0,\"y\":0}},\"points\":{{\"w\":1,\"h\":1}},\"width\":1,\"height\":1}},\"bytes\":{bytes}}}\n")
}

/// A hostile guest: after the hello it answers each call with `answer`,
/// which gets the id of the call and the stream.
fn hostile(answer: impl Fn(u64, &mut UnixStream) + Send + 'static) -> Conn {
    let (a, mut b) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        b.write_all(HELLO).unwrap();
        let mut r = BufReader::new(b.try_clone().unwrap());
        let mut line = String::new();
        while r.read_line(&mut line).unwrap_or(0) > 0 {
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            answer(v["id"].as_u64().unwrap(), &mut b);
            line.clear();
        }
    });
    Conn::over(a).unwrap()
}

#[test]
fn a_guest_that_streams_frames_and_other_ids_cannot_hold_a_call() {
    let mut c = hostile(|id, b| {
        // Frames and replies to other ids, for longer than the call may
        // take: each of them came in time, none is the answer.
        let end = std::time::Instant::now() + Duration::from_secs(3);
        let mut seq = 0;
        while std::time::Instant::now() < end {
            seq += 1;
            let other = format!(
                "{{\"ev\":\"reply\",\"id\":{},\"result\":\"ok\"}}\n",
                id + 1000
            );
            let msg = [frame_line(seq, 4).as_bytes(), b"fake", other.as_bytes()].concat();
            if b.write_all(&msg).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    c.cap_calls(Duration::from_millis(300));
    let t = std::time::Instant::now();
    let e = c.status(Target::DESKTOP).unwrap_err();
    assert_eq!(e.code, Code::Timeout, "{}", e.text);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // Without a subscription no frame waits in memory.
    assert_eq!(c.pending_frames(), 0);
    // The stream is no longer at a line start: later calls refuse at once.
    assert_eq!(
        c.status(Target::DESKTOP).unwrap_err().code,
        Code::Unreachable
    );
}

#[test]
fn a_guest_that_trickles_a_line_cannot_hold_a_call() {
    let mut c = hostile(|_, b| {
        // One byte now and then, quicker than any read timeout.
        for _ in 0..60 {
            if b.write_all(b"{").is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    c.cap_calls(Duration::from_millis(300));
    let t = std::time::Instant::now();
    assert_eq!(c.status(Target::DESKTOP).unwrap_err().code, Code::Timeout);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn frames_during_a_call_wait_in_a_short_queue() {
    let mut c = hostile(|id, b| {
        let reply = if id == 1 {
            "{\"ev\":\"reply\",\"id\":1,\"result\":\"ok\",\"data\":{\"subscribed\":{\"origin\":{\"x\":0,\"y\":0},\"points\":{\"w\":1,\"h\":1},\"width\":1,\"height\":1}}}\n".to_string()
        } else {
            let mut m = String::new();
            for seq in 1..=50 {
                m.push_str(&frame_line(seq, 0));
            }
            m + "{\"ev\":\"reply\",\"id\":2,\"result\":\"ok\",\"data\":{\"stopped\":{\"last_seq\":50}}}\n"
        };
        let _ = b.write_all(reply.as_bytes());
    });
    c.subscribe(Target::DESKTOP, 1.0, None, false).unwrap();
    // 50 frames came during the call: only the newest wait.
    c.call(Op::Status {
        target: Target::DESKTOP,
    })
    .unwrap();
    assert!(c.pending_frames() <= PENDING_MAX, "{}", c.pending_frames());
}

#[test]
fn a_guest_failure_loses_its_control_characters() {
    let mut c = hostile(|id, b| {
        let line = format!("{{\"ev\":\"reply\",\"id\":{id},\"result\":\"error\",\"code\":\"covered\",\"text\":\"a window of \\u001b]0;pwned\\u0007\\u001b[2Jevil covers it\"}}\n");
        let _ = b.write_all(line.as_bytes());
    });
    let e = c.status(Target::DESKTOP).unwrap_err();
    assert_eq!(e.code, Code::Covered);
    assert!(!e.text.chars().any(char::is_control), "{:?}", e.text);
    assert_eq!(e.text, "a window of evil covers it");
}
