//! The streaming path of the device file system: the sink that a
//! streaming device writes to, and the reads that it answers.

use super::handle::{Handle, Next, Phase};
use super::{DeviceFs, Inner, KEEP};
use crate::devices::Sink;
use fuser::{Errno, ReplyData};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

impl DeviceFs {
    /// Answer the waiting reads of a streaming handle with what it has
    /// now. The reads that get nothing yet keep waiting.
    pub(super) fn drain(handle: &mut Handle) -> Vec<(ReplyData, Result<Vec<u8>, Errno>)> {
        let Handle {
            phase: Phase::Streaming(s),
            cursor,
            waiting,
            ..
        } = handle
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut keep = Vec::new();
        for (reply, size, pid) in std::mem::take(waiting) {
            match s.next(cursor, size) {
                Next::Data(d) => out.push((reply, Ok(d))),
                Next::Error(e) => out.push((reply, Err(e))),
                Next::Wait => keep.push((reply, size, pid)),
            }
        }
        *waiting = keep;
        out
    }

    pub(super) fn answer(replies: Vec<(ReplyData, Result<Vec<u8>, Errno>)>) {
        for (reply, r) in replies {
            match r {
                Ok(d) => reply.data(&d),
                Err(e) => reply.error(e),
            }
        }
    }

    /// The device call of a stream ended: the reads after the last byte
    /// get the end of file, or the error. Returns how many bytes came.
    pub(super) fn finish_stream(inner: &Inner, fh: u64, result: Result<(), Errno>) -> usize {
        let (total, replies) = {
            let mut h = inner.handles.lock().unwrap();
            let Some(handle) = h.get_mut(&fh) else {
                return 0;
            };
            let Phase::Streaming(s) = &mut handle.phase else {
                return 0;
            };
            s.finish(result);
            (s.total(), Self::drain(handle))
        };
        Self::answer(replies);
        total
    }
}

/// The sink of a streaming device: the bytes go to the handle, and the
/// waiting reads get them at once. The device waits while the reader is
/// `KEEP` bytes behind, and stops (its call sees `cancelled()`) when the
/// handle is closed, the stream is over its limit, or the deadline passed.
pub(super) struct StreamSink {
    pub inner: Arc<Inner>,
    pub fh: u64,
    pub cancel: Arc<AtomicBool>,
}

impl Sink for StreamSink {
    fn push(&mut self, bytes: &[u8]) {
        let mut h = self.inner.handles.lock().unwrap();
        loop {
            let open = match h.get_mut(&self.fh) {
                Some(Handle {
                    phase: Phase::Streaming(s),
                    cursor,
                    ..
                }) if s.open() => Some(s.unread(*cursor) < KEEP),
                _ => None,
            };
            match open {
                None => {
                    self.cancel.store(true, Ordering::SeqCst);
                    return;
                }
                Some(true) => break,
                Some(false) if self.cancel.load(Ordering::SeqCst) => return,
                Some(false) => {
                    h = self
                        .inner
                        .space
                        .wait_timeout(h, Duration::from_millis(100))
                        .unwrap()
                        .0;
                }
            }
        }
        let Some(handle) = h.get_mut(&self.fh) else {
            return;
        };
        let Phase::Streaming(s) = &mut handle.phase else {
            return;
        };
        if !s.push(bytes) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        let replies = DeviceFs::drain(handle);
        drop(h);
        DeviceFs::answer(replies);
    }
}
