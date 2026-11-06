//! The general `NSPasteboard` as [`clipboard::Item`]s, macOS only. The shim
//! reads and writes the host pasteboard with it, the guest agent the guest
//! pasteboard. Only the types of `clipboard::READ_TYPES` and the three
//! markers are read; a write puts every type of each item.

use super::clipboard::{Item, READ_TYPES};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardItem, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};

/// The change count of the general pasteboard.
pub fn count() -> u64 {
    NSPasteboard::generalPasteboard().changeCount().max(0) as u64
}

/// The items of the general pasteboard with their known types.
pub fn read() -> Vec<Item> {
    let pb = NSPasteboard::generalPasteboard();
    let Some(items) = pb.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|it| {
            let types: Vec<String> = it.types().iter().map(|t| t.to_string()).collect();
            let mut item = Item::default();
            for t in READ_TYPES {
                if !types.iter().any(|x| x == t) {
                    continue;
                }
                let data = it
                    .dataForType(&NSString::from_str(t))
                    .map(|d| d.to_vec())
                    .unwrap_or_default();
                item.types.push((t.to_string(), data));
            }
            item
        })
        .collect()
}

/// Replace the general pasteboard with `items`. The new change count, or
/// None when the pasteboard refused the write.
pub fn write(items: &[Item]) -> Option<u64> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
        .iter()
        .map(|i| {
            let it = NSPasteboardItem::new();
            for (t, d) in &i.types {
                it.setData_forType(&NSData::with_bytes(d), &NSString::from_str(t));
            }
            ProtocolObject::from_retained(it)
        })
        .collect();
    if objects.is_empty() || pb.writeObjects(&NSArray::from_retained_slice(&objects)) {
        Some(count())
    } else {
        None
    }
}

/// The PNG of a TIFF through `NSBitmapImageRep`. None
/// when the bytes are not an image that AppKit reads.
pub fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(tiff))?;
    // SAFETY: the PNG file type with no properties.
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}
