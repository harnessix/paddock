//! The Virtualization framework configuration of a macOS guest, macOS
//! only. One function per device, every error with the `describe` text of
//! `vz.rs`. The session (`run.rs`) and the golden build (`vz_mac.rs`)
//! both build their configuration from these parts.

use super::proto::DATA_DISK_SERIAL;
use super::session::{Display, Profile};
use super::text;
use crate::vm::helper::{AppPlan, ShareArg};
use crate::vm::vz::{describe, url};
use anyhow::{anyhow, bail, Context as _, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObjectProtocol};
use objc2::{sel, AllocAnyThread};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSFileHandle, NSString};
use objc2_virtualization::*;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixDatagram;
use std::path::Path;

/// The pixels per inch of the guest display: the values of Apple's
/// sample app, 80 for a plain display and 226 for a Retina one, so the
/// guest renders at the scale the host windows show.
fn pixels_per_inch(scale: u32) -> isize {
    if scale >= 2 {
        226
    } else {
        80
    }
}

/// The hardware model of the golden, checked against this host.
pub fn hardware_model(bytes: &[u8]) -> Result<Retained<VZMacHardwareModel>> {
    let data = NSData::with_bytes(bytes);
    let model = unsafe {
        VZMacHardwareModel::initWithDataRepresentation(VZMacHardwareModel::alloc(), &data)
    }
    .ok_or_else(|| anyhow!(text::hardware_model_invalid()))?;
    if !unsafe { model.isSupported() } {
        bail!(text::hardware_model_unsupported_here());
    }
    Ok(model)
}

/// A new machine identifier, as its data representation.
pub fn new_machine_id() -> Result<Vec<u8>> {
    let id = unsafe { VZMacMachineIdentifier::new() };
    Ok(unsafe { id.dataRepresentation() }.to_vec())
}

/// The machine identifier of a profile.
pub fn machine_id(bytes: &[u8], path: &Path) -> Result<Retained<VZMacMachineIdentifier>> {
    let data = NSData::with_bytes(bytes);
    unsafe {
        VZMacMachineIdentifier::initWithDataRepresentation(VZMacMachineIdentifier::alloc(), &data)
    }
    .ok_or_else(|| anyhow!(text::machine_id_damaged(path)))
}

/// The platform: the hardware model, the machine identifier and the
/// auxiliary storage of the profile.
pub fn platform(
    profile: &Profile,
    model: &VZMacHardwareModel,
    id: &VZMacMachineIdentifier,
) -> Result<Retained<VZMacPlatformConfiguration>> {
    let platform = unsafe { VZMacPlatformConfiguration::new() };
    let aux_url = url(&profile.aux())?;
    let aux =
        unsafe { VZMacAuxiliaryStorage::initWithURL(VZMacAuxiliaryStorage::alloc(), &aux_url) };
    unsafe {
        platform.setHardwareModel(model);
        platform.setMachineIdentifier(id);
        platform.setAuxiliaryStorage(Some(&aux));
    }
    Ok(platform)
}

/// One display of the given pixels and scale.
pub fn graphics(display: Display) -> Retained<VZMacGraphicsDeviceConfiguration> {
    let dev = unsafe { VZMacGraphicsDeviceConfiguration::new() };
    let disp = unsafe {
        VZMacGraphicsDisplayConfiguration::initWithWidthInPixels_heightInPixels_pixelsPerInch(
            VZMacGraphicsDisplayConfiguration::alloc(),
            display.w as isize,
            display.h as isize,
            pixels_per_inch(display.scale),
        )
    };
    unsafe { dev.setDisplays(&NSArray::from_retained_slice(&[disp])) };
    dev
}

/// The trackpad, and the Mac keyboard on macOS 14 or newer, else the USB
/// keyboard. With `absolute`, a USB screen-coordinate pointer comes
/// before the trackpad: a mouse event of the host then lands at its view
/// point in the guest.
pub fn input_devices(config: &VZVirtualMachineConfiguration, absolute: bool) {
    let trackpad = unsafe { VZMacTrackpadConfiguration::new() };
    let mut pointers: Vec<Retained<VZPointingDeviceConfiguration>> = Vec::new();
    if absolute {
        pointers
            .push(unsafe { VZUSBScreenCoordinatePointingDeviceConfiguration::new() }.into_super());
    }
    pointers.push(trackpad.into_super());
    let keyboard: Retained<VZKeyboardConfiguration> =
        if AnyClass::get(c"VZMacKeyboardConfiguration").is_some() {
            unsafe { VZMacKeyboardConfiguration::new() }.into_super()
        } else {
            unsafe { VZUSBKeyboardConfiguration::new() }.into_super()
        };
    unsafe {
        config.setPointingDevices(&NSArray::from_retained_slice(&pointers));
        config.setKeyboards(&NSArray::from_retained_slice(&[keyboard]));
    }
}

