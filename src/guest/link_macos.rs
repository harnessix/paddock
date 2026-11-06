//! The macOS parts of the links of the guest (`link.rs`): the link helper
//! app, and the start of the link socket in the agent.
//!
//! Launch Services sends a link to the default browser as an Apple event,
//! never as an argument, so the helper runs an `NSApplication` whose
//! delegate takes `application:openURLs:`. It hands each link to the
//! agent's socket and exits [`link::HELPER_LINGER`] after the last one,
//! or [`link::HELPER_WAIT`] after its start when no link came.

use super::link;
use crate::macvm::text;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate};
use objc2_foundation::{NSArray, NSDate, NSDefaultRunLoopMode, NSRunLoop, NSURL};
use std::cell::RefCell;
use std::path::Path;
use std::time::{Duration, Instant};

/// `lsregister`, which reads a bundle into the Launch Services database.
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
/// One pass of the run loop of the helper.
const TICK: f64 = 0.05;

/// The helper process: take the links of Launch Services and hand each
/// to the agent. The exit code: 0, or 1 when a link could not be handed.
pub fn helper_main(mtm: MainThreadMarker) -> i32 {
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon and no menu bar: the helper has no window.
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    let delegate = LinkDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.finishLaunching();
    let run_loop = NSRunLoop::mainRunLoop();
    let mut end = Instant::now() + link::HELPER_WAIT;
    let mut code = 0;
    while Instant::now() < end {
        let until = NSDate::dateWithTimeIntervalSinceNow(TICK);
        // SAFETY: a run loop mode constant of Foundation.
        let mode = unsafe { NSDefaultRunLoopMode };
        if !run_loop.runMode_beforeDate(mode, &until) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let urls = delegate.take();
        if urls.is_empty() {
            continue;
        }
        for url in urls {
            if let Err(e) = link::send(Path::new(link::SOCKET), &url) {
                eprintln!("{}", text::link_helper_failed(&e));
                code = 1;
            }
        }
        end = Instant::now() + link::HELPER_LINGER;
    }
    code
}

pub struct LinkIvars {
    urls: RefCell<Vec<String>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockLinkDelegate"]
    #[ivars = LinkIvars]
    pub struct LinkDelegate;

    unsafe impl NSObjectProtocol for LinkDelegate {}

    unsafe impl NSApplicationDelegate for LinkDelegate {
        /// The links of `open URL` and of every program of the guest.
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let mut out = Vec::new();
            for url in urls.iter() {
                if let Some(s) = url.absoluteString() {
                    out.push(s.to_string());
                }
            }
            self.ivars().urls.borrow_mut().extend(out);
        }
    }
);

impl LinkDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(LinkIvars {
            urls: RefCell::new(Vec::new()),
        });
        // SAFETY: the superclass initializer of NSObject.
        unsafe { objc2::msg_send![super(this), init] }
    }

    /// The links that arrived, if any, and forget them.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.ivars().urls.borrow_mut())
    }
}

/// In the agent: make the helper bundle from the agent's own binary, read
/// it into Launch Services when it changed (or at the first start of this
/// boot), and serve the link socket. `sink` sends `link.open`.
pub fn start(sink: link::Sink) {
    std::thread::Builder::new()
        .name("link-socket".into())
        .spawn(move || {
            prepare_bundle();
            match link::bind(Path::new(link::SOCKET)) {
                Ok(l) => {
                    log::info!("link: the link socket listens at {}", link::SOCKET);
                    link::serve(l, sink);
                }
                Err(e) => log::error!("link: cannot listen at {}: {e}", link::SOCKET),
            }
        })
        .expect("a thread for the link socket");
}

/// The bundle and its registration. A failure is logged: the links then
/// open in the guest's own browser.
fn prepare_bundle() {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log::warn!("link: no path of the agent binary: {e}");
            return;
        }
    };
    let bundle = link::bundle_path(Path::new(super::SUPPORT_DIR));
    let version = crate::macvm::version::paddock_version();
    match link::ensure_bundle(&bundle, &exe, version) {
        Ok(changed) => log::info!(
            "link: the helper bundle {} is {}",
            bundle.display(),
            if changed { "new" } else { "current" }
        ),
        Err(e) => {
            log::warn!(
                "link: cannot make the helper bundle {}: {e}",
                bundle.display()
            );
            return;
        }
    }
    // Registered at every start of the agent: cheap, and a Launch
    // Services database that lost the bundle gets it back.
    let out = std::process::Command::new(LSREGISTER)
        .arg("-f")
        .arg(&bundle)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => log::info!("link: the helper is registered"),
        Ok(o) => log::warn!(
            "link: lsregister failed ({}): {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log::warn!("link: cannot run lsregister: {e}"),
    }
}
