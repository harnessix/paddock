//! The host side of computer use: one connection to the `guest-computer`
//! port of a session (`computer.rs` has the wire), its calls, and the
//! frame stream that `paddock mac view` draws.
//!
//! A connection goes through the control socket of the session: the line
//! `connect guest-computer`, the answer `ok`, then the bytes of the port.
//! [`Conn::over`] takes a stream that is already at the port, for the
//! tests.

use super::computer::{
    Call, Code, Data, Done, Element, Failure, FrameHeader, Geometry, Hello, Input, MaxSize, Msg,
    Op, Outcome, Status, Target, LINE_MAX, PAYLOAD_MAX,
};
use super::text;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long the control socket and the hello may take.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// The most frames that wait for [`Conn::next_frame`] while a call
/// waits for its reply. A frame is a whole picture, so the oldest goes
/// first: a guest that streams frames never fills the memory of the host.
pub const PENDING_MAX: usize = 2;
/// How long the rest of a frame may take once its line has started.
const FRAME_REST: Duration = Duration::from_secs(10);

/// One frame of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct FramePng {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub geometry: Geometry,
    pub png: Vec<u8>,
}

/// An accessibility snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// The geometry of the target at its native size, when it was read.
    pub geometry: Geometry,
    pub elements: Vec<Element>,
    pub cut: bool,
}

/// What the stream of a [`Conn`] carries next.
enum Next {
    Reply(u64, Outcome, Vec<u8>),
    Frame(FrameHeader, Vec<u8>),
    Hello(Hello),
}

/// The read side of a connection, with a deadline: each read waits at
/// most until it, so a guest that sends a byte now and then, or a stream
/// of frames, cannot hold a call longer than its time.
struct Timed {
    s: UnixStream,
    deadline: Instant,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.s.set_read_timeout(Some(left))?;
        self.s.read(buf)
    }
}

/// One connection to the computer-use port of a session.
pub struct Conn {
    writer: UnixStream,
    reader: BufReader<Timed>,
    next_id: u64,
    pub hello: Hello,
    /// Frames that came while a reply was awaited, at most
    /// [`PENDING_MAX`], and only while a subscription runs.
    pending: std::collections::VecDeque<FramePng>,
    subscribed: bool,
    /// A read failed inside a message: the stream is no longer at a line
    /// start, so no later call can trust it.
    broken: bool,
    /// The longest time of one call, below the time of its op (tests).
    call_cap: Option<Duration>,
}

fn unreachable(control: &Path, e: impl std::fmt::Display) -> Failure {
    Failure::new(
        Code::Unreachable,
        text::cu_session_unreachable(control, &e.to_string()),
    )
}

