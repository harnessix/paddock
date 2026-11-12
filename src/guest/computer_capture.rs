//! The capture of computer use in the guest, macOS only: ScreenCaptureKit
//! for one image of a region (`SCShareableContent`, `SCContentFilter`,
//! `SCStreamConfiguration`, `SCScreenshotManager`, macOS 14 and later),
//! the digest of its pixels, and its PNG with ImageIO. The classes are
//! looked up at run time, so an older guest gives an error, no crash.

use crate::macvm::computer::{Code, Differ, Failure, Geometry};
use crate::macvm::text;
use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_core_foundation::{CFString, CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSError};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::mpsc;
use std::time::Duration;

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageDestinationCreateWithData(
        data: *mut c_void,
        kind: *const c_void,
        count: usize,
        options: *const c_void,
    ) -> *mut c_void;
    fn CGImageDestinationAddImage(dest: *mut c_void, image: *const c_void, props: *const c_void);
    fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataCreateMutable(alloc: *const c_void, capacity: isize) -> *mut c_void;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGImageGetDataProvider(image: *const c_void) -> *const c_void;
    fn CGDataProviderCopyData(provider: *const c_void) -> *const c_void;
    fn CGImageRetain(image: *const c_void) -> *const c_void;
    fn CGImageRelease(image: *const c_void);
}

/// How long one capture may take.
const CAPTURE_WAIT: Duration = Duration::from_secs(10);

/// The image of a capture, retained.
pub(super) struct Image(pub(super) *const c_void);
// SAFETY: a CGImage is immutable and thread-safe.
unsafe impl Send for Image {}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: we own one retain.
        unsafe { CGImageRelease(self.0) };
    }
}

/// The digest of the pixels of an image, or None without a provider.
pub(super) fn pixel_digest(image: *const c_void) -> Option<[u8; 32]> {
    // SAFETY: a valid image; the Copy rule gives us the data.
    let data = unsafe { CGDataProviderCopyData(CGImageGetDataProvider(image)) };
    if data.is_null() {
        return None;
    }
    // SAFETY: valid data of this length; released after the hash.
    let d = unsafe {
        let len = CFDataGetLength(data).max(0) as usize;
        let ptr = CFDataGetBytePtr(data);
        let d = if ptr.is_null() {
            None
        } else {
            Some(Differ::pixels(std::slice::from_raw_parts(ptr, len)))
        };
        CFRelease(data);
        d
    };
    d
}

/// The class of ScreenCaptureKit, or the unsupported failure.
fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass, Failure> {
    AnyClass::get(name)
        .ok_or_else(|| Failure::new(Code::Unsupported, text::cu_capture_unsupported()))
}

fn ns_error_text(e: *mut NSError) -> String {
    // SAFETY: an error object of the completion handler, or null.
    match unsafe { e.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => String::new(),
    }
}

/// The capture failure of an error: a missing permission says so.
fn capture_failure(e: String) -> Failure {
    // SAFETY: a plain query.
    if !unsafe { CGPreflightScreenCaptureAccess() } {
        return Failure::new(Code::ScreenRecording, text::cu_no_screen_recording());
    }
    Failure::new(Code::Capture, text::cu_capture_failed(&e))
}

/// The shareable content now: the main display and the windows by id.
/// The main display and the windows of the shareable content, by id.
type Content = (Retained<AnyObject>, HashMap<u32, Retained<AnyObject>>);

fn shareable() -> Result<Content, Failure> {
    let cls = class(c"SCShareableContent")?;
    let (tx, rx) = mpsc::channel::<Result<Retained<AnyObject>, String>>();
    let block = RcBlock::new(move |content: *mut AnyObject, err: *mut NSError| {
        // SAFETY: the content of the handler, retained for us.
        let r = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(c),
            None => Err(ns_error_text(err)),
        };
        let _ = tx.send(r);
    });
    // SAFETY: the documented class method with a completion handler.
    unsafe {
        let _: () = msg_send![cls, getShareableContentExcludingDesktopWindows: Bool::NO, onScreenWindowsOnly: Bool::YES, completionHandler: &*block];
    }
    let content = rx
        .recv_timeout(CAPTURE_WAIT)
        .map_err(|_| Failure::new(Code::Timeout, text::cu_capture_failed("timeout")))?
        .map_err(capture_failure)?;
    let main = unsafe { CGMainDisplayID() };
    // SAFETY: documented properties of SCShareableContent.
    let displays: Retained<NSArray<AnyObject>> = unsafe { msg_send![&*content, displays] };
    let mut display = None;
    for d in displays.iter() {
        let id: u32 = unsafe { msg_send![&*d, displayID] };
        if id == main || display.is_none() {
            display = Some(d.clone());
        }
    }
    let display = display
        .ok_or_else(|| Failure::new(Code::Capture, text::cu_capture_failed("no display")))?;
    let windows: Retained<NSArray<AnyObject>> = unsafe { msg_send![&*content, windows] };
    let mut by_id = HashMap::new();
    for w in windows.iter() {
        let id: u32 = unsafe { msg_send![&*w, windowID] };
        by_id.insert(id, w.clone());
    }
    Ok((display, by_id))
}

