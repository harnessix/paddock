//! One open of a device file: the request it collects, the phase of its
//! request, and the response of a streaming device as far as it came.

use fuser::{Errno, ReplyData};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// How many unread bytes a stream keeps before the device waits for the
/// reader. The read front is dropped at half of it, so a stream holds
/// about this much in the broker, whatever its size.
pub const KEEP: usize = 1 << 20;

pub(super) enum Phase {
    /// The client writes the request.
    Collect,
    /// The device works on it, and answers whole.
    Running,
    /// The device writes the response as it goes.
    Streaming(Stream),
    Done(Result<Arc<Vec<u8>>, Errno>),
}

pub(super) enum Kind {
    Catalog,
    Device(usize),
    /// A secret file: its name and the value, held while the file is open.
    Secret(String, Arc<crate::credentials::secret::Secret>),
    Horse(String, Vec<u8>, bool),
}

pub(super) struct Handle {
    pub kind: Kind,
    pub request: Vec<u8>,
    pub phase: Phase,
    /// Reads that wait for the response: the reply, its size and the
    /// thread that reads (a host thread id; FUSE translates it).
    pub waiting: Vec<(ReplyData, u32, u32)>,
    /// How much of the response the client has read. The kernel offset
    /// counts the request bytes too, so the response keeps its own.
    pub cursor: usize,
    /// The length of the request once it was sent: the kernel offset of
    /// the first response byte.
    pub sent: usize,
    pub cancel: Arc<AtomicBool>,
}

impl Handle {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            request: Vec::new(),
            phase: Phase::Collect,
            waiting: Vec::new(),
            cursor: 0,
            sent: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// What a read of a stream gets now.
pub(super) enum Next {
    /// Bytes, or an empty vector at the end of the stream.
    Data(Vec<u8>),
    Error(Errno),
    /// Nothing yet: the read waits.
    Wait,
}

/// The response of a streaming device, as far as it came. The bytes that
/// the client read are dropped from the front; `base` is the response
/// offset of the first byte that is still here.
pub(super) struct Stream {
    buf: Vec<u8>,
    base: usize,
    /// The device ended the response.
    pub done: bool,
    /// The device failed; the reads after the last byte fail with it.
    pub err: Option<Errno>,
    max: usize,
}

impl Stream {
    /// A stream of at most `max` bytes.
    pub fn new(max: usize) -> Self {
        Self {
            buf: Vec::new(),
            base: 0,
            done: false,
            err: None,
            max,
        }
    }

    /// The response offset of the first byte that is still here.
    pub fn base(&self) -> usize {
        self.base
    }

    /// How many bytes the device wrote so far.
    pub fn total(&self) -> usize {
        self.base + self.buf.len()
    }

    /// The bytes after `cursor` that the client has not read.
    pub fn unread(&self, cursor: usize) -> usize {
        self.total().saturating_sub(cursor)
    }

    /// True while the device may write more: not ended, not failed.
    pub fn open(&self) -> bool {
        !self.done && self.err.is_none()
    }

    /// Append `bytes`. False when the stream is over its limit: the
    /// reads after the last byte fail with EMSGSIZE, and the device must
    /// stop.
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        if !self.open() {
            return false;
        }
        if self.total() + bytes.len() > self.max {
            self.err = Some(Errno::EMSGSIZE);
            return false;
        }
        self.buf.extend_from_slice(bytes);
        true
    }

    /// Mark the end: the result of the device call. An earlier error
    /// (the limit) stays.
    pub fn finish(&mut self, result: Result<(), Errno>) {
        if let (Err(e), None) = (result, self.err) {
            self.err = Some(e);
        }
        self.done = true;
    }

    /// What a read of `size` bytes at `cursor` gets now. A read below the
    /// dropped front fails with ESPIPE: a stream is read once, in order.
    pub fn next(&mut self, cursor: &mut usize, size: u32) -> Next {
        if *cursor < self.base {
            return Next::Error(Errno::ESPIPE);
        }
        if *cursor < self.total() {
            let start = *cursor - self.base;
            let end = start.saturating_add(size as usize).min(self.buf.len());
            *cursor += end - start;
            let out = self.buf[start..end].to_vec();
            self.trim(*cursor);
            return Next::Data(out);
        }
        if let Some(e) = self.err {
            return Next::Error(e);
        }
        if self.done {
            return Next::Data(Vec::new());
        }
        Next::Wait
    }

    /// Drop the read part of the front once it is worth a move.
    fn trim(&mut self, cursor: usize) {
        let read = cursor - self.base;
        if read >= KEEP / 2 {
            self.buf.drain(..read);
            self.base += read;
        }
    }

    /// How much the broker holds for this stream now.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.buf.len()
    }
}
