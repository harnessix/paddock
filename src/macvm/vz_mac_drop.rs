//! The drop target of a session window, macOS only: a
//! `VZVirtualMachineView` that is an `NSDraggingDestination` for file
//! URLs, file promises, plain text and images. A drop becomes
//! [`DropEvent`] on a channel; the session (`run_files.rs`) stages the
//! items and sends `drop` to the guest. The session windows use this view
//! in place of a plain `VZVirtualMachineView`.

use super::drop::Item;
use super::proto::Point;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::runtime::{AnyClass, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDragOperation, NSDraggingDestination,
    NSDraggingInfo, NSFilePromiseReceiver, NSPasteboard, NSPasteboardTypeFileURL,
    NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSView,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSOperationQueue, NSRect, NSString, NSURL};
use objc2_virtualization::VZVirtualMachineView;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::mpsc::Sender;

/// One drop on a window, in guest window id and guest points.
#[derive(Debug, Clone)]
pub struct DropEvent {
    pub window: u32,
    pub point: Point,
    pub items: Vec<Item>,
}

pub struct DropIvars {
    tx: Sender<DropEvent>,
    /// The guest window that this view shows.
    window: Cell<u32>,
    /// Where the file promises are received.
    promise_dir: PathBuf,
    promises: Cell<u64>,
}

define_class!(
    #[unsafe(super(
        VZVirtualMachineView,
        NSView,
        objc2_app_kit::NSResponder,
        objc2::runtime::NSObject
    ))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockDropVmView"]
    #[ivars = DropIvars]
    pub struct DropVmView;

    unsafe impl NSObjectProtocol for DropVmView {}

    unsafe impl NSDraggingDestination for DropVmView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(
            &self,
            _sender: &ProtocolObject<dyn NSDraggingInfo>,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(
            &self,
            _sender: &ProtocolObject<dyn NSDraggingInfo>,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            Bool::YES
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            let pb = sender.draggingPasteboard();
            let at = sender.draggingLocation();
            let frame = self.frame();
            // The view's origin is bottom left; the guest's is top left.
            let point = Point {
                x: at.x - frame.origin.x,
                y: frame.size.height - (at.y - frame.origin.y),
            };
            let iv = self.ivars();
            let window = iv.window.get();
            let items = read_items(&pb);
            let promised = receive_promises(&pb, iv, window, point);
            if items.is_empty() && !promised {
                return Bool::NO;
            }
            if !items.is_empty() {
                let _ = iv.tx.send(DropEvent {
                    window,
                    point,
                    items,
                });
            }
            Bool::YES
        }
    }
);

impl DropVmView {
    /// A view of `frame` for the guest window `window`. The file promises
    /// of a drop land under `promise_dir`.
    pub fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        window: u32,
        promise_dir: PathBuf,
        tx: Sender<DropEvent>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DropIvars {
            tx,
            window: Cell::new(window),
            promise_dir,
            promises: Cell::new(0),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let mut types: Vec<Retained<NSString>> = Vec::new();
        // SAFETY: constants of AppKit.
        unsafe {
            for t in [
                NSPasteboardTypeFileURL,
                NSPasteboardTypeString,
                NSPasteboardTypePNG,
                NSPasteboardTypeTIFF,
            ] {
                types.push(NSString::from_str(&t.to_string()));
            }
        }
        for t in NSFilePromiseReceiver::readableDraggedTypes().iter() {
            types.push(t);
        }
        this.registerForDraggedTypes(&NSArray::from_retained_slice(&types));
        this
    }

    /// The guest window changed (the view is kept across a rebuild).
    pub fn set_window(&self, window: u32) {
        self.ivars().window.set(window);
    }
}

/// The items of a drag pasteboard: each file URL a file, else a PNG, a
/// TIFF (as PNG), or a text.
pub fn read_items(pb: &NSPasteboard) -> Vec<Item> {
    let Some(items) = pb.pasteboardItems() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for it in items.iter() {
        // SAFETY: constants of AppKit.
        let (file, string, png, tiff) = unsafe {
            (
                it.stringForType(NSPasteboardTypeFileURL),
                it.stringForType(NSPasteboardTypeString),
                it.dataForType(NSPasteboardTypePNG),
                it.dataForType(NSPasteboardTypeTIFF),
            )
        };
        if let Some(url) = file.and_then(|u| super::clipboard::url_path(&u.to_string())) {
            out.push(Item::File(url));
        } else if let Some(d) = png {
            out.push(Item::Image(d.to_vec()));
        } else if let Some(d) = tiff {
            if let Some(png) = tiff_to_png(&d.to_vec()) {
                out.push(Item::Image(png));
            }
        } else if let Some(s) = string {
            let s = s.to_string();
            if !s.is_empty() {
                out.push(Item::Text(s));
            }
        }
    }
    out
}

/// TIFF bytes as PNG, through `NSBitmapImageRep`.
pub fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::imageRepWithData(&objc2_foundation::NSData::with_bytes(tiff))?;
    let props: Retained<NSDictionary<NSString, objc2::runtime::AnyObject>> = NSDictionary::new();
    // SAFETY: a PNG representation with no properties.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(data.to_vec())
}

/// Receive the file promises of a drop (a file from Mail, Photos, a
/// browser) into `promise_dir/<n>/`; each file arrives later as its own
/// [`DropEvent`]. True when the pasteboard had promises.
fn receive_promises(pb: &NSPasteboard, iv: &DropIvars, window: u32, point: Point) -> bool {
    let class: &AnyClass = NSFilePromiseReceiver::class();
    let classes = NSArray::from_slice(&[class]);
    // SAFETY: the class array names NSFilePromiseReceiver; no options.
    let Some(receivers) = (unsafe { pb.readObjectsForClasses_options(&classes, None) }) else {
        return false;
    };
    let mut any = false;
    for r in receivers.iter() {
        let Ok(receiver) = r.downcast::<NSFilePromiseReceiver>() else {
            continue;
        };
        iv.promises.set(iv.promises.get() + 1);
        let dir = iv.promise_dir.join(iv.promises.get().to_string());
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let Ok(url) = crate::vm::vz::url(&dir) else {
            continue;
        };
        let tx = iv.tx.clone();
        let reader = RcBlock::new(move |file: NonNull<NSURL>, err: *mut NSError| {
            if !err.is_null() {
                log::warn!("macvm: a promised file did not arrive");
                return;
            }
            // SAFETY: a non-null NSURL of the framework.
            let file = unsafe { file.as_ref() };
            if let Some(p) = file.path() {
                let _ = tx.send(DropEvent {
                    window,
                    point,
                    items: vec![Item::File(PathBuf::from(p.to_string()))],
                });
            }
        });
        let options: Retained<NSDictionary> = NSDictionary::new();
        let queue = NSOperationQueue::new();
        // SAFETY: the destination folder exists; the reader block outlives
        // the call, and the queue is one of our own.
        unsafe {
            receiver.receivePromisedFilesAtDestination_options_operationQueue_reader(
                &url, &options, &queue, &reader,
            );
        }
        any = true;
    }
    any
}

/// The staging folder of the promises of a profile, for the view.
pub fn promise_dir(drop_dir: &Path) -> PathBuf {
    drop_dir.join(".promise")
}