/// One image of the region of `g`, as a retained CGImage.
pub(super) fn capture_image(
    g: &Geometry,
    only: Option<&[u32]>,
    cursor: bool,
) -> Result<Image, Failure> {
    let (display, windows) = shareable()?;
    let filter_cls = class(c"SCContentFilter")?;
    let config_cls = class(c"SCStreamConfiguration")?;
    let manager = class(c"SCScreenshotManager")?;
    // SAFETY: the documented initializers of SCContentFilter.
    let filter: Retained<AnyObject> = unsafe {
        let alloc: *mut AnyObject = msg_send![filter_cls, alloc];
        match only {
            None => {
                let none = NSArray::<AnyObject>::new();
                let f: *mut AnyObject =
                    msg_send![alloc, initWithDisplay: &*display, excludingWindows: &*none];
                Retained::from_raw(f)
            }
            Some(ids) => {
                let list: Vec<Retained<AnyObject>> = ids
                    .iter()
                    .filter_map(|id| windows.get(id).cloned())
                    .collect();
                if list.is_empty() {
                    return Err(Failure::new(Code::NoWindow, text::cu_no_window()));
                }
                let arr = NSArray::from_retained_slice(&list);
                let f: *mut AnyObject =
                    msg_send![alloc, initWithDisplay: &*display, includingWindows: &*arr];
                Retained::from_raw(f)
            }
        }
    }
    .ok_or_else(|| Failure::new(Code::Capture, text::cu_capture_failed("filter")))?;
    // SAFETY: documented properties of SCStreamConfiguration.
    let config: Retained<AnyObject> = unsafe { msg_send![config_cls, new] };
    unsafe {
        let _: () = msg_send![&*config, setWidth: g.width as usize];
        let _: () = msg_send![&*config, setHeight: g.height as usize];
        let rect = CGRect::new(
            CGPoint::new(g.origin.x, g.origin.y),
            CGSize::new(g.points.w, g.points.h),
        );
        let _: () = msg_send![&*config, setSourceRect: rect];
        let _: () = msg_send![&*config, setShowsCursor: Bool::new(cursor)];
        let _: () = msg_send![&*config, setScalesToFit: Bool::YES];
    }
    let (tx, rx) = mpsc::channel::<Result<Image, String>>();
    let block = RcBlock::new(move |image: *const c_void, err: *mut NSError| {
        let r = if image.is_null() {
            Err(ns_error_text(err))
        } else {
            // SAFETY: the image of the handler lives for the call: retain it.
            Ok(Image(unsafe { CGImageRetain(image) }))
        };
        let _ = tx.send(r);
    });
    // SAFETY: the documented class method (macOS 14) with its handler.
    unsafe {
        let _: () = msg_send![manager, captureImageWithFilter: &*filter, configuration: &*config, completionHandler: &*block];
    }
    rx.recv_timeout(CAPTURE_WAIT)
        .map_err(|_| Failure::new(Code::Timeout, text::cu_capture_failed("timeout")))?
        .map_err(capture_failure)
}

/// The PNG of an image, with ImageIO.
pub(super) fn encode_png(image: *const c_void) -> Option<Vec<u8>> {
    let kind = CFString::from_static_str("public.png");
    // SAFETY: plain Core Foundation and ImageIO calls; every object made
    // here is released here.
    unsafe {
        let data = CFDataCreateMutable(std::ptr::null(), 0);
        if data.is_null() {
            return None;
        }
        let kind_ptr: *const CFString = &*kind;
        let dest = CGImageDestinationCreateWithData(data, kind_ptr.cast(), 1, std::ptr::null());
        if dest.is_null() {
            CFRelease(data);
            return None;
        }
        CGImageDestinationAddImage(dest, image, std::ptr::null());
        let ok = CGImageDestinationFinalize(dest);
        CFRelease(dest);
        let out = ok.then(|| {
            let len = CFDataGetLength(data).max(0) as usize;
            let ptr = CFDataGetBytePtr(data);
            if ptr.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(ptr, len).to_vec()
            }
        });
        CFRelease(data);
        out.filter(|v| !v.is_empty())
    }
}
