//! The macOS side of computer use in the guest: the listener of the
//! `guest-computer` port, and [`MacScreen`], the [`Screen`] of
//! `computer.rs` with ScreenCaptureKit, CGEvent and Accessibility.
//!
//! - The capture (`computer_capture.rs`): an `SCContentFilter` of the
//!   display (desktop scope) or of the app's windows (app scope), the
//!   region and the pixel size, one image of `SCScreenshotManager`. The
//!   digest is over the pixels; ImageIO encodes the PNG only for a frame
//!   that goes out.
//! - The input: CGEvents from an HID system state source, posted to the
//!   HID event tap, so the session sees them as a person's input. The
//!   Accessibility permission of the golden setup allows it.
//! - The snapshot: the Accessibility tree of each window, depth first,
//!   within the depth, the node limit and a time budget, every element
//!   with the 1 s messaging timeout of `ax.rs`.
//!
//! Every call runs on a thread of computer use, never on the main thread
//! of the agent, except the read of the keyboard layout: the TIS calls
//! go to the main queue (`main_call`).

use super::ax::{self, ax_error, bool_attr, frame_of, string_attr, Ax};
use super::computer::{AxWindow, Screen, Server, Shot};
use super::computer_capture::{capture_image, encode_png, pixel_digest, Image};
use super::menu::{children, text_attr};
use super::windows::RawWindow;
use super::HOST_CID;
use crate::macvm::computer::Button;
use crate::macvm::computer::{element_id, Code, Element, Failure, Geometry};
use crate::macvm::computer_input::Low;
use crate::macvm::proto::Display;
use crate::macvm::text;
use dispatch2::DispatchQueue;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

// <CoreGraphics/CGDirectDisplay.h>, <CGEvent.h>, <CGImage.h>. The event
// and image types are plain pointers here: this file owns their releases.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGEventSourceCreate(state: i32) -> *mut c_void;
    fn CGEventCreateMouseEvent(
        source: *mut c_void,
        kind: u32,
        at: CGPoint,
        button: u32,
    ) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, code: u16, down: bool) -> *mut c_void;
    fn CGEventCreateScrollWheelEvent2(
        source: *mut c_void,
        units: u32,
        count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> *mut c_void;
    fn CGEventKeyboardSetUnicodeString(event: *mut c_void, len: usize, chars: *const u16);
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventSetType(event: *mut c_void, kind: u32);
    fn CGEventPost(tap: u32, event: *mut c_void);
}

/// `kCGEventSourceStateHIDSystemState`.
const HID_STATE: i32 = 1;
/// `kCGHIDEventTap`.
const HID_TAP: u32 = 0;
/// `kCGMouseEventClickState`, `kCGMouseEventButtonNumber`.
const FIELD_CLICK_STATE: u32 = 1;
const FIELD_BUTTON: u32 = 3;
/// `kCGScrollEventUnitLine`.
const UNIT_LINE: u32 = 1;
/// `kCGEventFlagsChanged`.
const FLAGS_CHANGED: u32 = 12;
/// `VMADDR_CID_ANY`.
const CID_ANY: u32 = u32::MAX;
/// The longest wait for the main thread to read the keyboard layout.
const LAYOUT_WAIT: Duration = Duration::from_secs(2);
/// The time budget of one accessibility snapshot.
const AX_BUDGET: Duration = Duration::from_secs(12);

/// The mouse event types of a button: down, up, dragged.
fn mouse_types(b: Button) -> (u32, u32, u32, u32) {
    // (down, up, dragged, CGMouseButton)
    match b {
        Button::Left => (1, 2, 6, 0),
        Button::Right => (3, 4, 7, 1),
        Button::Middle => (25, 26, 27, 2),
    }
}

/// The guest system of computer use.
pub struct MacScreen {
    source: *mut c_void,
    bundles: HashMap<u32, Option<String>>,
}

// SAFETY: the event source is used under the server's mutex only.
unsafe impl Send for MacScreen {}

impl MacScreen {
    pub fn new() -> Self {
        // SAFETY: a plain call; the source lives as long as the screen.
        let source = unsafe { CGEventSourceCreate(HID_STATE) };
        MacScreen {
            source,
            bundles: HashMap::new(),
        }
    }

