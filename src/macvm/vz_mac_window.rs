//! The guided window of `paddock macvm setup`: one `VZVirtualMachineView`
//! of the golden on the left, and a side panel with the six setup steps
//! on the right (`vz_mac_panel.rs`). macOS only, main thread only; the
//! setup driver on its worker thread reaches it through the main dispatch
//! queue.
//!
//! The window can be resized. Its first frame fits inside the visible
//! frame of the main screen, so it is never under the Dock or the menu
//! bar, and the guest view keeps the aspect of the guest display in the
//! room left of the panel. `guided_ui.rs` computes both, with Linux tests.

use super::guided_ui::{self, Rect};
use super::text;
use super::vz_mac::{Sendable, DISPLAY};
use super::vz_mac_panel::Panel;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSScreen, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineView};
use std::sync::mpsc::Sender;

/// The width over the height of the guest display.
pub const ASPECT: f64 = DISPLAY.w as f64 / DISPLAY.h as f64;

/// The visible frame of a screen when AppKit names no main screen: a 13
/// inch laptop with the Dock at the bottom.
const FALLBACK_VISIBLE: Rect = Rect::new(0.0, 70.0, 1440.0, 805.0);

/// What the panel sends to the setup driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelEvent {
    Continue,
    Stop,
    Closed,
    /// The password that the person set, typed into the secure field
    /// after the guest refused the one of the keyring.
    Password(String),
}

/// The handle that the worker thread keeps. Every method goes through the
/// main queue.
pub struct Window {
    window: Sendable<Retained<NSWindow>>,
    panel: Sendable<Retained<Panel>>,
}

pub(super) fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

fn rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

impl Window {
    /// Make and show the window. Call it on the main thread.
    pub fn open(vm: &VZVirtualMachine, password: String, tx: Sender<PanelEvent>) -> Window {
        // SAFETY: the callers run this on the main queue.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let visible = NSScreen::mainScreen(mtm)
            .map(|s| rect(s.visibleFrame()))
            .filter(|r| r.w > 0.0 && r.h > 0.0)
            .unwrap_or(FALLBACK_VISIBLE);
        let probe = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0));
        let title_bar = NSWindow::frameRectForContentRect_styleMask(probe, style, mtm)
            .size
            .height
            - 100.0;
        let layout = guided_ui::window_layout(visible, ASPECT, title_bar.max(0.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                ns_rect(layout.content),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&text::panel_title()));
        window.setContentMinSize(NSSize::new(layout.min_w, layout.min_h));
        let size = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(layout.content.w, layout.content.h),
        );
        let content = NSView::initWithFrame(mtm.alloc(), size);
        let view =
            unsafe { VZVirtualMachineView::initWithFrame(mtm.alloc(), ns_rect(layout.view)) };
        unsafe {
            view.setVirtualMachine(Some(vm));
            view.setAutomaticallyReconfiguresDisplay(true);
            view.setCapturesSystemKeys(true);
        }
        content.addSubview(&view);
        window.setContentView(Some(&content));
        let panel = Panel::new(mtm, password, tx);
        panel.build(mtm, &window, &content, &view);
        window.setDelegate(Some(ProtocolObject::from_ref(&*panel)));
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&view));
        NSApplication::sharedApplication(mtm).activate();
        Window {
            window: Sendable(window),
            panel: Sendable(panel),
        }
    }

    /// Mark the steps 1 to `through` done and show the next one.
    pub fn mark_done(&self, through: usize) {
        let panel = Sendable(self.panel.0.clone());
        DispatchQueue::main().exec_async(move || {
            let panel = panel.get();
            panel.ivars().done.set(through);
            panel.update();
        });
    }

    /// Show the question for the password that the person set: the guest
    /// refused the one of the keyring. The answer comes as
    /// `PanelEvent::Password`.
    pub fn ask_password(&self) {
        let panel = Sendable(self.panel.0.clone());
        DispatchQueue::main().exec_async(move || panel.get().ask_password());
    }

    /// No step waits on the person (`on`), or one does again: the panel
    /// shows its banner and hides the Type buttons while none does.
    pub fn set_unattended(&self, on: bool) {
        let panel = Sendable(self.panel.0.clone());
        DispatchQueue::main().exec_async(move || panel.get().set_unattended(on, None));
    }

    /// A step goes back to the person: the guided steps with `line`.
    pub fn hand_over(&self, line: &str) {
        let panel = Sendable(self.panel.0.clone());
        let line = line.to_string();
        DispatchQueue::main().exec_async(move || panel.get().set_unattended(false, Some(&line)));
    }

    /// Close the window.
    pub fn close(&self) {
        let window = Sendable(self.window.0.clone());
        let panel = Sendable(self.panel.0.clone());
        DispatchQueue::main().exec_sync(move || {
            let (window, panel) = (window.get(), panel.get());
            panel.ivars().closing.set(true);
            window.setDelegate(None);
            window.close();
        });
    }
}
