//! The guest screen lock, macOS only: the lock state of the session
//! (`CGSessionCopyCurrentDictionary`, key `CGSSessionScreenIsLocked`), the
//! seconds since the last input, and the lock of `bench.lock` through the
//! private `SACLockScreenImmediate` of `login.framework`, looked up at run
//! time so a macOS without it gives an error and no crash.

use super::platform::Fail;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{ns_string, NSDictionary, NSNumber, NSString};
use std::ffi::c_void;

// <CoreGraphics/CGSession.h> and <CoreGraphics/CGEventSource.h>:
// CGEventSourceStateID is int32_t, CGEventType is uint32_t.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// `kCGEventSourceStateCombinedSessionState`.
const COMBINED_SESSION: i32 = 0;
/// `kCGAnyInputEventType`.
const ANY_INPUT: u32 = !0;

/// The private framework and the call of the lock.
const LOGIN_FRAMEWORK: &std::ffi::CStr =
    c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login";
const LOCK_CALL: &std::ffi::CStr = c"SACLockScreenImmediate";

/// The screen of this session is locked now. No dictionary (no window
/// server session) counts as unlocked.
pub fn locked() -> bool {
    // SAFETY: no arguments; the result is a CFDictionary we own, or null.
    let d = unsafe { CGSessionCopyCurrentDictionary() };
    if d.is_null() {
        return false;
    }
    // SAFETY: the Copy rule: we own it; CFDictionary is toll-free
    // bridged to NSDictionary.
    let Some(dict) = (unsafe { Retained::from_raw(d as *mut NSDictionary<NSString, AnyObject>) })
    else {
        return false;
    };
    dict.objectForKey(ns_string!("CGSSessionScreenIsLocked"))
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .is_some_and(|n| n.boolValue())
}

/// The whole seconds since the last keyboard or mouse input of the
/// session.
pub fn idle_secs() -> Option<u64> {
    // SAFETY: plain values.
    let s = unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION, ANY_INPUT) };
    (s.is_finite() && s >= 0.0).then_some(s as u64)
}

/// Lock the screen now, as the lock item of the Apple menu does.
pub fn lock_now() -> Result<(), Fail> {
    // SAFETY: a NUL-terminated path; the handle stays open for the life
    // of the process, as a framework does.
    let handle = unsafe { libc::dlopen(LOGIN_FRAMEWORK.as_ptr(), libc::RTLD_LAZY) };
    if handle.is_null() {
        return Err((
            ErrorCode::Unsupported,
            text::screen_lock_missing("login.framework"),
        ));
    }
    // SAFETY: a NUL-terminated symbol name in an open handle.
    let sym = unsafe { libc::dlsym(handle, LOCK_CALL.as_ptr()) };
    if sym.is_null() {
        return Err((
            ErrorCode::Unsupported,
            text::screen_lock_missing("SACLockScreenImmediate"),
        ));
    }
    // SAFETY: `int SACLockScreenImmediate(void)` of login.framework.
    let lock: extern "C" fn() -> i32 = unsafe { std::mem::transmute(sym) };
    let r = lock();
    if r != 0 {
        log::warn!("SACLockScreenImmediate returned {r}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "screen_macos_tests.rs"]
mod tests;
