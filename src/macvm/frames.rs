//! Where the pixels of a host window come from.
//!
//! Two `VZVirtualMachineView` instances on one VM both show the live
//! display, so [`Source::Viewport`] is the source of this build: `windows.rs` puts
//! one full-display view into each host window at a negative origin
//! (`crop::view_origin`), and the framework composites the guest display
//! into it with no pixel copy. [`Stream`], the per-window ScreenCaptureKit
//! stream, stays a stub behind the same trait.

use super::text;

/// The two ways to get the pixels of a guest window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// One `VZVirtualMachineView` per host window, cropped by its place.
    Viewport,
    /// A ScreenCaptureKit stream per window from the guest, not built.
    Stream,
}

/// The source of this build.
pub const CHOSEN: Source = Source::Viewport;

/// A frame source: it puts the pixels of guest window `id` into the host
/// window `W` and takes them out again.
pub trait Frames<W> {
    fn source(&self) -> Source;
    /// Show guest window `id` in `window`.
    fn attach(&self, id: u32, window: &W) -> Result<(), String>;
    /// Stop showing guest window `id`.
    fn detach(&self, id: u32, window: &W);
}

/// The stream source, a stub: every attach fails with a line that says
/// the viewport windows are used instead.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stream;

impl<W> Frames<W> for Stream {
    fn source(&self) -> Source {
        Source::Stream
    }

    fn attach(&self, _id: u32, _window: &W) -> Result<(), String> {
        Err(text::stream_not_built())
    }

    fn detach(&self, _id: u32, _window: &W) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewport_is_chosen_and_the_stream_is_a_stub() {
        assert_eq!(CHOSEN, Source::Viewport);
        let s = Stream;
        assert_eq!(Frames::<()>::source(&s), Source::Stream);
        let err = Frames::<()>::attach(&s, 1, &()).unwrap_err();
        assert!(err.contains("viewport"), "{err}");
        Frames::<()>::detach(&s, 1, &());
    }
}
