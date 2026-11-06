//! The caller of a device request: the headers that name the sandbox,
//! its harness and project, the mount of the project in the device
//! sandbox, and the chain of sandboxes above it.

use crate::devices::config::{CallerMount, DeviceDef, Run, CALLER_PROJECT_MOUNT};
use crate::devices::DeviceCtx;

/// Who calls, for the headers. The broker fills it from the mount, never
/// from the request.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    pub device: String,
    pub sandbox: String,
    pub harness: String,
    pub project: String,
    /// Where the caller's project is mounted in the device sandbox, when
    /// the profile asks for it.
    pub mount: Option<String>,
    /// The sandboxes above the caller, root first.
    pub chain: Vec<String>,
    /// `Paddock-Table`: this run's view of the device table, for a shared
    /// instance that must refuse to serve another one (devices/shared.rs).
    pub table: Option<String>,
}

impl Caller {
    pub fn new(device: &str, ctx: &DeviceCtx) -> Self {
        Self {
            device: device.to_string(),
            sandbox: ctx.sandbox.clone(),
            harness: ctx.harness.clone(),
            project: ctx.project.display().to_string(),
            mount: None,
            chain: ctx.started_by.clone(),
            table: None,
        }
    }

    /// `Paddock-Caller-Mount`: the caller's project in the device sandbox.
    pub fn with_mount(mut self, mount: &str) -> Self {
        self.mount = Some(mount.to_string());
        self
    }

    /// The caller of `def`'s requests: `new`, with the mount of the
    /// caller's project when the sandbox profile of the table has
    /// `caller = ["project:ro"]` (`PADDOCK_CALLER_MOUNT` in the program).
    /// `Paddock-Table`: the view of the table that this run sends.
    pub fn with_table(mut self, table: &str) -> Self {
        self.table = Some(table.to_string());
        self
    }

    pub fn for_device(def: &DeviceDef, ctx: &DeviceCtx) -> Self {
        let mut c = Self::new(&def.name, ctx);
        if def.scope == crate::devices::config::Scope::Shared {
            c = c.with_table(&crate::devices::shared::table_hash(def));
        }
        let mounted =
            def.run == Run::Sandbox && def.profile().caller.contains(&CallerMount::ProjectRo);
        if mounted {
            let at = caller_mount(cfg!(target_os = "macos"), &ctx.project);
            c.with_mount(&at)
        } else {
            c
        }
    }
}

/// Where a device program finds the caller's project: bwrap mounts it at
/// `CALLER_PROJECT_MOUNT`; Seatbelt on macOS cannot move a path, so the
/// program reads the project at its host path.
pub fn caller_mount(macos: bool, project: &std::path::Path) -> String {
    if macos {
        crate::backend::seatbelt::real(project)
            .display()
            .to_string()
    } else {
        CALLER_PROJECT_MOUNT.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_caller_mount_is_the_host_path_on_macos_only() {
        let p = std::path::Path::new("/no/such/project");
        assert_eq!(super::caller_mount(false, p), "/run/paddock/caller/project");
        assert_eq!(super::caller_mount(true, p), "/no/such/project");
    }
}
