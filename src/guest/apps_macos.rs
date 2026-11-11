//! Launch and quit of the app in the guest with AppKit: `NSWorkspace`
//! launches, `NSRunningApplication` terminates, with `forceTerminate` on
//! `force`.

use super::platform::Fail;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSString, NSURL};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

/// The launch answers within this time, else it failed.
const LAUNCH_WAIT: Duration = Duration::from_secs(25);

/// Launch the app bundle at `path` and give its pid.
pub fn launch(path: &Path, args: &[String], env: &BTreeMap<String, String>) -> Result<u32, Fail> {
    if !path.is_dir() {
        return Err((ErrorCode::NoSuchApp, text::not_app_bundle(path)));
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let cfg = NSWorkspaceOpenConfiguration::configuration();
    let args: Vec<Retained<NSString>> = args.iter().map(|a| NSString::from_str(a)).collect();
    cfg.setArguments(&NSArray::from_retained_slice(&args));
    if !env.is_empty() {
        let keys: Vec<Retained<NSString>> = env.keys().map(|k| NSString::from_str(k)).collect();
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let values: Vec<Retained<NSString>> = env.values().map(|v| NSString::from_str(v)).collect();
        cfg.setEnvironment(&NSDictionary::from_retained_objects(&key_refs, &values));
    }
    cfg.setActivates(true);
    cfg.setPromptsUserIfNeeded(false);
    let (tx, rx) = mpsc::channel::<Result<u32, String>>();
    let tx = Mutex::new(tx);
    let block = RcBlock::new(move |app: *mut NSRunningApplication, err: *mut NSError| {
        // SAFETY: the pointers are valid for the call, or null.
        let result = match unsafe { app.as_ref() } {
            Some(app) => Ok(app.processIdentifier() as u32),
            None => Err(unsafe { err.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "no app and no error".into())),
        };
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(result);
        }
    });
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &url,
        &cfg,
        Some(&block),
    );
    match rx.recv_timeout(LAUNCH_WAIT) {
        Ok(Ok(pid)) => {
            log::info!("launched {} as pid {pid}", path.display());
            Ok(pid)
        }
        Ok(Err(text)) => Err((ErrorCode::Io, text::launch_failed(path, &text))),
        Err(_) => Err((ErrorCode::Timeout, text::launch_timeout(path, LAUNCH_WAIT))),
    }
}

/// Ask the app to quit, or kill it on `force`. The exit shows in the
/// process table later.
pub fn quit(pid: u32, force: bool) -> Result<(), Fail> {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
    else {
        return Err((ErrorCode::NoSuchApp, text::no_running_app(pid)));
    };
    let sent = if force {
        app.forceTerminate()
    } else {
        app.terminate()
    };
    if sent {
        Ok(())
    } else {
        Err((ErrorCode::Io, text::quit_not_taken(pid)))
    }
}
