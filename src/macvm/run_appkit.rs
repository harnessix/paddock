//! One pass of AppKit's events in the session loop of `run.rs`, macOS
//! only.
//!
//! The loop takes AppKit's events from the queue of the app and sends
//! them on, as `NSApplication.run` does. Running the run loop alone draws
//! the windows and runs the main queue, but no click or key ever reaches
//! a window: they stay in the queue of the app, and the app never counts
//! as finished launching.

use objc2_app_kit::{NSApplication, NSEventMask};
use objc2_foundation::{NSDate, NSRunLoopMode};

/// The most AppKit events that one pass sends on, so the session's own
/// events are not starved by a flood of input.
const EVENTS_PER_PASS: usize = 64;

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
