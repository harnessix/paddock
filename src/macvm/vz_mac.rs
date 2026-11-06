//! The Virtualization framework side of `paddock macvm setup`. macOS only,
//! and it runs in `paddock-vm` (`paddock-vm macvm-setup`), the binary with
//! the `com.apple.security.virtualization` entitlement.
//!
//! The main thread runs the AppKit loop for the guided window. The setup
//! driver (`setup.rs`) runs on a worker thread with [`VzBackend`], and
//! every call that needs the main thread goes through the main dispatch
//! queue. Each framework call is a function of its own, and every error
//! carries the `localizedDescription` of the framework. The states that
//! talk to the guest agent (guided, configure, the shutdown for the
//! seal) are in `vz_mac_session.rs`.

use super::golden::DISK_BYTES;
use super::layout::{self, Golden, Layout};
use super::setup::{self, Backend, End, HostInfo, ImageInfo, Opts, RestoreInfo, Ui};
use super::text;
use super::vz_mac_agent::AgentEvent;
use super::vz_mac_config;
use super::vz_mac_image::{
    content_length, download, fetch_latest, load_image, make_sparse, run_installer, version_of,
};
use super::vz_mac_session;
use crate::env::HostEnv;
pub(crate) use crate::vm::vz::{describe, error_text, url};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSArray, NSError, NSFileHandle};
use objc2_virtualization::*;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// The display of the guided window: 1240 by 1000 points at 2x.
pub(super) const DISPLAY: super::session::Display = super::session::Display {
    w: 2480,
    h: 2000,
    scale: 2,
};
/// The RAM and CPUs of the golden boots, within the minimum of the image
/// and the maximum of the host.
pub(super) const WANT_MEMORY: u64 = 4 << 30;
pub(super) const WANT_CPUS: usize = 4;
/// How long a boot may take to the `started` callback.
pub(super) const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the guest may take to shut down for the seal.
pub(super) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(300);

/// A value that crosses threads although Objective-C did not say so. The
/// framework objects here are used from one thread at a time.
pub(crate) struct Sendable<T>(pub T);
// SAFETY: see the type's comment; the callers keep the one-thread rule.
unsafe impl<T> Send for Sendable<T> {}
unsafe impl<T> Sync for Sendable<T> {}

impl<T> Sendable<T> {
    /// A method, so that a `move` closure takes the whole wrapper and not
    /// the field alone (edition 2021 closures capture fields).
    pub fn get(&self) -> &T {
        &self.0
    }
}

/// `paddock-vm macvm-setup`: the setup with the framework calls.
pub fn setup_main(env: &HostEnv, args: &crate::vm::helper::MacvmSetupArgs) -> Result<i32> {
    if !cfg!(target_arch = "aarch64") {
        bail!(text::needs_apple_silicon());
    }
    if !unsafe { VZVirtualMachine::isSupported() } {
        bail!(text::no_virtualization());
    }
    let layout = Layout::at(args.dir.clone());
    let golden = if args.new {
        layout.golden_new.clone()
    } else {
        layout.golden.clone()
    };
    let current_version = if args.new {
        layout
            .golden
            .read_manifest()
            .ok()
            .flatten()
            .filter(|m| m.state == super::golden::State::Sealed)
            .map(|m| m.version)
    } else {
        None
    };
    let opts = Opts {
        ipsw: args.ipsw.clone(),
        yes: args.yes,
        agent: args.agent.clone(),
        fake_free: env.var("PADDOCK_MACVM_FAKE_FREE").map(str::to_string),
        skip_configure: env.var("PADDOCK_MACVM_SKIP_CONFIGURE") == Some("1"),
        force: args.force,
        current_version,
        guided: env.var("PADDOCK_MACVM_GUIDED") == Some("1"),
    };
    watch_parent();
    let mtm = MainThreadMarker::new()
        .with_context(|| text::main_thread_only("paddock-vm macvm-setup"))?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let env = env.clone();
    std::thread::spawn(move || {
        let mut backend = VzBackend::default();
        let mut out = std::io::stdout();
        let mut ask = setup::terminal_ask;
        let mut ui = Ui {
            out: &mut out,
            ask: &mut ask,
        };
        let code = setup::run(&env, &layout, &golden, &mut backend, &opts, &mut ui);
        drop(backend);
        match code {
            Ok(c) => std::process::exit(c),
            Err(e) => {
                eprintln!("{}", text::helper_line(&format!("{e:#}")));
                std::process::exit(1);
            }
        }
    });
    app.run();
    Ok(0)
}

