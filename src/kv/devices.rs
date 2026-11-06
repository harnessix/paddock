//! The `devices` op of the kv socket: `paddock device stop|restart NAME
//! --sandbox ID` reaches the device instances of a run (devices/sandbox.rs)
//! through the paddock process that owns them.

use super::{Request, Response, Service, Status};
use std::sync::Arc;

/// Stop, restart or list the device instances of a run: `(action, name)`.
pub type DeviceControl =
    Arc<dyn Fn(&str, &str) -> std::result::Result<serde_json::Value, String> + Send + Sync>;

impl Service {
    /// The `devices` op: stop, restart or status of a device instance of
    /// this run. Without a control the op says so.
    pub(super) fn devices_op(&self, req: &Request) -> Response {
        let action = req.action.as_deref().unwrap_or("status");
        if !matches!(action, "stop" | "restart" | "status") {
            return Response::new(
                Status::Invalid,
                format!("unknown devices action '{action}': use stop, restart or status"),
            );
        }
        let name = req.key.as_deref().unwrap_or("");
        if action != "status" && name.is_empty() {
            return Response::new(Status::Invalid, "devices: name the device in key");
        }
        let ctl = self
            .devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(ctl) = ctl else {
            return Response::new(
                Status::Disabled,
                "this run has no device instances to control",
            );
        };
        match ctl(action, name) {
            Ok(v) => Response::done(v),
            Err(m) => Response::new(Status::Failed, m),
        }
    }
}
