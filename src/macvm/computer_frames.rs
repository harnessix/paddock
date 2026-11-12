//! The frame stream of computer use (`computer.rs` has the contract): the
//! pause between two captures, and the change detection that sends a
//! frame only when its picture changed.

use super::computer::{Geometry, FPS_MAX, FPS_MIN};
use std::time::Duration;

/// The pause between two captures of a subscription.
pub fn frame_interval(max_fps: f64) -> Duration {
    let fps = if max_fps.is_finite() {
        max_fps.clamp(FPS_MIN, FPS_MAX)
    } else {
        FPS_MAX
    };
    Duration::from_secs_f64(1.0 / fps)
}

/// The change detection of a frame stream: the digest of the last frame
/// sent. A frame goes out when its pixels or its geometry differ.
#[derive(Debug, Default, Clone)]
pub struct Differ {
    last: Option<[u8; 32]>,
    seq: u64,
}

impl Differ {
    /// The digest of the pixels of a frame.
    pub fn pixels(pixels: &[u8]) -> [u8; 32] {
        *blake3::hash(pixels).as_bytes()
    }

    /// The digest of a frame: the digest of its pixels and its geometry,
    /// so a window that moved over the same pixels is a new frame.
    pub fn digest(pixels: [u8; 32], g: &Geometry) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(&pixels);
        h.update(&g.width.to_le_bytes());
        h.update(&g.height.to_le_bytes());
        for v in [g.origin.x, g.origin.y, g.points.w, g.points.h] {
            h.update(&v.to_le_bytes());
        }
        *h.finalize().as_bytes()
    }

    /// True when a frame with this digest is not the one sent last.
    pub fn changed(&self, digest: [u8; 32]) -> bool {
        self.last != Some(digest)
    }

    /// The sequence number for a frame with this digest, or None when the
    /// picture is the one sent last.
    pub fn next(&mut self, digest: [u8; 32]) -> Option<u64> {
        if self.last == Some(digest) {
            return None;
        }
        self.last = Some(digest);
        self.seq += 1;
        Some(self.seq)
    }

    /// The last sequence number given, 0 before the first.
    pub fn last_seq(&self) -> u64 {
        self.seq
    }
}