    fn bundle(&mut self, pid: u32) -> Option<String> {
        self.bundles
            .entry(pid)
            .or_insert_with(|| {
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
                    .and_then(|a| a.bundleIdentifier())
                    .map(|s| s.to_string())
            })
            .clone()
    }

    /// Post one event and release it.
    fn send(&self, ev: *mut c_void) -> Result<(), Failure> {
        if ev.is_null() {
            return Err(Failure::new(Code::Input, text::cu_post_failed()));
        }
        // SAFETY: a valid event that we own.
        unsafe {
            CGEventPost(HID_TAP, ev);
            CFRelease(ev);
        }
        Ok(())
    }

    fn mouse(
        &self,
        kind: u32,
        at: crate::macvm::proto::Point,
        button: u32,
        clicks: u32,
        flags: u64,
    ) -> Result<(), Failure> {
        // SAFETY: plain calls on the event that we own until `send`.
        let ev = unsafe {
            let ev = CGEventCreateMouseEvent(self.source, kind, CGPoint::new(at.x, at.y), button);
            if !ev.is_null() {
                CGEventSetIntegerValueField(ev, FIELD_CLICK_STATE, i64::from(clicks.max(1)));
                CGEventSetIntegerValueField(ev, FIELD_BUTTON, i64::from(button));
                CGEventSetFlags(ev, flags);
            }
            ev
        };
        self.send(ev)
    }

    fn key(&self, code: u16, down: bool, flags: u64, chars: Option<char>) -> Result<(), Failure> {
        // SAFETY: plain calls on the event that we own until `send`.
        let ev = unsafe {
            let ev = CGEventCreateKeyboardEvent(self.source, code, down);
            if !ev.is_null() {
                CGEventSetFlags(ev, flags);
                if let Some(c) = chars {
                    let mut buf = [0u16; 2];
                    let units = c.encode_utf16(&mut buf);
                    CGEventKeyboardSetUnicodeString(ev, units.len(), units.as_ptr());
                }
            }
            ev
        };
        self.send(ev)
    }
}

impl Default for MacScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MacScreen {
    fn drop(&mut self) {
        if !self.source.is_null() {
            // SAFETY: we own the source.
            unsafe { CFRelease(self.source) };
        }
    }
}

/// The elements of one window, depth first.
struct Walk {
    out: Vec<Element>,
    left: usize,
    depth: u32,
    end: Instant,
    cut: bool,
}

impl Walk {
    fn element(
        &mut self,
        el: &AXUIElement,
        window: u32,
        path: &mut Vec<usize>,
        level: u32,
    ) -> Result<(), AXError> {
        if self.left == 0 || Instant::now() >= self.end {
            self.cut = true;
            return Ok(());
        }
        self.left -= 1;
        let role = text_attr(el, "AXRole")?;
        let mut label = text_attr(el, "AXTitle")?;
        if label.is_empty() {
            label = text_attr(el, "AXDescription")?;
        }
        if label.is_empty() {
            label = string_attr(el, "AXHelp").unwrap_or_default();
        }
        let value = string_attr(el, "AXValue").or_else(|| {
            bool_attr(el, "AXValue").map(|b| if b { "1".to_string() } else { "0".to_string() })
        });
        self.out.push(Element {
            id: element_id(window, path),
            role,
            subrole: string_attr(el, "AXSubrole").unwrap_or_default(),
            label,
            value,
            frame: frame_of(el).unwrap_or_default(),
            enabled: bool_attr(el, "AXEnabled").unwrap_or(true),
            focused: bool_attr(el, "AXFocused").unwrap_or(false),
            depth: level,
        });
        if level >= self.depth {
            return Ok(());
        }
        for (i, child) in children(el)?.iter().enumerate() {
            path.push(i);
            let r = self.element(child, window, path, level + 1);
            path.pop();
            match r {
                Ok(()) => {}
                // One hung subtree: skip it, keep the rest.
                Err(AXError::CannotComplete) => self.cut = true,
                Err(e) => return Err(e),
            }
            if self.cut && (self.left == 0 || Instant::now() >= self.end) {
                break;
            }
        }
        Ok(())
    }
}