impl Conn {
    /// Connect through the control socket of a session.
    pub fn open(control: &Path) -> Result<Conn, Failure> {
        let mut s = UnixStream::connect(control).map_err(|e| unreachable(control, e))?;
        s.set_read_timeout(Some(OPEN_TIMEOUT)).ok();
        s.set_write_timeout(Some(OPEN_TIMEOUT)).ok();
        s.write_all(b"connect guest-computer\n")
            .map_err(|e| unreachable(control, e))?;
        // Byte by byte: what follows the answer belongs to the port.
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        let end = Instant::now() + OPEN_TIMEOUT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Failure::new(Code::Timeout, text::cu_no_answer()));
            }
            s.set_read_timeout(Some(left)).ok();
            match s.read(&mut b) {
                Ok(0) => return Err(unreachable(control, text::cu_closed())),
                Ok(_) if b[0] == b'\n' => break,
                Ok(_) if line.len() < 4096 => line.push(b[0]),
                Ok(_) => return Err(unreachable(control, text::cu_closed())),
                Err(e) => return Err(unreachable(control, e)),
            }
        }
        let answer = String::from_utf8_lossy(&line).into_owned();
        if answer != "ok" {
            let why = answer.strip_prefix("error ").unwrap_or(&answer);
            return Err(Failure::new(Code::Unreachable, text::cu_port_refused(why)));
        }
        Conn::over(s)
    }

    /// A connection on a stream that is at the port: it reads the hello.
    pub fn over(s: UnixStream) -> Result<Conn, Failure> {
        s.set_read_timeout(Some(OPEN_TIMEOUT)).ok();
        let read = s
            .try_clone()
            .map_err(|e| Failure::new(Code::Internal, e.to_string()))?;
        let mut c = Conn {
            writer: s,
            reader: BufReader::new(Timed {
                s: read,
                deadline: Instant::now() + OPEN_TIMEOUT,
            }),
            next_id: 1,
            hello: Hello {
                v: 0,
                display: super::proto::Display { w: 0, h: 0 },
                scale: 1.0,
                screen_recording: false,
                accessibility: false,
            },
            pending: Default::default(),
            subscribed: false,
            broken: false,
            call_cap: None,
        };
        match c.read_next()? {
            Next::Hello(h) => {
                if h.v != super::computer::VERSION {
                    return Err(Failure::new(
                        Code::Unsupported,
                        text::cu_version(h.v, super::computer::VERSION),
                    ));
                }
                c.hello = h;
                Ok(c)
            }
            _ => Err(Failure::new(Code::Unreachable, text::cu_no_hello())),
        }
    }

    /// Every later read ends at `now + d`.
    fn deadline(&mut self, d: Duration) {
        self.reader.get_mut().deadline = Instant::now() + d;
    }

    /// Cap the time of every call (the tests of a guest that never
    /// answers).
    pub fn cap_calls(&mut self, d: Duration) {
        self.call_cap = Some(d);
    }

    fn read_next(&mut self) -> Result<Next, Failure> {
        let r = self.read_message();
        if r.is_err() {
            self.broken = true;
        }
        r
    }

    fn read_message(&mut self) -> Result<Next, Failure> {
        let mut buf = Vec::new();
        let n = (&mut self.reader)
            .take(LINE_MAX as u64 + 1)
            .read_until(b'\n', &mut buf)
            .map_err(read_failure)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            if buf.len() > LINE_MAX {
                return Err(Failure::new(
                    Code::Internal,
                    text::cu_line_too_long(LINE_MAX),
                ));
            }
            return Err(Failure::new(Code::Unreachable, text::cu_closed()));
        }
        buf.pop();
        let msg: Msg = serde_json::from_slice(&buf)
            .map_err(|e| Failure::new(Code::Internal, text::cu_bad_message(&e.to_string())))?;
        let len = msg.bytes();
        if len > PAYLOAD_MAX {
            return Err(Failure::new(
                Code::Internal,
                text::cu_payload_too_big(len, PAYLOAD_MAX),
            ));
        }
        // Read, never allocate, what the line announces: the bytes that
        // come are the memory that the payload takes.
        let mut payload = Vec::new();
        (&mut self.reader)
            .take(len)
            .read_to_end(&mut payload)
            .map_err(read_failure)?;
        if payload.len() as u64 != len {
            return Err(Failure::new(Code::Unreachable, text::cu_closed()));
        }
        Ok(match msg {
            Msg::Hello(h) => Next::Hello(h),
            Msg::Reply { id, outcome } => Next::Reply(id, outcome, payload),
            Msg::Frame(h) => Next::Frame(h, payload),
        })
    }

    /// Send one call and wait for its reply, at most the time of the op
    /// in all: frames and other lines that come meanwhile do not extend
    /// it. Frames of a subscription wait in a short queue for
    /// [`Conn::next_frame`].
    pub fn call(&mut self, op: Op) -> Result<(Option<Data>, Vec<u8>), Failure> {
        if self.broken {
            return Err(Failure::new(Code::Unreachable, text::cu_closed()));
        }
        let id = self.next_id;
        self.next_id += 1;
        let timeout = match self.call_cap {
            Some(cap) => op.timeout().min(cap),
            None => op.timeout(),
        };
        let mut line = serde_json::to_vec(&Call { id, op })
            .map_err(|e| Failure::new(Code::Internal, e.to_string()))?;
        line.push(b'\n');
        self.writer.set_write_timeout(Some(timeout)).ok();
        if let Err(e) = self.writer.write_all(&line) {
            self.broken = true;
            return Err(Failure::new(
                Code::Unreachable,
                text::cu_send_failed(&e.to_string()),
            ));
        }
        self.deadline(timeout);
        loop {
            match self.read_next()? {
                Next::Reply(rid, outcome, payload) if rid == id => {
                    return match outcome {
                        Outcome::Ok { data, .. } => Ok((data, payload)),
                        Outcome::Error(f) => Err(from_guest(f)),
                    }
                }
                // A reply to no call (a line the agent could not read).
                Next::Reply(_, Outcome::Error(f), _) => return Err(from_guest(f)),
                Next::Reply(..) | Next::Hello(_) => {}
                Next::Frame(h, png) => self.queue(frame_of(h, png)),
            }
        }
    }

    /// Keep a frame for [`Conn::next_frame`]: only while subscribed, and
    /// only the newest [`PENDING_MAX`].
    fn queue(&mut self, f: FramePng) {
        if !self.subscribed {
            return;
        }
        while self.pending.len() >= PENDING_MAX {
            self.pending.pop_front();
        }
        self.pending.push_back(f);
    }

    /// The frames that wait now (tests).
    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }

    pub fn status(&mut self, target: Target) -> Result<Status, Failure> {
        match self.call(Op::Status { target })?.0 {
            Some(Data::Status(s)) => Ok(s),
            _ => Err(wrong_reply("status")),
        }
    }

    /// One PNG of the target and its geometry.
    pub fn screenshot(
        &mut self,
        target: Target,
        max: Option<MaxSize>,
        cursor: bool,
    ) -> Result<(Geometry, Vec<u8>), Failure> {
        match self.call(Op::Screenshot {
            target,
            max,
            cursor,
        })? {
            (Some(Data::Shot(g)), png) if !png.is_empty() => Ok((g, png)),
            _ => Err(wrong_reply("screenshot")),
        }
    }

    pub fn input(&mut self, target: Target, action: Input) -> Result<Done, Failure> {
        match self.call(Op::Input { target, action })?.0 {
            Some(Data::Input(d)) => Ok(d),
            _ => Err(wrong_reply("input")),
        }
    }

    pub fn ax(
        &mut self,
        target: Target,
        depth: Option<u32>,
        max_nodes: Option<u32>,
    ) -> Result<Snapshot, Failure> {
        match self.call(Op::Ax {
            target,
            depth,
            max_nodes,
        })? {
            (Some(Data::Ax { geometry, cut, .. }), json) => {
                let elements: Vec<Element> = serde_json::from_slice(&json).map_err(|e| {
                    Failure::new(Code::Internal, text::cu_bad_message(&e.to_string()))
                })?;
                Ok(Snapshot {
                    geometry,
                    elements,
                    cut,
                })
            }
            _ => Err(wrong_reply("ax")),
        }
    }

    /// `AXPress` on an element: its role and label.
    pub fn press(
        &mut self,
        target: Target,
        element: &str,
        role: Option<&str>,
    ) -> Result<(String, String), Failure> {
        match self
            .call(Op::AxPress {
                target,
                element: element.to_string(),
                role: role.map(str::to_string),
            })?
            .0
        {
            Some(Data::Pressed { role, label }) => Ok((role, label)),
            _ => Err(wrong_reply("ax.press")),
        }
    }

    /// Start the frame stream: the geometry of the target now.
    pub fn subscribe(
        &mut self,
        target: Target,
        max_fps: f64,
        max: Option<MaxSize>,
        cursor: bool,
    ) -> Result<Geometry, Failure> {
        self.pending.clear();
        self.subscribed = true;
        match self
            .call(Op::FramesSubscribe {
                target,
                max_fps,
                max,
                cursor,
            })?
            .0
        {
            Some(Data::Subscribed(g)) => Ok(g),
            _ => Err(wrong_reply("frames.subscribe")),
        }
    }

    /// End the frame stream: the last sequence number that the agent
    /// sent. Frames still on the way are dropped.
    pub fn stop(&mut self) -> Result<u64, Failure> {
        let r = match self.call(Op::FramesStop)?.0 {
            Some(Data::Stopped { last_seq }) => Ok(last_seq),
            _ => Err(wrong_reply("frames.stop")),
        };
        self.subscribed = false;
        self.pending.clear();
        r
    }

    /// The next frame of the subscription, None when none came within
    /// `timeout`. A frame is a whole picture: draw it as it is.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<FramePng>, Failure> {
        if let Some(f) = self.pending.pop_front() {
            return Ok(Some(f));
        }
        if self.broken {
            return Err(Failure::new(Code::Unreachable, text::cu_closed()));
        }
        self.deadline(timeout.max(Duration::from_millis(1)));
        if self.reader.buffer().is_empty() {
            // Peek without taking a byte: a timeout here leaves the stream
            // at a line start.
            match self.reader.fill_buf() {
                Ok([]) => return Err(Failure::new(Code::Unreachable, text::cu_closed())),
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(read_failure(e)),
            }
        }
        // A frame has started: its line and its PNG follow at once.
        self.deadline(FRAME_REST);
        match self.read_next()? {
            Next::Frame(h, png) => Ok(Some(frame_of(h, png))),
            Next::Reply(_, Outcome::Error(f), _) => Err(from_guest(f)),
            _ => Ok(None),
        }
    }
}

