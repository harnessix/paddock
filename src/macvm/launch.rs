//! What a launcher bundle of `paddock app install` does when Finder starts
//! it. macOS only.
//!
//! Two kinds, both the shim bundle of `shim.rs` with a target file:
//!
//! - A launcher of one app runs `paddock app run NAME` and exits as soon
//!   as the session is up, so the Dock shows the session's own icon and
//!   not a second one.
//! - The drag target `Paddock Apps.app` takes app bundles. Finder sends
//!   them as an open-documents Apple event, never as an argv, so this one
//!   runs an `NSApplication` until the event arrives or [`DROP_WAIT`]
//!   passes. Each bundle becomes an app profile and starts.
//!
//! What to do with a bundle is `launchers::macapp::add_dropped`, which is
//! tested on Linux. This file is the AppKit shell around it.

use super::host::{self, Wait};
use super::text;
use crate::env::HostEnv;
use crate::launchers::macapp;
use crate::vm::helper::InstallArgs;
use anyhow::{bail, Result};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSRunningApplication,
};
use objc2_foundation::{NSArray, NSDate, NSDefaultRunLoopMode, NSRunLoop, NSURL};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The drag target waits this long for the bundles of the drop. A target
/// that someone opened without a document exits after it.
pub const DROP_WAIT: Duration = Duration::from_secs(10);
/// One pass of the run loop while it waits.
const TICK: f64 = 0.05;

/// `paddock-vm install`: the entry point of a launcher bundle.
pub fn install_main(env: &HostEnv, args: &InstallArgs) -> Result<i32> {
    if let Some(app) = &args.app {
        let resumed_by = args.resume_from_dock.then(text::resume_reason_dock);
        return start(env, app, args.instance, resumed_by);
    }
    if !args.drop_target {
        let exe = std::env::current_exe().unwrap_or_default();
        bail!(text::shim_no_target(&exe));
    }
    let mut paths = args.paths.clone();
    if paths.is_empty() {
        paths = wait_for_drop();
    }
    if paths.is_empty() {
        // Nothing was dropped: the target has nothing to do.
        return Ok(0);
    }
    let mut code = 0;
    for path in paths {
        match add_and_start(env, &path) {
            Ok(()) => {}
            Err(e) => {
                let line = format!("{e:#}"); // not user text
                eprintln!("{line}");
                crate::app::desktop_notice(env, &crate::app::start_error_note("paddock", &e));
                code = 1;
            }
        }
    }
    Ok(code)
}

/// One dropped bundle: the profile, its launcher, and the session.
fn add_and_start(env: &HostEnv, path: &Path) -> Result<()> {
    let slug = macapp::add_dropped(env, path)?;
    println!("{}", text::dropped_bundle_added(&slug, path));
    start(env, &slug, None, None)?;
    Ok(())
}

/// `paddock app run NAME` for a launcher: the session takes over the Dock,
/// so this process returns at once.
fn start(
    env: &HostEnv,
    name: &str,
    instance: Option<u32>,
    resumed_by: Option<String>,
) -> Result<i32> {
    let o = crate::app::AppOptions {
        name: name.to_string(),
        // The folders of `[apps.NAME] roots`, or none.
        roots: crate::app::roots(env, name, &[]).unwrap_or_default(),
        ro: vec![],
        gpu: false,
        network: None,
        inherit_env: None,
        auth: None,
        args: vec![],
        backend: Some(host::BACKEND.to_string()),
        worktree: false,
        guest: super::host_record::GuestOptions {
            instance,
            resumed_by,
            ..Default::default()
        },
    };
    host::run_app_mode(env, &o, Wait::Detach)
}

/// Run an `NSApplication` until Finder sends the dropped bundles, or
/// [`DROP_WAIT`] passes. The paths of the drop.
fn wait_for_drop() -> Vec<PathBuf> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let app = NSApplication::sharedApplication(mtm);
    // The target shows no window and no Dock icon of its own: the apps it
    // starts have theirs.
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = DropDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.finishLaunching();
    let run_loop = NSRunLoop::mainRunLoop();
    let end = Instant::now() + DROP_WAIT;
    while Instant::now() < end {
        let until = NSDate::dateWithTimeIntervalSinceNow(TICK);
        // SAFETY: a run loop mode constant of Foundation.
        let mode = unsafe { NSDefaultRunLoopMode };
        if !run_loop.runMode_beforeDate(mode, &until) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let paths = delegate.take();
        if !paths.is_empty() {
            return paths;
        }
    }
    Vec::new()
}

pub struct DropIvars {
    paths: RefCell<Vec<PathBuf>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockDropDelegate"]
    #[ivars = DropIvars]
    pub struct DropDelegate;

    unsafe impl NSObjectProtocol for DropDelegate {}

    unsafe impl NSApplicationDelegate for DropDelegate {
        /// The bundles of the drop, or of `open -a "Paddock Apps" X.app`.
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let mut out = Vec::new();
            for url in urls.iter() {
                if let Some(p) = url.path() {
                    out.push(PathBuf::from(p.to_string()));
                }
            }
            self.ivars().paths.borrow_mut().extend(out);
        }
    }
);

impl DropDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DropIvars {
            paths: RefCell::new(Vec::new()),
        });
        // SAFETY: the superclass initializer of NSObject.
        unsafe { objc2::msg_send![super(this), init] }
    }

    /// The paths that arrived, if any, and forget them.
    fn take(&self) -> Vec<PathBuf> {
        std::mem::take(&mut self.ivars().paths.borrow_mut())
    }
}

/// The running application of this process, for the Dock face. Kept so a
/// future caller has one place for it.
pub fn current_app() -> Retained<NSRunningApplication> {
    NSRunningApplication::currentApplication()
}
