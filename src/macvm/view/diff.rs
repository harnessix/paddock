//! Draw only what changed. The guest sends a frame only when its
//! picture changes, but the view checks again: a frame with an old
//! sequence number, or with the same pixels and the same place as the
//! frame on the screen, is not drawn. The numbers count within one
//! subscription: a new one starts at 1 again ([`Gate::new_stream`]). A resize, the menu or Ctrl-L asks
//! for one redraw of the last frame.

use super::fit::Placement;

/// What the gate knows about the screen.
#[derive(Debug, Default)]
pub struct Gate {
    shown: Option<(blake3::Hash, Placement)>,
    last_seq: Option<u64>,
    dirty: bool,
    /// Frames that were not drawn because nothing changed or they were old.
    pub skipped: u64,
}

/// Why a frame is drawn or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Draw,
    /// The same pixels at the same place.
    Same,
    /// A sequence number at or below one already seen.
    Old,
}

impl Gate {
    /// Ask for a redraw of the next frame, even when it is the same.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Decide on frame `seq` with bytes `png` at `place`. A `Draw` counts
    /// as shown.
    pub fn check(&mut self, seq: u64, png: &[u8], place: &Placement) -> Verdict {
        if self.last_seq.is_some_and(|s| seq <= s) && !self.dirty {
            self.skipped += 1;
            return Verdict::Old;
        }
        self.last_seq = Some(self.last_seq.map_or(seq, |s| s.max(seq)));
        let hash = blake3::hash(png);
        if !self.dirty && self.shown.as_ref() == Some(&(hash, *place)) {
            self.skipped += 1;
            return Verdict::Same;
        }
        self.dirty = false;
        self.shown = Some((hash, *place));
        Verdict::Draw
    }

    /// A new subscription: its `seq` starts at 1 again, so the old
    /// numbers say nothing about its frames. The picture on the screen
    /// stays, so a first frame with the same pixels is still `Same`.
    pub fn new_stream(&mut self) {
        self.last_seq = None;
    }

    /// The screen was cleared: nothing is shown any more.
    pub fn cleared(&mut self) {
        self.shown = None;
        self.dirty = true;
    }
}

/// Keep only the newest of the frames that waited: a slow terminal draws
/// the present, not a backlog. Returns the newest and how many it drops.
pub fn newest<T>(mut waiting: Vec<T>, seq: impl Fn(&T) -> u64) -> Option<(T, usize)> {
    if waiting.is_empty() {
        return None;
    }
    let mut best = 0;
    for (i, f) in waiting.iter().enumerate() {
        if seq(f) >= seq(&waiting[best]) {
            best = i;
        }
    }
    let n = waiting.len() - 1;
    Some((waiting.swap_remove(best), n))
}
