//! The `/dev/paddock` FUSE mount. macOS has no native FUSE mount in this build.

use super::{Device, DeviceCtx};
use crate::credentials::secret::SecretFiles;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Placeholder for the Linux file system. `mount` refuses to use it.
#[derive(Clone)]
pub struct DeviceFs {
    _private: (),
}

impl DeviceFs {
    pub fn new(sandbox: &str, devices: Vec<Arc<dyn Device>>) -> Self {
        Self::with_secrets(sandbox, devices, SecretFiles::new(None, None))
    }

    pub fn with_secrets(
        sandbox: &str,
        devices: Vec<Arc<dyn Device>>,
        secrets: Arc<SecretFiles>,
    ) -> Self {
        let _ = (sandbox, devices, secrets);
        Self { _private: () }
    }

    /// The horse tree needs the FUSE mount; this build has none.
    pub fn with_horse(self, setup: Option<crate::horse::device::Access>, ctx: DeviceCtx) -> Self {
        let _ = (setup, ctx);
        self
    }
}

/// A mount that was refused before it started.
pub struct Mounted {
    pub dir: PathBuf,
}

impl Mounted {
    pub fn unmount(self) {}
}

pub fn mount(dir: &Path, fs: DeviceFs) -> Result<Mounted> {
    let _ = (dir, fs);
    Err(crate::platform::unavailable("the FUSE device mount"))
}

pub(super) fn lazy_unmount(dir: &Path) {
    let _ = dir;
}
