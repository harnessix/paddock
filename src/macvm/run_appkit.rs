//! One pass of AppKit's events in the session loop of `run.rs`, macOS
//! only.
//!
//! The loop takes AppKit's events from the queue of the app and sends
//! them on, as `NSApplication.run` does. Running the run loop alone draws
//! the windows and runs the main queue, but no click or key ever reaches
//! a window: they stay in the queue of the app, and the app never counts
//! as finished launching.

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSDate, NSPoint, NSRunLoopMode};
use std::sync::atomic::{AtomicBool, Ordering};

/// The most AppKit events that one pass sends on, so the session's own
/// events are not starved by a flood of input.
const EVENTS_PER_PASS: usize = 64;

/// A wake event is on its way to the main loop.
static WAKE_POSTED: AtomicBool = AtomicBool::new(false);

/// The subtype of the wake event, so nothing mistakes it for another one.
const WAKE_SUBTYPE: i16 = 0x5044;

/// From any thread: end the main loop's wait for AppKit events now, so an
/// event of the session (a reply of the agent, a window event) is handled
/// at once and not after up to one tick. At most one wake is on its way.
pub fn wake() {
    if WAKE_POSTED.swap(true, Ordering::AcqRel) {
        return;
    }
    DispatchQueue::main().exec_async(|| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            WAKE_SUBTYPE,
            0,
            0,
        );
        match event {
            Some(e) => NSApplication::sharedApplication(mtm).postEvent_atStart(&e, false),
            None => WAKE_POSTED.store(false, Ordering::Release),
        }
    });
}

/// The main loop takes the session's events next: a later event wakes it
/// again. Call it before the session reads its channel.
pub fn woken() {
    WAKE_POSTED.store(false, Ordering::Release);
}

/// Wait until `until` for the first event, which runs the run loop (the
/// timers, the main queue, the drawing), send it on, then the events
/// already queued. The number of events sent.
pub fn pass(app: &NSApplication, until: &NSDate, mode: &NSRunLoopMode) -> usize {
    let mut next = app.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        Some(until),
        mode,
        true,
    );
    let mut sent = 0;
    while let Some(event) = next {
        app.sendEvent(&event);
        sent += 1;
        if sent >= EVENTS_PER_PASS {
            break;
        }
        next = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&NSDate::distantPast()),
            mode,
            true,
        );
    }
    if sent > 0 {
        app.updateWindows();
    }
    sent
}
