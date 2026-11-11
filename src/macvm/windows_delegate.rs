//! The delegate of the session windows, macOS only: key, minimize, move,
//! resize and full screen notifications of each `SessionWindow` become
//! [`HostEvent`]s or the view and cover updates of `windows_lead.rs`.

use super::windows::{HostEvent, HostSink, SessionWindow};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSWindowDelegate;
use objc2_foundation::NSNotification;

pub struct DelegateIvars {
    sink: HostSink,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockSessionWindowDelegate"]
    #[ivars = DelegateIvars]
    pub struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::BecameKey(id));
            }
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::ResignedKey(id));
            }
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::Miniaturized(id));
            }
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::Deminiaturized(id));
            }
        }

        #[unsafe(method(windowDidMove:))]
        fn did_move(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::moved(&w);
            }
        }

        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::moved(&w);
            }
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::full_screen_done(&w, true);
            }
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::full_screen_done(&w, false);
            }
        }
    }
);

impl WindowDelegate {
    pub(super) fn new(mtm: MainThreadMarker, sink: HostSink) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { sink });
        unsafe { msg_send![super(this), init] }
    }
}

/// The session window of a window notification.
fn window_of(note: &NSNotification) -> Option<Retained<SessionWindow>> {
    note.object()?.downcast::<SessionWindow>().ok()
}

/// The guest window id of a window notification.
fn id_of(note: &NSNotification) -> Option<u32> {
    window_of(note).map(|w| w.id())
}
