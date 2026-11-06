//! A drop of the host on a session window, in the guest, macOS only. The
//! agent cannot make a real drag from another process, so it puts the file
//! URLs on the guest pasteboard and opens the files with the app, which
//! attaches or opens them.

use super::platform::Fail;
use crate::macvm::clipboard::{Item, FILE_URL};
use crate::macvm::pasteboard;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::{Path, PathBuf};

/// Put the file URLs on the pasteboard and open the files with the app
/// of `pid`, or with their default app without one. The new pasteboard
/// change count.
pub fn open(paths: &[PathBuf], pid: Option<u32>) -> Result<u64, Fail> {
    let urls: Vec<Retained<NSURL>> = paths
        .iter()
        .filter(|p| p.starts_with(crate::macvm::proto::SHARE_ROOT))
        .map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())))
        .collect();
    if urls.is_empty() {
        return Err((
            ErrorCode::BadRequest,
            text::io_failed("no path under the shares", ""),
        ));
    }
    let items: Vec<Item> = urls
        .iter()
        .filter_map(|u| u.absoluteString())
        .map(|s| Item::default().with(FILE_URL, s.to_string().as_bytes()))
        .collect();
    let count = pasteboard::write(&items).ok_or_else(|| {
        (
            ErrorCode::Io,
            text::io_failed("cannot write the pasteboard", ""),
        )
    })?;
    let ws = NSWorkspace::sharedWorkspace();
    let app = pid
        .and_then(|p| NSRunningApplication::runningApplicationWithProcessIdentifier(p as i32))
        .and_then(|a| a.bundleURL());
    match app {
        Some(app) => {
            let cfg = NSWorkspaceOpenConfiguration::configuration();
            ws.openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(&urls),
                &app,
                &cfg,
                None,
            );
        }
        None => {
            for u in &urls {
                if !ws.openURL(u) {
                    log::warn!("the guest could not open {}", path_of(u).display());
                }
            }
        }
    }
    Ok(count)
}

fn path_of(u: &NSURL) -> PathBuf {
    u.path()
        .map(|p| PathBuf::from(p.to_string()))
        .unwrap_or_else(|| Path::new("?").to_path_buf())
}