/// End when the parent (`paddock macvm setup`) is gone: a `kill -9` of it
/// must not leave an installer running without a terminal.
fn watch_parent() {
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        if unsafe { libc::getppid() } != parent {
            eprintln!("{}", text::parent_ended());
            std::process::exit(1);
        }
    });
}

/// What the VM of the golden and the guest agent report.
pub(super) enum VmEvent {
    Started(Result<(), String>),
    GuestStopped,
    Failed(String),
    /// The control line of the guest agent.
    Agent(AgentEvent),
}

#[derive(Default)]
pub struct VzBackend {
    image: Option<(PathBuf, Sendable<Retained<VZMacOSRestoreImage>>)>,
    pub(super) session: Option<vz_mac_session::Session>,
    /// The panel steps done, for the `Continue` button and the marks.
    pub(super) progress: super::guided::Progress,
    /// No step waits on the person: paddock, not the person, ends the
    /// waits.
    pub(super) unattended: bool,
    /// The offline seed put the agent on the disk.
    pub(super) seeded: bool,
}

impl Backend for VzBackend {
    fn host(&self) -> HostInfo {
        HostInfo {
            version: sw_vers("-productVersion"),
            build: sw_vers("-buildVersion"),
        }
    }

    fn latest(&mut self) -> Result<RestoreInfo> {
        let image = fetch_latest()?;
        let url = unsafe { image.0.URL().absoluteString() }
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!(text::restore_image_no_url()))?;
        let (version, build) = version_of(&image.0);
        let size = content_length(&url);
        Ok(RestoreInfo {
            url,
            version,
            build,
            size,
        })
    }

    fn download(&mut self, url: &str, dest: &Path, size: Option<u64>) -> Result<()> {
        download(url, dest, size)
    }

    fn check_image(&mut self, path: &Path) -> Result<ImageInfo> {
        let image = load_image(path)?;
        let (version, build) = version_of(&image.0);
        let Some(req) = (unsafe { image.0.mostFeaturefulSupportedConfiguration() }) else {
            bail!(text::image_unsupported(&version, &build));
        };
        let hw = unsafe { req.hardwareModel() };
        if !unsafe { hw.isSupported() } {
            bail!(text::hardware_model_unsupported(&version));
        }
        let b64 = base64::engine::general_purpose::STANDARD
            .encode(unsafe { hw.dataRepresentation() }.to_vec());
        self.image = Some((path.to_path_buf(), image));
        Ok(ImageInfo {
            version,
            build,
            hardware_model_b64: b64,
        })
    }

    fn install(&mut self, golden: &Golden, ipsw: &Path) -> Result<()> {
        if self.image.as_ref().is_none_or(|(p, _)| p != ipsw) {
            self.check_image(ipsw)?;
        }
        let image = &self.image.as_ref().expect("checked above").1;
        let req = unsafe { image.0.mostFeaturefulSupportedConfiguration() }
            .ok_or_else(|| anyhow!(text::image_lost_configuration()))?;
        let hw = unsafe { req.hardwareModel() };
        let cpus = unsafe { req.minimumSupportedCPUCount() };
        let memory = unsafe { req.minimumSupportedMemorySize() };
        crate::privfs::dir(&golden.dir)?;
        let disk = layout::partial(&golden.disk);
        make_sparse(&disk, DISK_BYTES)?;
        let aux = layout::partial(&golden.aux);
        let _ = std::fs::remove_file(&aux);
        let aux_url = url(&aux)?;
        let storage = unsafe {
            VZMacAuxiliaryStorage::initCreatingStorageAtURL_hardwareModel_options_error(
                VZMacAuxiliaryStorage::alloc(),
                &aux_url,
                &hw,
                VZMacAuxiliaryStorageInitializationOptions::AllowOverwrite,
            )
        }
        .map_err(|e| anyhow!(text::cannot_make_for_vm(&aux, &describe(&e))))?;
        let machine_id = unsafe { VZMacMachineIdentifier::new() };
        crate::privfs::write(
            &layout::partial(&golden.machine_id),
            unsafe { machine_id.dataRepresentation() }.to_vec(),
        )?;
        crate::privfs::write(
            &layout::partial(&golden.hardware_model),
            unsafe { hw.dataRepresentation() }.to_vec(),
        )?;
        let parts = Parts {
            disk,
            console: None,
            hw,
            machine_id,
            storage,
            cpus: cpus.max(2),
            memory: memory.max(
                WANT_MEMORY
                    .min(unsafe { VZVirtualMachineConfiguration::maximumAllowedMemorySize() }),
            ),
            network: None,
            seed: None,
        };
        let config = configuration(&parts)?;
        let queue = DispatchQueue::new("paddock-macvm-install", None);
        let vm = Sendable(unsafe {
            VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &config,
                &queue,
            )
        });
        run_installer(&vm, &queue, ipsw)
    }

    fn provisioning(&self) -> std::result::Result<(), String> {
        super::vz_mac_provision::support()
    }

    fn guided(
        &mut self,
        golden: &Golden,
        password: &str,
        route: &super::provision::Route,
    ) -> Result<End> {
        self.guided_run(golden, password, route)
    }

    fn configure(&mut self, golden: &Golden, password: &str) -> Result<End> {
        self.configure_run(golden, password)
    }

    fn ask_password(&mut self, golden: &Golden) -> Result<Option<String>> {
        self.ask_password_run(golden)
    }

    fn shutdown(&mut self, golden: &Golden, password: &str) -> Result<()> {
        self.stop_guest(golden, password)
    }

    fn disk_tool(&mut self) -> Option<Box<dyn super::diskimage::DiskTool>> {
        Some(Box::new(super::diskimage::Hdiutil))
    }

    fn agent_requirement(&mut self, agent: Option<&Path>) -> Result<Vec<u8>> {
        Ok(super::tcc::agent_requirement(agent))
    }

    fn trusted(&mut self, golden: &Golden, password: &str) -> Result<Option<bool>> {
        self.trusted_run(golden, password)
    }

    fn finish(&mut self) {
        if self.session.is_some() {
            self.stop_session(Duration::from_secs(60));
        }
    }
}