impl Screen for MacScreen {
    fn display(&mut self) -> (Display, f64) {
        // SAFETY: plain display queries; the mode is released.
        unsafe {
            let id = CGMainDisplayID();
            let b = CGDisplayBounds(id);
            let mode = CGDisplayCopyDisplayMode(id);
            let scale = if mode.is_null() {
                1.0
            } else {
                let px = CGDisplayModeGetPixelWidth(mode) as f64;
                let pt = CGDisplayModeGetWidth(mode) as f64;
                CGDisplayModeRelease(mode);
                if pt > 0.0 {
                    px / pt
                } else {
                    1.0
                }
            };
            (
                Display {
                    w: b.size.width.max(0.0) as u32,
                    h: b.size.height.max(0.0) as u32,
                },
                scale,
            )
        }
    }

    fn permissions(&mut self) -> (bool, bool) {
        // SAFETY: a plain query.
        (
            unsafe { CGPreflightScreenCaptureAccess() },
            ax::trusted_now(),
        )
    }

    fn on_screen(&mut self) -> Vec<RawWindow> {
        super::macos::window_list(&mut |pid| self.bundle(pid))
    }

    fn locked(&mut self) -> bool {
        super::screen::locked()
    }

    fn front_pid(&mut self) -> Option<u32> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier() as u32)
    }

    fn front_app(&mut self) -> Option<String> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|a| a.bundleIdentifier())
            .map(|s| s.to_string())
    }

    fn capture(
        &mut self,
        g: &Geometry,
        windows: Option<&[u32]>,
        cursor: bool,
    ) -> Result<Shot, Failure> {
        let image = capture_image(g, windows, cursor)?;
        let digest = pixel_digest(image.0)
            .ok_or_else(|| Failure::new(Code::Capture, text::cu_capture_failed("no pixels")))?;
        Ok(Shot {
            width: g.width,
            height: g.height,
            digest,
            image: Box::new(image),
        })
    }

    fn png(&mut self, shot: &Shot) -> Result<Vec<u8>, Failure> {
        let image = shot
            .image
            .downcast_ref::<Image>()
            .ok_or_else(|| Failure::new(Code::Internal, text::cu_png_failed()))?;
        encode_png(image.0).ok_or_else(|| Failure::new(Code::Capture, text::cu_png_failed()))
    }

    fn post(&mut self, ev: &Low) -> Result<(), Failure> {
        match ev {
            Low::Move(p) => self.mouse(5, *p, 0, 0, 0),
            Low::Down {
                at,
                button,
                clicks,
                flags,
            } => {
                let (down, _, _, b) = mouse_types(*button);
                self.mouse(down, *at, b, *clicks, *flags)
            }
            Low::Up {
                at,
                button,
                clicks,
                flags,
            } => {
                let (_, up, _, b) = mouse_types(*button);
                self.mouse(up, *at, b, *clicks, *flags)
            }
            Low::Drag { at, button, flags } => {
                let (_, _, drag, b) = mouse_types(*button);
                self.mouse(drag, *at, b, 1, *flags)
            }
            Low::Scroll { x, y } => {
                // SAFETY: a plain call; the event goes to `send`.
                let ev =
                    unsafe { CGEventCreateScrollWheelEvent2(self.source, UNIT_LINE, 2, *y, *x, 0) };
                self.send(ev)
            }
            Low::Key {
                code,
                down,
                flags,
                chars,
            } => self.key(*code, *down, *flags, *chars),
            Low::Modifier { code, down, flags } => {
                // SAFETY: plain calls on the event that we own until `send`.
                let ev = unsafe {
                    let ev = CGEventCreateKeyboardEvent(self.source, *code, *down);
                    if !ev.is_null() {
                        CGEventSetType(ev, FLAGS_CHANGED);
                        CGEventSetFlags(ev, *flags);
                    }
                    ev
                };
                self.send(ev)
            }
            Low::Text(t) => {
                let units: Vec<u16> = t.encode_utf16().collect();
                for down in [true, false] {
                    // SAFETY: plain calls on the event that we own until `send`.
                    let ev = unsafe {
                        let ev = CGEventCreateKeyboardEvent(self.source, 0, down);
                        if !ev.is_null() {
                            CGEventSetFlags(ev, 0);
                            CGEventKeyboardSetUnicodeString(ev, units.len(), units.as_ptr());
                        }
                        ev
                    };
                    self.send(ev)?;
                }
                Ok(())
            }
            Low::Wait(ms) => {
                std::thread::sleep(Duration::from_millis(u64::from(*ms)));
                Ok(())
            }
        }
    }

    /// The TIS calls must run on the main thread (macOS 14 asserts it),
    /// and this runs on a thread of computer use: the main queue reads the
    /// layout. A main thread that is busy past [`LAYOUT_WAIT`] gives None,
    /// and then the Help keys of the common layouts count.
    fn key_char(&mut self, code: u16, shift: bool) -> Option<char> {
        super::main_call::run(
            objc2::MainThreadMarker::new().is_some(),
            |job| DispatchQueue::main().exec_async(job),
            move || crate::macvm::tis::key_char(code, shift),
            LAYOUT_WAIT,
        )
        .flatten()
    }

    fn activate(&mut self, pid: u32) -> Result<(), Failure> {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
            .ok_or_else(|| Failure::new(Code::NoApp, text::cu_no_app()))?;
        if app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
            Ok(())
        } else {
            Err(Failure::new(Code::NotFront, text::cu_activate_failed(pid)))
        }
    }

    fn ax(
        &mut self,
        windows: &[AxWindow],
        depth: u32,
        max_nodes: u32,
    ) -> Result<(Vec<Element>, bool), Failure> {
        let mut a = Ax::new();
        if !a.trusted() {
            return Err(Failure::new(
                Code::Accessibility,
                text::cu_no_accessibility(),
            ));
        }
        let mut walk = Walk {
            out: Vec::new(),
            left: max_nodes as usize,
            depth,
            end: Instant::now() + AX_BUDGET,
            cut: false,
        };
        for w in windows {
            let Some(el) = a.element(w.pid, w.id, w.frame) else {
                continue;
            };
            match walk.element(&el, w.id, &mut Vec::new(), 0) {
                Ok(()) => {}
                Err(AXError::CannotComplete) => walk.cut = true,
                Err(e) => return Err(ax_failure(ax_error(e, "snapshot"))),
            }
            if walk.left == 0 || Instant::now() >= walk.end {
                walk.cut = true;
                break;
            }
        }
        Ok((walk.out, walk.cut))
    }

    fn ax_press(
        &mut self,
        window: AxWindow,
        path: &[usize],
        role: Option<&str>,
    ) -> Result<(String, String), Failure> {
        let id = element_id(window.id, path);
        let mut a = Ax::new();
        if !a.trusted() {
            return Err(Failure::new(
                Code::Accessibility,
                text::cu_no_accessibility(),
            ));
        }
        let mut el: CFRetained<AXUIElement> = a
            .element(window.pid, window.id, window.frame)
            .ok_or_else(|| {
                Failure::new(Code::NoElement, text::cu_element_window_gone(window.id))
            })?;
        for i in path {
            el = children(&el)
                .map_err(|e| ax_failure(ax_error(e, "press")))?
                .get(*i)
                .cloned()
                .ok_or_else(|| Failure::new(Code::NoElement, text::cu_element_gone(&id)))?;
        }
        let now = text_attr(&el, "AXRole").map_err(|e| ax_failure(ax_error(e, "press")))?;
        if let Some(want) = role {
            if want != now {
                return Err(Failure::new(
                    Code::NoElement,
                    text::cu_element_role(&id, want, &now),
                ));
            }
        }
        let mut label = text_attr(&el, "AXTitle").unwrap_or_default();
        if label.is_empty() {
            label = text_attr(&el, "AXDescription").unwrap_or_default();
        }
        // SAFETY: a valid action name on a valid element.
        match unsafe { el.perform_action(&CFString::from_static_str("AXPress")) } {
            AXError::Success => Ok((now, label)),
            AXError::CannotComplete => Err(Failure::new(Code::Timeout, text::cu_ax_timeout())),
            e => Err(Failure::new(
                Code::NoElement,
                text::cu_press_failed(&id, e.0),
            )),
        }
    }
}

