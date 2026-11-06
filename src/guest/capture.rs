//! The corner radius of a guest window from its image, macOS only: one
//! `CGWindowListCreateImage` of the window alone, without its shadow
//! (`kCGWindowImageBoundsIgnoreFraming`), at the nominal resolution, then
//! the alpha of the four corner patches into the fit of `chrome.rs`.
//!
//! The capture needs Screen Recording: the setup sets the agent's row
//! offline (`macvm/tcc.rs`). Without it macOS gives an image without the
//! window, the fit finds no radius, and the host keeps the radius of the
//! guest's macOS version. The call is deprecated for ScreenCaptureKit,
//! which has no synchronous call for one window; it still works for a
//! process with the permission.

use super::chrome::{self, Alpha};
use super::windows::RawWindow;
use objc2::AllocAnyThread;
use objc2_app_kit::NSBitmapImageRep;
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{CGImage, CGWindowImageOption, CGWindowListOption};

extern "C" {
    /// `CGRectNull`: the bounds of the window itself.
    static CGRectNull: CGRect;
}

/// The side of the corner patch that is read, in pixels.
const PATCH: usize = 96;

/// The corner radius of window `w` in points, or None.
pub fn radius(w: &RawWindow) -> Option<f64> {
    if w.bounds.w < 8.0 || w.bounds.h < 8.0 {
        return None;
    }
    // SAFETY: a constant of CoreGraphics.
    let null = unsafe { CGRectNull };
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGWindowListCreateImage(
        null,
        CGWindowListOption::OptionIncludingWindow,
        w.id,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::NominalResolution,
    )?;
    let iw = CGImage::width(Some(&image));
    let ih = CGImage::height(Some(&image));
    if iw < 8 || ih < 8 {
        return None;
    }
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
    let (pw, ph) = (rep.pixelsWide(), rep.pixelsHigh());
    if pw <= 0 || ph <= 0 {
        return None;
    }
    let (pw, ph) = (pw as usize, ph as usize);
    // Only the corner patches are read; the rest counts as opaque.
    let mut a = vec![1f32; pw * ph];
    let n = PATCH.min(pw).min(ph);
    let xs: Vec<usize> = (0..n).chain(pw - n..pw).collect();
    let ys: Vec<usize> = (0..n).chain(ph - n..ph).collect();
    for &y in &ys {
        for &x in &xs {
            let alpha = rep
                .colorAtX_y(x as isize, y as isize)
                .map_or(0.0, |c| c.alphaComponent());
            a[y * pw + x] = alpha as f32;
        }
    }
    let scale = pw as f64 / w.bounds.w;
    let r = chrome::radius_points(
        &Alpha {
            w: pw,
            h: ph,
            a: &a,
        },
        scale,
    );
    log::debug!(
        "window {}: corner radius {:?} at {scale:.2} px per point",
        w.id,
        r
    );
    r
}