/// A virtio block device for a raw disk image. `system.img` uses no
/// synchronization: it is a disposable clone. `data.img` uses `.full`
/// with the host cache, so a guest `fsync` reaches the host disk, and the
/// serial `paddock-data`, so the agent finds it.
pub fn disk(
    path: &Path,
    full_sync: bool,
    serial: Option<&str>,
) -> Result<Retained<VZStorageDeviceConfiguration>> {
    let path_url = url(path)?;
    let (caching, sync) = if full_sync {
        (
            VZDiskImageCachingMode::Cached,
            VZDiskImageSynchronizationMode::Full,
        )
    } else {
        (
            VZDiskImageCachingMode::Automatic,
            VZDiskImageSynchronizationMode::None,
        )
    };
    let attachment = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &path_url,
            false,
            caching,
            sync,
        )
    }
    .map_err(|e| anyhow!(text::cannot_open_for_vm(path, &describe(&e))))?;
    let dev = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        )
    };
    if let Some(serial) = serial {
        let s = NSString::from_str(serial);
        if dev.respondsToSelector(sel!(setBlockDeviceIdentifier:)) {
            unsafe { VZVirtioBlockDeviceConfiguration::validateBlockDeviceIdentifier_error(&s) }
                .map_err(|e| anyhow!(text::disk_serial_invalid(serial, &describe(&e))))?;
            unsafe { dev.setBlockDeviceIdentifier(&s) };
        } else {
            log::warn!(
                "macvm: this macOS cannot name a disk: the agent finds the data disk by size"
            );
        }
    }
    Ok(dev.into_super())
}

/// The virtiofs device with the automount tag and every share by name.
pub fn shares(shares: &[ShareArg]) -> Result<Retained<VZDirectorySharingDeviceConfiguration>> {
    let mut keys: Vec<Retained<NSString>> = Vec::new();
    let mut dirs: Vec<Retained<VZSharedDirectory>> = Vec::new();
    for s in shares {
        let name = NSString::from_str(&s.name);
        unsafe { VZMultipleDirectoryShare::validateName_error(&name) }
            .map_err(|e| anyhow!(text::share_name_invalid(&s.name, &describe(&e))))?;
        let dir_url = url(&s.path)?;
        let dir = unsafe {
            VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &dir_url,
                s.read_only,
            )
        };
        keys.push(name);
        dirs.push(dir);
    }
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let dict: Retained<NSDictionary<NSString, VZSharedDirectory>> =
        NSDictionary::from_retained_objects(&key_refs, &dirs);
    let share = unsafe {
        VZMultipleDirectoryShare::initWithDirectories(VZMultipleDirectoryShare::alloc(), &dict)
    };
    let tag = unsafe { VZVirtioFileSystemDeviceConfiguration::macOSGuestAutomountTag() };
    let fs = unsafe {
        VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        )
    };
    unsafe { fs.setShare(Some(&share.into_super())) };
    Ok(fs.into_super())
}

/// The serial console of the guest, appended to `logs/console.log`.
pub fn console(path: &Path) -> Result<Retained<VZSerialPortConfiguration>> {
    let console_url = url(path)?;
    let attachment = unsafe {
        VZFileSerialPortAttachment::initWithURL_append_error(
            VZFileSerialPortAttachment::alloc(),
            &console_url,
            true,
        )
    }
    .map_err(|e| anyhow!(text::cannot_open_for_vm(path, &describe(&e))))?;
    let serial = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };
    unsafe { serial.setAttachment(Some(&attachment.into_super())) };
    Ok(serial.into_super())
}

/// Check the CPU and memory limits of this Mac.
pub fn check_limits(
    config: &VZVirtualMachineConfiguration,
    cpus: u32,
    memory_bytes: u64,
) -> Result<()> {
    let cpus = cpus as usize;
    let (min_cpu, max_cpu) = unsafe {
        (
            VZVirtualMachineConfiguration::minimumAllowedCPUCount(),
            VZVirtualMachineConfiguration::maximumAllowedCPUCount(),
        )
    };
    if cpus < min_cpu || cpus > max_cpu {
        bail!(text::cpus_out_of_range(cpus, min_cpu, max_cpu));
    }
    let (min_mem, max_mem) = unsafe {
        (
            VZVirtualMachineConfiguration::minimumAllowedMemorySize(),
            VZVirtualMachineConfiguration::maximumAllowedMemorySize(),
        )
    };
    if memory_bytes < min_mem || memory_bytes > max_mem {
        bail!(text::memory_out_of_range(
            memory_bytes >> 20,
            min_mem >> 20,
            max_mem >> 20
        ));
    }
    unsafe {
        config.setCPUCount(cpus);
        config.setMemorySize(memory_bytes);
    }
    Ok(())
}