/// What a configuration of the golden needs.
pub(super) struct Parts {
    pub disk: PathBuf,
    pub console: Option<PathBuf>,
    pub hw: Retained<VZMacHardwareModel>,
    pub machine_id: Retained<VZMacMachineIdentifier>,
    pub storage: Retained<VZMacAuxiliaryStorage>,
    pub cpus: usize,
    pub memory: u64,
    pub network: Option<OwnedFd>,
    /// The seed folder, shared read-only as `seed::SHARE` under the
    /// automount tag, so the guest sees it at
    /// `/Volumes/My Shared Files/paddock-seed`. `None` for the install.
    pub seed: Option<PathBuf>,
}

/// The configuration of the golden guest: the platform with the hardware
/// model, the machine identifier and the auxiliary storage, the macOS
/// boot loader, the disk, one display, a keyboard and a trackpad, an
/// entropy device, a serial console to a file, and the stub network and
/// the seed share when asked. The devices come from `vz_mac_config`, as for a session.
pub(super) fn configuration(p: &Parts) -> Result<Retained<VZVirtualMachineConfiguration>> {
    let config = unsafe { VZVirtualMachineConfiguration::new() };
    let platform = unsafe { VZMacPlatformConfiguration::new() };
    unsafe {
        platform.setHardwareModel(&p.hw);
        platform.setMachineIdentifier(&p.machine_id);
        platform.setAuxiliaryStorage(Some(&p.storage));
        config.setPlatform(&platform);
        config.setBootLoader(Some(&VZMacOSBootLoader::new()));
        config.setCPUCount(p.cpus);
        config.setMemorySize(p.memory);
    }
    let disk = vz_mac_config::disk(&p.disk, false, None)?;
    unsafe {
        config.setStorageDevices(&NSArray::from_retained_slice(&[disk]));
        config.setGraphicsDevices(&NSArray::from_retained_slice(&[vz_mac_config::graphics(
            DISPLAY,
        )
        .into_super()]));
        config.setEntropyDevices(&NSArray::from_retained_slice(&[
            VZVirtioEntropyDeviceConfiguration::new().into_super(),
        ]));
        config.setSocketDevices(&NSArray::from_retained_slice(&[
            VZVirtioSocketDeviceConfiguration::new().into_super(),
        ]));
    }
    // An absolute pointer first, so a click of the unattended setup lands
    // at the view point where the screen text is.
    vz_mac_config::input_devices(&config, true);
    if let Some(seed) = &p.seed {
        let fs = vz_mac_config::shares(&[crate::vm::helper::ShareArg {
            name: super::seed::SHARE.to_string(),
            path: seed.clone(),
            read_only: true,
        }])?;
        unsafe { config.setDirectorySharingDevices(&NSArray::from_retained_slice(&[fs])) };
    }
    if let Some(console) = &p.console {
        let serial = vz_mac_config::console(console)?;
        unsafe { config.setSerialPorts(&NSArray::from_retained_slice(&[serial])) };
    }
    match &p.network {
        Some(fd) => {
            use std::os::fd::AsRawFd;
            // A datagram socket whose other end nobody reads: the guest
            // gets a network service, and no frame goes anywhere.
            let dup = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if dup < 0 {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| text::stub_network_failed("dup"));
            }
            let handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
                NSFileHandle::alloc(),
                dup,
                true,
            );
            let attachment = unsafe {
                VZFileHandleNetworkDeviceAttachment::initWithFileHandle(
                    VZFileHandleNetworkDeviceAttachment::alloc(),
                    &handle,
                )
            };
            let net = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
            unsafe {
                net.setAttachment(Some(&attachment));
                config.setNetworkDevices(&NSArray::from_retained_slice(&[net.into_super()]));
            }
        }
        None => unsafe { config.setNetworkDevices(&NSArray::new()) },
    }
    unsafe { config.validateWithError() }
        .map_err(|e| anyhow!(text::configuration_invalid(&describe(&e))))?;
    Ok(config)
}

