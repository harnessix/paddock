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
use objc2_foundation::{NSArray, NSDate, NSDefaultRunLoopMode, NSURL};
use std::cell::RefCell;
use std::path::Path;
use std::time::Instant;

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
    let mut end = Instant::now() + link::HELPER_WAIT;
    let mut code = 0;
    while Instant::now() < end {
        let until = NSDate::dateWithTimeIntervalSinceNow(TICK);
        // SAFETY: a run loop mode constant of Foundation.
        let mode = unsafe { NSDefaultRunLoopMode };
        // AppKit's events, as `NSApplication.run` takes them: the run loop
        // alone never delivers the open-URL Apple event, and the helper
        // then ended after its wait without a link.
        crate::macvm::run_appkit::pass(&app, &until, mode);
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
                    // The helper runs as the guest person, as the app that
                    // opens the link: the socket belongs to the person,
                    // still 0600.
                    if let Some((_, uid, gid)) = guest_person() {
                        if let Err(e) = std::os::unix::fs::chown(link::SOCKET, Some(uid), Some(gid))
                        {
                            log::warn!("link: cannot give the link socket to uid {uid}: {e}");
                        }
                    }
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
    register_for_user(&bundle);
}

/// The agent runs as root, and `lsregister` writes the Launch Services
/// database of the user that runs it. The default browser of the guest
/// person comes from the person's own database: without the helper there,
/// an `https` link goes to another app that claims the scheme (a
/// ChatGPT app claims `http` and `https` itself) and no window opens. So
/// the helper is registered for the owner of the guest home too, and the
/// `https` entry of the person's Launch Services preferences is logged.
/// The guest person: the home on the data disk, else the home of the
/// golden, with its owner.
fn guest_person() -> Option<(std::path::PathBuf, u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    let home = [Path::new(super::data::HOME), Path::new("/Users/paddock")]
        .into_iter()
        .find(|h| h.is_dir())?;
    let meta = std::fs::metadata(home).ok()?;
    (meta.uid() != 0).then(|| (home.to_path_buf(), meta.uid(), meta.gid()))
}

fn register_for_user(bundle: &Path) {
    let Some((home, uid, gid)) = guest_person() else {
        log::warn!("link: no guest home");
        return;
    };
    let out = std::process::Command::new("/bin/launchctl")
        .args(["asuser", &uid.to_string(), "/usr/bin/sudo", "-u"])
        .arg(format!("#{uid}"))
        .arg(LSREGISTER)
        .arg("-f")
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            log::info!("link: the helper is registered for the guest person (uid {uid})")
        }
        Ok(o) => log::warn!(
            "link: lsregister for uid {uid} failed ({}): {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log::warn!("link: cannot run lsregister for uid {uid}: {e}"),
    }
    let prefs = home
        .join(crate::macvm::launch_services::FOLDER)
        .join(crate::macvm::launch_services::FILE);
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(&prefs)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let xml = String::from_utf8_lossy(&o.stdout);
            if xml.contains(link::BUNDLE_ID) {
                log::info!(
                    "link: the Launch Services preferences of the guest person name the helper"
                );
            } else {
                log::warn!("link: the Launch Services preferences of the guest person do not name the helper: paddock writes them");
                write_prefs(&prefs, &xml, uid, gid);
            }
        }
        _ => {
            log::warn!(
                "link: no Launch Services preferences at {}: paddock writes them",
                prefs.display()
            );
            write_prefs(&prefs, "", uid, gid);
        }
    }
}

/// Write the `http` and `https` entries of the helper into the person's
/// Launch Services preferences, owned by the person, and restart the
/// person's `lsd` so that it reads them.
fn write_prefs(prefs: &Path, xml: &str, uid: u32, gid: u32) {
    use crate::macvm::launch_services as ls;
    let text = if xml.trim().is_empty() {
        Ok(ls::fresh(link::BUNDLE_ID))
    } else {
        ls::merge(xml, link::BUNDLE_ID)
    };
    let text = match text {
        Ok(t) => t,
        Err(e) => {
            log::warn!("link: cannot merge the Launch Services preferences: {e}");
            return;
        }
    };
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = prefs.parent() {
            std::fs::create_dir_all(dir)?;
            std::os::unix::fs::chown(dir, Some(uid), Some(gid))?;
        }
        let tmp = prefs.with_extension("plist.paddock");
        std::fs::write(&tmp, text.as_bytes())?;
        std::os::unix::fs::chown(&tmp, Some(uid), Some(gid))?;
        std::fs::rename(&tmp, prefs)
    })();
    match result {
        Ok(()) => log::info!("link: wrote the helper into {}", prefs.display()),
        Err(e) => {
            log::warn!("link: cannot write {}: {e}", prefs.display());
            return;
        }
    }
    let _ = std::process::Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &format!("gui/{uid}/com.apple.lsd")])
        .stdin(std::process::Stdio::null())
        .output();
}
