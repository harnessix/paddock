//! The provisioning options of macOS 27 provisioning, macOS only.
//!
//! `VZMacGuestProvisioningOptions` and
//! `-[VZMacOSVirtualMachineStartOptions setGuestProvisioningOptions:error:]`
//! are newer than the bindings of `objc2-virtualization`, so this file
//! reaches them at run time: the class by name, each method after a
//! `respondsToSelector:` check. A Mac without them gets the guided
//! window (`provision.rs` decides).
//!
//! The password goes only into the options object. An error of the
//! framework is shown as macOS wrote it, with the password removed in
//! case macOS ever quotes it.

use super::golden::GUEST_USER;
use super::provision::{check_options, redact, Secret, Why, FULL_NAME};
use super::vz_mac::describe;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSError, NSString};
use objc2_virtualization::VZMacOSVirtualMachineStartOptions;

const OPTIONS: &std::ffi::CStr = c"VZMacGuestProvisioningOptions";
const START: &std::ffi::CStr = c"VZMacOSVirtualMachineStartOptions";

/// The setters that the options object must answer.
fn setters() -> [Sel; 6] {
    [
        sel!(setFullName:),
        sel!(setUsername:),
        sel!(setPassword:),
        sel!(setLogsInAutomatically:),
        sel!(setEnablesRemoteLogin:),
        sel!(validateWithError:),
    ]
}

/// The framework check: `Err` with the name of what this Mac lacks.
pub(super) fn support() -> Result<(), String> {
    let Some(cls) = AnyClass::get(OPTIONS) else {
        return Err("VZMacGuestProvisioningOptions".into());
    };
    for s in setters() {
        if !cls.responds_to(s) {
            let name = s.name().to_string_lossy();
            let method = format!("-[VZMacGuestProvisioningOptions {name}]"); // not user text
            return Err(method);
        }
    }
    let Some(start) = AnyClass::get(START) else {
        return Err("VZMacOSVirtualMachineStartOptions".into());
    };
    if !start.responds_to(sel!(setGuestProvisioningOptions:error:)) {
        return Err(
            "-[VZMacOSVirtualMachineStartOptions setGuestProvisioningOptions:error:]".into(),
        );
    }
    Ok(())
}

/// The start options with the provisioning options of the account
/// `paddock`: automatic login on, Remote Login off (the guest has no
/// network that the host reaches). `Err` when this Mac lacks the API or
/// macOS refuses a value; the guided window then boots the guest without
/// options, so nothing used up the first boot.
pub(super) fn start_options(
    password: &str,
) -> Result<Retained<VZMacOSVirtualMachineStartOptions>, Why> {
    support().map_err(Why::Missing)?;
    check_options(FULL_NAME, GUEST_USER, password)?;
    let secret = Secret::new(password);
    let refused = |e: &NSError| Why::Refused(redact(&describe(e), &secret));
    let cls = AnyClass::get(OPTIONS)
        .ok_or_else(|| Why::Missing("VZMacGuestProvisioningOptions".into()))?;
    // SAFETY: the class and every selector were checked above; the
    // setters take an NSString or a BOOL, the validation an NSError out
    // parameter, as the macOS 27 SDK declares them.
    unsafe {
        let opts: Retained<AnyObject> = msg_send![cls, new];
        let _: () = msg_send![&opts, setFullName: &*NSString::from_str(FULL_NAME)];
        let _: () = msg_send![&opts, setUsername: &*NSString::from_str(GUEST_USER)];
        let _: () = msg_send![&opts, setPassword: &*NSString::from_str(password)];
        let _: () = msg_send![&opts, setLogsInAutomatically: Bool::YES];
        let _: () = msg_send![&opts, setEnablesRemoteLogin: Bool::NO];
        let valid: Result<(), Retained<NSError>> = msg_send![&opts, validateWithError: _];
        valid.map_err(|e| refused(&e))?;
        let start = VZMacOSVirtualMachineStartOptions::new();
        let set: Result<(), Retained<NSError>> =
            msg_send![&start, setGuestProvisioningOptions: &*opts, error: _];
        set.map_err(|e| refused(&e))?;
        Ok(start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a macOS 27 host both classes answer every method; on an older
    /// one the check names what is missing, and the options refuse.
    #[test]
    fn support_and_options_agree() {
        match support() {
            Ok(()) => {
                let start = start_options("bcdefghijklnoprstuvx");
                assert!(start.is_ok(), "{:?}", start.err());
                let long = "a".repeat(600);
                assert!(matches!(start_options(&long), Err(Why::Refused(_))));
            }
            Err(name) => {
                assert!(name.contains("VZ"), "{name}");
                assert!(matches!(
                    start_options("bcdefghijklnoprstuvx"),
                    Err(Why::Missing(_))
                ));
            }
        }
    }
}