/// A pair of connected datagram sockets: one for the guest device, one
/// that stays open and unread on the host.
pub(super) fn stub_network() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: a plain socketpair into a 2-element array.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| text::stub_network_failed("make"));
    }
    // SAFETY: both descriptors are new and owned here.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn sw_vers(flag: &str) -> String {
    Command::new("/usr/bin/sw_vers")
        .arg(flag)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub(super) struct DelegateIvars {
    tx: Sender<VmEvent>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AllocAnyThread]
    #[name = "PaddockGoldenDelegate"]
    #[ivars = DelegateIvars]
    pub(super) struct GoldenDelegate;

    unsafe impl NSObjectProtocol for GoldenDelegate {}

    unsafe impl VZVirtualMachineDelegate for GoldenDelegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop(&self, _vm: &VZVirtualMachine) {
            let _ = self.ivars().tx.send(VmEvent::GuestStopped);
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn did_stop_with_error(&self, _vm: &VZVirtualMachine, error: &NSError) {
            let _ = self.ivars().tx.send(VmEvent::Failed(describe(error)));
        }
    }
);

impl GoldenDelegate {
    pub(super) fn new(tx: Sender<VmEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { tx });
        unsafe { msg_send![super(this), init] }
    }
}

impl Drop for VzBackend {
    fn drop(&mut self) {
        self.finish();
    }
}
