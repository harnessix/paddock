//! The restore image and the installer of `paddock macvm setup`: the
//! fetch from Apple, the download with curl, the load and check of a
//! file, and `VZMacOSInstaller` with its progress lines. macOS only.

use super::text::{self, SetupPhase};
use super::vz_mac::{error_text, url, Sendable};
use anyhow::{bail, Context, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::NSError;
use objc2_virtualization::*;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long the fetch of the restore image list and the load of a file may
/// take.
const FRAMEWORK_TIMEOUT: Duration = Duration::from_secs(120);

/// `VZMacOSInstaller` on the queue of the VM, with one line per 5 percent
/// and the time left.
pub(crate) fn run_installer(
    vm: &Sendable<Retained<VZVirtualMachine>>,
    queue: &DispatchRetained<DispatchQueue>,
    ipsw: &Path,
) -> Result<()> {
    let ipsw_url = Sendable(url(ipsw)?);
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let (ptx, prx) = mpsc::channel::<Sendable<Retained<objc2_foundation::NSProgress>>>();
    let vm2 = Sendable(vm.0.clone());
    queue.exec_sync(move || {
        let installer = unsafe {
            VZMacOSInstaller::initWithVirtualMachine_restoreImageURL(
                VZMacOSInstaller::alloc(),
                vm2.get(),
                ipsw_url.get(),
            )
        };
        let done = RcBlock::new(move |err: *mut NSError| {
            let _ = tx.send(error_text(err));
        });
        unsafe { installer.installWithCompletionHandler(&done) };
        let _ = ptx.send(Sendable(unsafe { installer.progress() }));
        // The installer object must live to the end: the block holds the
        // result, the progress object the installer.
        std::mem::forget(installer);
    });
    let progress = prx.recv().with_context(text::installer_no_start)?;
    let begin = Instant::now();
    let mut last = 0u64;
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(())) => {
                println!(
                    "{}",
                    text::progress(SetupPhase::Install, 100, Some(Duration::ZERO))
                );
                return Ok(());
            }
            Ok(Err(e)) => bail!(text::install_failed(&e)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!(text::installer_no_result())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let fraction = progress.0.fractionCompleted().clamp(0.0, 1.0);
        let pct = (fraction * 100.0) as u64 / 5 * 5;
        if pct > last {
            last = pct;
            let elapsed = begin.elapsed().as_secs_f64();
            let left = if fraction > 0.01 {
                Some(Duration::from_secs_f64(
                    elapsed / fraction * (1.0 - fraction),
                ))
            } else {
                None
            };
            println!("{}", text::progress(SetupPhase::Install, pct as u8, left));
        }
    }
}

/// `VZMacOSRestoreImage.fetchLatestSupported`.
pub(crate) fn fetch_latest() -> Result<Sendable<Retained<VZMacOSRestoreImage>>> {
    let (tx, rx) = mpsc::channel::<Result<Sendable<Retained<VZMacOSRestoreImage>>, String>>();
    let block = RcBlock::new(move |img: *mut VZMacOSRestoreImage, err: *mut NSError| {
        // SAFETY: the framework hands a live object, or null with an error.
        let r = match unsafe { Retained::retain(img) } {
            Some(i) => Ok(Sendable(i)),
            None => Err(error_text(err)
                .err()
                .unwrap_or_else(|| "no restore image".into())),
        };
        let _ = tx.send(r);
    });
    unsafe { VZMacOSRestoreImage::fetchLatestSupportedWithCompletionHandler(&block) };
    match rx.recv_timeout(FRAMEWORK_TIMEOUT) {
        Ok(Ok(i)) => Ok(i),
        Ok(Err(e)) => bail!(text::fetch_failed(&e)),
        Err(_) => bail!(text::fetch_timeout()),
    }
}

/// `VZMacOSRestoreImage.load` of a file.
pub(crate) fn load_image(path: &Path) -> Result<Sendable<Retained<VZMacOSRestoreImage>>> {
    let (tx, rx) = mpsc::channel::<Result<Sendable<Retained<VZMacOSRestoreImage>>, String>>();
    let block = RcBlock::new(move |img: *mut VZMacOSRestoreImage, err: *mut NSError| {
        // SAFETY: the framework hands a live object, or null with an error.
        let r = match unsafe { Retained::retain(img) } {
            Some(i) => Ok(Sendable(i)),
            None => Err(error_text(err)
                .err()
                .unwrap_or_else(|| "no restore image".into())),
        };
        let _ = tx.send(r);
    });
    let file_url = url(path)?;
    unsafe { VZMacOSRestoreImage::loadFileURL_completionHandler(&file_url, &block) };
    match rx.recv_timeout(FRAMEWORK_TIMEOUT) {
        Ok(Ok(i)) => Ok(i),
        Ok(Err(e)) => bail!(text::image_unusable(path, &e)),
        Err(_) => bail!(text::image_load_timeout(path)),
    }
}

/// `15.2` and `24C101` of an image.
pub(crate) fn version_of(image: &VZMacOSRestoreImage) -> (String, String) {
    let v = unsafe { image.operatingSystemVersion() };
    let version = if v.patchVersion > 0 {
        format!("{}.{}.{}", v.majorVersion, v.minorVersion, v.patchVersion)
    } else {
        format!("{}.{}", v.majorVersion, v.minorVersion)
    };
    (version, unsafe { image.buildVersion() }.to_string())
}

/// The `Content-Length` of a URL, from a HEAD request with curl.
pub(crate) fn content_length(url: &str) -> Option<u64> {
    let out = Command::new("/usr/bin/curl")
        .args(["-sIL", "--max-time", "60", url])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<u64>().ok())
                .flatten()
        })
}

/// Download with curl into `dest`, continuing a partial file, with one
/// progress line per 5 percent and the time left.
pub(crate) fn download(url: &str, dest: &Path, size: Option<u64>) -> Result<()> {
    println!("{}", text::downloading(url));
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "-L",
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            "5",
            "--retry-delay",
            "5",
            "--retry-all-errors",
            "-C",
            "-",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| text::cannot_run("/usr/bin/curl"))?;
    let begin = Instant::now();
    let start_bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut last = 0u64;
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                bail!(text::download_failed(&status.to_string(), err.trim()));
            }
            println!(
                "{}",
                text::progress(SetupPhase::Download, 100, Some(Duration::ZERO))
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
        if let Some(total) = size.filter(|t| *t > 0) {
            let have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            let pct = (have * 100 / total) / 5 * 5;
            if pct > last {
                last = pct;
                let done = have.saturating_sub(start_bytes) as f64;
                let rate = done / begin.elapsed().as_secs_f64().max(0.001);
                let left =
                    (rate > 0.0).then(|| Duration::from_secs_f64((total - have) as f64 / rate));
                println!("{}", text::progress(SetupPhase::Download, pct as u8, left));
            }
        }
    }
}

pub(crate) fn make_sparse(path: &Path, bytes: u64) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| text::cannot_make(path))?;
    f.set_len(bytes).with_context(|| text::cannot_size(path))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