/// The computer-use failure of an Accessibility failure of `ax.rs`.
fn ax_failure(f: super::platform::Fail) -> Failure {
    use crate::macvm::proto::ErrorCode;
    match f.0 {
        ErrorCode::Timeout => Failure::new(Code::Timeout, text::cu_ax_timeout()),
        ErrorCode::Accessibility => Failure::new(Code::Accessibility, text::cu_no_accessibility()),
        _ => Failure::new(Code::Internal, f.1),
    }
}

/// Mark the agent's Screen Recording as approved for the system's
/// reminder of screen capture (the `replayd` approvals of the user), so
/// macOS does not ask again in the guest, where nobody answers. An
/// approval file that macOS ignores changes nothing.
pub fn approve_capture() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let file = std::path::Path::new(&home).join(APPROVALS);
    let r = std::process::Command::new("/usr/bin/defaults")
        .arg("write")
        .arg(&file)
        .arg(crate::macvm::tcc::CLIENT)
        .args(["-date", APPROVED_UNTIL])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match r {
        Ok(s) if s.success() => log::info!("computer: screen capture approval written"),
        Ok(s) => log::info!("computer: screen capture approval not written: {s}"),
        Err(e) => log::info!("computer: screen capture approval not written: {e}"),
    }
}

/// The approvals of the screen capture reminder, under the home.
const APPROVALS: &str = "Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals";
/// The date of the approval: far after any session.
const APPROVED_UNTIL: &str = "3024-01-01 00:00:00 +0000";

