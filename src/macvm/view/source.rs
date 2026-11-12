//! The two ends that the view talks to: a frame source (the
//! `frames.subscribe` and `frames.stop` contract of the guest agent) and
//! an input sink (the actions of `paddock mac click|type|key|scroll|drag`).
//! The view only needs these traits. The fakes here draw a test picture
//! and record the actions, for the tests and for `--fake-source`.

use super::map::Action;
use super::png;
use crate::macvm::text;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the view shows: the whole guest desktop, or the windows of one
/// app session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The desktop of the guest of `app`, or of the only running guest.
    Desktop {
        app: Option<String>,
        instance: Option<u32>,
    },
    /// Only the windows of one app session.
    App { name: String, instance: Option<u32> },
}

impl Target {
    /// The name for the status line and the messages.
    pub fn label(&self) -> String {
        match self {
            Target::Desktop { app: None, .. } => text::view_guest_desktop(),
            Target::Desktop {
                app: Some(a),
                instance,
            } => text::view_desktop_of(&with_instance(a, *instance)),
            Target::App { name, instance } => with_instance(name, *instance),
        }
    }
}

fn with_instance(name: &str, n: Option<u32>) -> String {
    match n {
        Some(n) if n > 1 => text::view_instance(name, n),
        _ => name.to_string(),
    }
}

/// One frame of the stream: PNG bytes, the sequence number and the size
/// that the guest states. The view checks the size against the PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

impl Frame {
    /// Check the frame before it reaches the terminal: a real PNG within
    /// the limits of `png.rs`, of the stated size.
    pub fn check(&self) -> Result<(), String> {
        let h = png::header(&self.png)?;
        if (h.width, h.height) != (self.width, self.height) {
            return Err(text::view_frame_size_mismatch(
                self.width,
                self.height,
                h.width,
                h.height,
            ));
        }
        Ok(())
    }
}

/// An item of the frame stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Frame(Frame),
    /// The stream ended, with the reason.
    End(String),
}

/// The largest frame that the view can show at one guest pixel per
/// terminal pixel: the room above the status line. It is `max {width,
/// height}` of `frames.subscribe`, so the guest scales its frames down
/// before they cross the TTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Max {
    pub width: u32,
    pub height: u32,
}

impl Max {
    /// A `width` x `height` picture scaled down to fit, aspect kept, never
    /// up. At least 1 x 1.
    pub fn fit(&self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = (u64::from(width.max(1)), u64::from(height.max(1)));
        let (mw, mh) = (u64::from(self.width.max(1)), u64::from(self.height.max(1)));
        if w <= mw && h <= mh {
            return (w as u32, h as u32);
        }
        let (num, den) = if mw * h <= mh * w { (mw, w) } else { (mh, h) };
        (
            ((w * num / den).max(1)) as u32,
            ((h * num / den).max(1)) as u32,
        )
    }
}

/// The frame contract: `frames.subscribe {target, max_fps, max}` streams
/// frames only on change, `frames.stop` ends the stream. A new subscribe
/// replaces the old stream, and its `seq` starts at 1 again.
pub trait FrameSource {
    /// Subscribe; frames come on the receiver until [`FrameSource::stop`]
    /// or the next subscribe. Frames are at most `max`.
    fn subscribe(
        &mut self,
        target: &Target,
        max_fps: u32,
        max: Max,
    ) -> Result<Receiver<Item>, String>;
    /// `frames.stop`. Leaves the target and its session running.
    fn stop(&mut self);
}

/// The input end: one action on the target, in frame coordinates.
pub trait InputSink {
    fn send(&mut self, action: &Action) -> Result<(), String>;
}

/// The fake source: a test picture of `width` x `height` with a moving
/// square, a new frame every `every`, and one frame of the same picture
/// twice in a row (which the view must not draw again).
pub struct FakeSource {
    pub width: u32,
    pub height: u32,
    pub every: Duration,
    /// Frames to send; None runs until stopped.
    pub count: Option<u64>,
    /// The `max` of every subscribe, in order.
    pub subscriptions: Arc<Mutex<Vec<Max>>>,
    /// The stop flag of the current stream.
    stop: Arc<Mutex<bool>>,
}

impl FakeSource {
    pub fn new(width: u32, height: u32, every: Duration, count: Option<u64>) -> FakeSource {
        FakeSource {
            width,
            height,
            every,
            count,
            subscriptions: Arc::default(),
            stop: Arc::new(Mutex::new(false)),
        }
    }

    /// The test picture number `n`.
    pub fn picture(width: u32, height: u32, n: u64) -> Vec<u8> {
        let (w, h) = (width as usize, height as usize);
        let mut px = vec![0u8; w * h * 4];
        let side = (w.min(h) / 4).max(1);
        let span = w.saturating_sub(side).max(1);
        let sx = (n as usize * 7) % span;
        let sy = h.saturating_sub(side) / 2;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let inside = x >= sx && x < sx + side && y >= sy && y < sy + side;
                let (r, g, b) = if inside {
                    (230, 80, 40)
                } else {
                    // The blue changes with every picture.
                    (
                        (x * 255 / w.max(1)) as u8,
                        (y * 255 / h.max(1)) as u8,
                        (n * 37 % 256) as u8,
                    )
                };
                px[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        px
    }
}

impl FrameSource for FakeSource {
    fn subscribe(
        &mut self,
        _target: &Target,
        max_fps: u32,
        max: Max,
    ) -> Result<Receiver<Item>, String> {
        // A new subscribe replaces the old stream, as the contract says.
        self.stop();
        self.subscriptions.lock().unwrap().push(max);
        let (tx, rx) = mpsc::channel();
        let (w, h) = max.fit(self.width, self.height);
        let count = self.count;
        let every = self
            .every
            .max(Duration::from_millis(1000 / u64::from(max_fps.max(1))));
        let stop = Arc::new(Mutex::new(false));
        self.stop = stop.clone();
        std::thread::spawn(move || {
            let mut seq = 0u64;
            loop {
                if *stop.lock().unwrap() || count.is_some_and(|c| seq >= c) {
                    let _ = tx.send(Item::End("the fake source stopped".into()));
                    return;
                }
                // Frames 3 and 4 have the same picture.
                let n = if seq == 4 { 3 } else { seq };
                let png = png::encode(w, h, &FakeSource::picture(w, h, n));
                seq += 1;
                let frame = Frame {
                    seq,
                    width: w,
                    height: h,
                    png,
                };
                if tx.send(Item::Frame(frame)).is_err() {
                    return;
                }
                std::thread::sleep(every);
            }
        });
        Ok(rx)
    }

    fn stop(&mut self) {
        *self.stop.lock().unwrap() = true;
    }
}

/// The fake sink: it records every action.
#[derive(Debug, Default, Clone)]
pub struct Recorder {
    pub actions: Arc<Mutex<Vec<Action>>>,
}

impl InputSink for Recorder {
    fn send(&mut self, action: &Action) -> Result<(), String> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(())
    }
}
