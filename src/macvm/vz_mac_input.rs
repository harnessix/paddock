//! Keys into a running macOS guest, for the Type buttons of the guided
//! window and for the password into a locked guest screen during an app
//! session (`run_screen.rs`).
//!
//! `VZVirtualMachineView` takes only events that come from the hardware:
//! an NSEvent made by the app, even one built on a Core Graphics event,
//! never reaches the guest. Virtualization has no public call for it, so
//! paddock uses the private keyboard of the VM: `-[VZVirtualMachine
//! _keyboards]`, whose items answer `sendKeyEvents:` with `_VZKeyEvent`
//! objects made by `initWithType:keyCode:`. Each call is checked with
//! `respondsToSelector:` first; when one is missing the function returns
//! false, and the person types by hand.
//! macOS only. Call them on the queue of the VM (the main queue here).

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSArray;
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineView};

fn responds(obj: &AnyObject, sel: Sel) -> bool {
    let ok: Bool = unsafe { msg_send![obj, respondsToSelector: sel] };
    ok.as_bool()
}

fn vm_of(view: &VZVirtualMachineView) -> Option<Retained<VZVirtualMachine>> {
    unsafe { view.virtualMachine() }
}

/// One key, pressed (`down`) or released, by its macOS virtual key code,
/// to the first keyboard of the guest. A modifier key is a key like any
/// other here: its press and its release. False when the private calls are
/// missing.
pub(super) fn key(view: &VZVirtualMachineView, code: u16, down: bool) -> bool {
    let Some(vm) = vm_of(view) else {
        return false;
    };
    key_vm(&vm, code, down)
}

/// [`key`] on the VM itself, for a session that has no guest view.
pub(super) fn key_vm(vm: &VZVirtualMachine, code: u16, down: bool) -> bool {
    let vm: &AnyObject = vm;
    if !responds(vm, sel!(_keyboards)) {
        return false;
    }
    let Some(cls) = AnyClass::get(c"_VZKeyEvent") else {
        return false;
    };
    // `_VZKeyEvent` type: 0 a press, 1 a release (measured on macOS 27).
    let kind: isize = if down { 0 } else { 1 };
    // SAFETY: the selectors were checked above; the types are the ones of
    // the Objective-C runtime of Virtualization (`initWithType:keyCode:` is
    // `@@:qS`, `sendKeyEvents:` is `v@:@`).
    unsafe {
        let keyboards: Option<Retained<NSArray<AnyObject>>> = msg_send![vm, _keyboards];
        let Some(kb) = keyboards.and_then(|k| k.firstObject()) else {
            return false;
        };
        if !responds(&kb, sel!(sendKeyEvents:)) {
            return false;
        }
        let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
        let ev: Option<Retained<AnyObject>> = msg_send![alloc, initWithType: kind, keyCode: code];
        let Some(ev) = ev else {
            return false;
        };
        let list = NSArray::from_retained_slice(&[ev]);
        let _: () = msg_send![&*kb, sendKeyEvents: &*list];
    }
    true
}