/// Listen on the computer-use port and serve each connection of the host
/// in its own thread.
pub fn start() {
    let server = Server::new(MacScreen::new(), Arc::new(super::computer::scene));
    let _ = std::thread::Builder::new()
        .name("computer-listen".into())
        .spawn(move || loop {
            match listen(crate::vm::ports::GUEST_COMPUTER) {
                Ok(fd) => accept_loop(fd, &server),
                Err(e) => log::warn!(
                    "computer: cannot listen on vsock port {}: {e}",
                    crate::vm::ports::GUEST_COMPUTER
                ),
            }
            std::thread::sleep(Duration::from_secs(5));
        });
}

/// A listening vsock socket on `port` for any CID.
fn listen(port: u32) -> std::io::Result<i32> {
    // SAFETY: plain socket calls on our own descriptor.
    unsafe {
        let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        let mut addr: libc::sockaddr_vm = std::mem::zeroed();
        addr.svm_len = std::mem::size_of::<libc::sockaddr_vm>() as u8;
        addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
        addr.svm_port = port;
        addr.svm_cid = CID_ANY;
        let len = std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t;
        if libc::bind(fd, (&addr as *const libc::sockaddr_vm).cast(), len) != 0
            || libc::listen(fd, 8) != 0
        {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        Ok(fd)
    }
}

/// Accept until the socket fails. A peer other than the host is closed.
fn accept_loop(fd: i32, server: &Server<MacScreen>) {
    loop {
        // SAFETY: an out address of the right size.
        let (conn, cid) = unsafe {
            let mut addr: libc::sockaddr_vm = std::mem::zeroed();
            let mut len = std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t;
            let c = libc::accept(fd, (&mut addr as *mut libc::sockaddr_vm).cast(), &mut len);
            (c, addr.svm_cid)
        };
        if conn < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            log::warn!("computer: accept failed: {e}");
            // SAFETY: our descriptor.
            unsafe { libc::close(fd) };
            return;
        }
        // SAFETY: the accepted descriptor is ours.
        let stream = unsafe { UnixStream::from_raw_fd(conn) };
        unsafe { libc::fcntl(conn, libc::F_SETFD, libc::FD_CLOEXEC) };
        if cid != HOST_CID {
            log::warn!("computer: refused a connection from CID {cid}");
            continue;
        }
        let s = server.clone();
        let _ = std::thread::Builder::new()
            .name("computer-conn".into())
            .spawn(move || super::computer::serve(s, stream));
    }
}