/// A source of whole frames: the frame stream of a session, or a fake.
/// `paddock mac view` draws from one.
pub trait FrameSource {
    /// The next frame, None when the picture did not change within
    /// `timeout`.
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<FramePng>, Failure>;
    /// End the stream.
    fn stop(&mut self) -> Result<(), Failure>;
}

impl FrameSource for Conn {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<FramePng>, Failure> {
        Conn::next_frame(self, timeout)
    }

    fn stop(&mut self) -> Result<(), Failure> {
        Conn::stop(self).map(|_| ())
    }
}

fn frame_of(h: FrameHeader, png: Vec<u8>) -> FramePng {
    FramePng {
        seq: h.seq,
        width: h.width,
        height: h.height,
        geometry: h.geometry,
        png,
    }
}

/// A failure that the guest sent: its text comes from the guest (the
/// owner of a window, an element), so it loses every control character
/// before it reaches a terminal of the host.
fn from_guest(mut f: Failure) -> Failure {
    f.text = crate::view::strip_controls(&f.text);
    f
}

fn read_failure(e: std::io::Error) -> Failure {
    if matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ) {
        Failure::new(Code::Timeout, text::cu_no_answer())
    } else {
        Failure::new(Code::Unreachable, text::cu_read_failed(&e.to_string()))
    }
}

fn wrong_reply(op: &str) -> Failure {
    Failure::new(Code::Internal, text::cu_wrong_reply(op))
}

#[cfg(test)]
#[path = "computer_client_tests.rs"]
mod tests;
