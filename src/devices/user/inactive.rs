//! The files of user devices that run nothing: `Disabled`, a device that
//! this sandbox does not get, and `NotYet`, a table whose runtime this
//! paddock does not have. Both still list in the catalog with their
//! details, and a request gets a status instead of a call.

use super::super::config::{DeviceDef, Shape};
use super::super::grant::{how_to_enable, Why};
use super::super::ledger;
use super::super::{Call, DevError, Device, Info};
use super::{details, info};

/// The text of an off device.
fn disabled_text(name: &str, why: Why) -> String {
    match why {
        Why::Off => format!(
            "The device {name} is disabled. The user turns it on with enabled = true in [devices.{name}] of ~/.config/paddock/config.toml on the host."
        ),
        Why::NoGrant | Why::AboveCeiling => format!(
            "The device {name} is not granted to this sandbox: {}.",
            how_to_enable(name, why)
        ),
        Why::NotRemote => format!(
            "The device {name} is not granted to remote callers: {}.",
            how_to_enable(name, why)
        ),
        Why::NoCallerFiles => format!(
            "The device {name} is off for remote callers: {}.",
            how_to_enable(name, why)
        ),
    }
}

/// The response of a file that answers with a status and a message, in
/// the format of the device.
fn answer(def: &DeviceDef, status: &str, text: &str) -> Vec<u8> {
    match def.shape {
        Shape::HttpProxy => super::super::http::paddock_answer(403, "Forbidden", status, text),
        _ => format!("status: {status}\nmessage: {text}\n\n").into_bytes(),
    }
}

/// The file of a device that the user defined but that this sandbox does
/// not get: off, not granted, or above the ceiling of a spawned child.
pub struct Disabled(pub DeviceDef, pub Why);

impl Device for Disabled {
    fn name(&self) -> &str {
        &self.0.name
    }
    fn info(&self) -> Info {
        info(&self.0)
    }
    fn state(&self) -> &'static str {
        "off"
    }
    fn details(&self) -> Option<serde_json::Value> {
        let mut d = details(&self.0);
        if self.0.paid() {
            d["cost"] = ledger::money(self.0.cost).into();
        }
        d["how_to_enable"] = how_to_enable(&self.0.name, self.1).into();
        Some(d)
    }
    fn call(&self, call: &Call, _request: &[u8]) -> Result<Vec<u8>, DevError> {
        let mut text = disabled_text(&self.0.name, self.1);
        // The remote caller reads which grant names it.
        if let Some(peer) = call.sandbox.strip_prefix("remote:") {
            text = text.replace(
                "grant = [\"remote:HOST\"] (the address or name of the calling machine)",
                &format!("grant = [\"remote:{peer}\"]"),
            );
        }
        Ok(answer(&self.0, "disabled", &text))
    }
}

/// The file of a table whose runtime this paddock does not have yet. A
/// request fails with EIO, and the catalog names the reason. Every value
/// of this version is served; the file stays for the next one.
pub struct NotYet(pub DeviceDef, pub String);

impl Device for NotYet {
    fn name(&self) -> &str {
        &self.0.name
    }
    fn info(&self) -> Info {
        info(&self.0)
    }
    fn state(&self) -> &'static str {
        "unavailable"
    }
    fn details(&self) -> Option<serde_json::Value> {
        let mut d = details(&self.0);
        d["how_to_fix"] = self.1.clone().into();
        Some(d)
    }
    fn call(&self, _call: &Call, _request: &[u8]) -> Result<Vec<u8>, DevError> {
        Err(DevError::Failed)
    }
}