/// The stub network device: a virtio network device
/// on one end of a datagram socket pair, MTU 1500, with the guest MAC of
/// `stubnet`. The other end comes back for the pump of the stub.
pub fn network_device() -> Result<(Retained<VZVirtioNetworkDeviceConfiguration>, UnixDatagram)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: a plain socketpair into a 2-element array.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| text::stub_network_failed("make"));
    }
    // SAFETY: both descriptors are new and owned here.
    let (guest_end, host_end) =
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        guest_end.into_raw_fd(),
        true,
    );
    let attachment = unsafe {
        VZFileHandleNetworkDeviceAttachment::initWithFileHandle(
            VZFileHandleNetworkDeviceAttachment::alloc(),
            &handle,
        )
    };
    let net = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
    let mac = unsafe {
        VZMACAddress::initWithString(
            VZMACAddress::alloc(),
            &NSString::from_str(super::stubnet::GUEST_MAC_TEXT),
        )
    }
    .ok_or_else(|| anyhow!(text::stub_network_failed("address")))?;
    unsafe {
        attachment.setMaximumTransmissionUnit(super::stubnet::MTU as isize);
        net.setAttachment(Some(&attachment));
        net.setMACAddress(&mac);
    }
    Ok((net, UnixDatagram::from(host_end)))
}

/// The whole configuration of a session: the platform, the boot loader,
/// the display, the input devices, the two disks, vsock, entropy, the
/// balloon, the serial console, the shares, and the stub network device,
/// whose host end comes back for the pump.
pub fn session_configuration(
    plan: &AppPlan,
    hardware_model: &[u8],
    machine_id_bytes: &[u8],
) -> Result<(Retained<VZVirtualMachineConfiguration>, UnixDatagram)> {
    let config = unsafe { VZVirtualMachineConfiguration::new() };
    check_limits(&config, plan.cpus, plan.memory_bytes)?;
    let model = self::hardware_model(hardware_model)?;
    let id = machine_id(machine_id_bytes, &plan.profile.machine_id())?;
    let platform = platform(&plan.profile, &model, &id)?;
    let boot = unsafe { VZMacOSBootLoader::new() };
    unsafe {
        config.setPlatform(&platform.into_super());
        config.setBootLoader(Some(&boot.into_super()));
        config.setGraphicsDevices(&NSArray::from_retained_slice(&[
            graphics(plan.display).into_super()
        ]));
    }
    input_devices(&config, false);
    let system = disk(&plan.profile.system(), false, None)?;
    let data = disk(&plan.profile.data(), true, Some(DATA_DISK_SERIAL))?;
    unsafe { config.setStorageDevices(&NSArray::from_retained_slice(&[system, data])) };
    let fs = shares(&plan.shares)?;
    let serial = console(&plan.profile.console_log())?;
    unsafe {
        config.setDirectorySharingDevices(&NSArray::from_retained_slice(&[fs]));
        config.setSerialPorts(&NSArray::from_retained_slice(&[serial]));
        let vsock = VZVirtioSocketDeviceConfiguration::new();
        config.setSocketDevices(&NSArray::from_retained_slice(&[vsock.into_super()]));
        let entropy = VZVirtioEntropyDeviceConfiguration::new();
        config.setEntropyDevices(&NSArray::from_retained_slice(&[entropy.into_super()]));
        let balloon = VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new();
        config.setMemoryBalloonDevices(&NSArray::from_retained_slice(&[balloon.into_super()]));
    }
    let (net, host_end) = network_device()?;
    unsafe { config.setNetworkDevices(&NSArray::from_retained_slice(&[net.into_super()])) };
    unsafe { config.validateWithError() }
        .map_err(|e| anyhow!(text::configuration_invalid(&describe(&e))))?;
    Ok((config, host_end))
}

/// Whether this configuration can save and restore its state. The error
/// text when it cannot.
pub fn save_restore_supported(config: &VZVirtualMachineConfiguration) -> Result<(), String> {
    unsafe { config.validateSaveRestoreSupportWithError() }.map_err(|e| describe(&e))
}
