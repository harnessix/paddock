//! The login keychain of the guest account after the home moved onto the
//! data disk. securityd keeps the keychain of the old path open, so after
//! the move the login keychain is locked and an app asks for the
//! "login" keychain password. On `keychain.unlock` the agent unlocks it
//! with the guest password through `SecKeychainUnlock` (the password is
//! never on a command line), and makes the search list and the default
//! keychain of the user name the keychain in the moved home
//! (`security list-keychains -d user -s`, `security default-keychain -d
//! user -s`; no secret in those). The parsers and the decisions are pure
//! and tested on Linux; the framework calls are macOS only.

use std::path::{Path, PathBuf};

/// The login keychain under a home.
pub const LOGIN_KEYCHAIN: &str = "Library/Keychains/login.keychain-db";

/// The login keychain of a home.
pub fn login_keychain(home: &Path) -> PathBuf {
    home.join(LOGIN_KEYCHAIN)
}

/// The home of the account: the one on the data disk once the home moved
/// there, else the home of the environment, else `/Users/paddock`.
pub fn account_home(moved: Option<&Path>, env_home: Option<&str>) -> PathBuf {
    match (moved, env_home) {
        (Some(m), _) => m.to_path_buf(),
        (None, Some(h)) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(super::data::USER_HOME),
    }
}

/// The paths of the output of `security list-keychains` or `security
/// default-keychain`: one quoted path per line.
pub fn parse_list(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let t = l.trim();
            let t = t.strip_prefix('"')?.strip_suffix('"')?;
            (!t.is_empty()).then(|| t.to_string())
        })
        .collect()
}

/// A login keychain of any home: `login.keychain` or `login.keychain-db`.
pub fn is_login_keychain(path: &str) -> bool {
    matches!(
        Path::new(path).file_name().and_then(|n| n.to_str()),
        Some("login.keychain" | "login.keychain-db")
    )
}

/// The search list of the user with `want` first and no other login
/// keychain, the other keychains kept in their order. None when the list
/// is that already.
pub fn search_list(have: &[String], want: &str) -> Option<Vec<String>> {
    let mut list = vec![want.to_string()];
    list.extend(
        have.iter()
            .filter(|p| p.as_str() != want && !is_login_keychain(p))
            .cloned(),
    );
    (list != have).then_some(list)
}

/// The default keychain must be set: it is not `want`.
pub fn default_needs_set(have: &[String], want: &str) -> bool {
    have.first().map(String::as_str) != Some(want)
}

/// `security` with these arguments, as the user, without stdin.
pub fn security_argv(args: &[&str]) -> Vec<String> {
    std::iter::once("/usr/bin/security")
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

#[cfg(target_os = "macos")]
pub use mac::{info, unlock};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::guest::configure::{spawn, Plan};
    use crate::guest::data;
    use crate::guest::platform::Fail;
    use crate::macvm::proto::ErrorCode;
    use crate::macvm::text;
    use std::ffi::{c_char, c_void, CString};
    use std::os::unix::ffi::OsStrExt;

    type SecKeychainRef = *mut c_void;

    // The signatures of <Security/SecKeychain.h>: OSStatus is SInt32,
    // SecKeychainStatus is UInt32, Boolean is unsigned char.
    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecKeychainOpen(path_name: *const c_char, keychain: *mut SecKeychainRef) -> i32;
        fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
        fn SecKeychainUnlock(
            keychain: SecKeychainRef,
            password_length: u32,
            password: *const c_void,
            use_password: u8,
        ) -> i32;
        fn SecKeychainSetUserInteractionAllowed(state: u8) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    /// `kSecUnlockStateStatus`.
    const UNLOCK_STATE: u32 = 1;

    /// One opened keychain, released on drop.
    pub(super) struct Keychain(SecKeychainRef);

    impl Drop for Keychain {
        fn drop(&mut self) {
            // SAFETY: the reference came from SecKeychainOpen (the Create rule).
            unsafe { CFRelease(self.0 as *const c_void) };
        }
    }

    impl Keychain {
        pub(super) fn open(path: &Path) -> Result<Self, Fail> {
            let c = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| (ErrorCode::Io, text::keychain_missing(path)))?;
            let mut kc: SecKeychainRef = std::ptr::null_mut();
            // SAFETY: a NUL-terminated path and an out pointer.
            let st = unsafe { SecKeychainOpen(c.as_ptr(), &mut kc) };
            if st != 0 || kc.is_null() {
                return Err((
                    ErrorCode::Io,
                    text::keychain_call_failed("SecKeychainOpen", st),
                ));
            }
            Ok(Keychain(kc))
        }

        pub(super) fn unlocked(&self) -> Result<bool, Fail> {
            let mut status: u32 = 0;
            // SAFETY: a valid keychain and an out pointer.
            let st = unsafe { SecKeychainGetStatus(self.0, &mut status) };
            if st != 0 {
                return Err((
                    ErrorCode::Io,
                    text::keychain_call_failed("SecKeychainGetStatus", st),
                ));
            }
            Ok(status & UNLOCK_STATE != 0)
        }

        /// Unlock with the password, with no dialog of the system.
        pub(super) fn unlock(&self, password: &str) -> Result<(), Fail> {
            let bytes = password.as_bytes();
            // SAFETY: plain calls; the password buffer lives for the call.
            let st = unsafe {
                SecKeychainSetUserInteractionAllowed(0);
                let st = SecKeychainUnlock(
                    self.0,
                    bytes.len() as u32,
                    bytes.as_ptr() as *const c_void,
                    1,
                );
                SecKeychainSetUserInteractionAllowed(1);
                st
            };
            if st != 0 {
                return Err((
                    ErrorCode::Io,
                    text::keychain_call_failed("SecKeychainUnlock", st),
                ));
            }
            Ok(())
        }
    }

    /// The home of the account now.
    fn home() -> PathBuf {
        let moved =
            (data::mounted() && Path::new(data::HOME).is_dir()).then(|| Path::new(data::HOME));
        account_home(moved, std::env::var("HOME").ok().as_deref())
    }

    /// `security ARGS` as the user: the output, or the failure text.
    fn security(args: &[&str]) -> Result<String, String> {
        let plan = Plan {
            argv: security_argv(args),
            password_on_stdin: false,
        };
        match spawn(&plan, None) {
            Ok((true, out)) => Ok(out),
            Ok((false, out)) => Err(text::command_failed(&plan.argv.join(" "), &out)),
            Err(e) => Err(text::cannot_run_command(
                &plan.argv.join(" "),
                &e.to_string(),
            )),
        }
    }

    /// The search list and the default keychain name `want`: whether each
    /// was set. A failure is logged and leaves that one as it is.
    fn point_at(want: &str) -> (bool, bool) {
        let list = match security(&["list-keychains", "-d", "user"]) {
            Ok(out) => search_list(&parse_list(&out), want),
            Err(e) => {
                log::warn!("keychain: {e}");
                None
            }
        };
        let list_set = match list {
            Some(l) => {
                let mut args = vec!["list-keychains", "-d", "user", "-s"];
                args.extend(l.iter().map(String::as_str));
                security(&args)
                    .map_err(|e| log::warn!("keychain: {e}"))
                    .is_ok()
            }
            None => false,
        };
        let default_set = match security(&["default-keychain", "-d", "user"]) {
            Ok(out) if default_needs_set(&parse_list(&out), want) => {
                security(&["default-keychain", "-d", "user", "-s", want])
                    .map_err(|e| log::warn!("keychain: {e}"))
                    .is_ok()
            }
            Ok(_) => false,
            Err(e) => {
                log::warn!("keychain: {e}");
                false
            }
        };
        (list_set, default_set)
    }

    /// `keychain.unlock`: unlock the login keychain of the home with the
    /// password when it is locked, then point the search list and the
    /// default keychain at it.
    pub fn unlock(password: &str) -> Result<String, Fail> {
        let path = login_keychain(&home());
        if !path.is_file() {
            return Err((ErrorCode::Io, text::keychain_missing(&path)));
        }
        let kc = Keychain::open(&path)?;
        let was_locked = !kc.unlocked()?;
        if was_locked {
            kc.unlock(password)?;
            if !kc.unlocked()? {
                return Err((ErrorCode::Io, text::keychain_still_locked(&path)));
            }
        }
        let (list, default) = point_at(&path.to_string_lossy());
        Ok(text::keychain_done(&path, was_locked, list, default))
    }

    /// `bench.keychain`: `status unlocked PATH` or `status locked PATH`,
    /// the default keychain, the search list, and the output of `security
    /// show-keychain-info` when the keychain is unlocked (on a locked one
    /// it would ask for the password).
    pub fn info() -> Result<String, Fail> {
        let path = login_keychain(&home());
        if !path.is_file() {
            return Err((ErrorCode::Io, text::keychain_missing(&path)));
        }
        let unlocked = Keychain::open(&path)?.unlocked()?;
        let shown = path.to_string_lossy().into_owned();
        let state = if unlocked { "unlocked" } else { "locked" };
        let mut lines = vec![format!("status {state} {shown}")]; // not user text
        for (name, args) in [
            ("default", ["default-keychain", "-d", "user"]),
            ("search", ["list-keychains", "-d", "user"]),
        ] {
            let out = security(&args).unwrap_or_else(|e| e);
            lines.push(format!("{name} {}", parse_list(&out).join(" "))); // not user text
        }
        if unlocked {
            let out = security(&["show-keychain-info", &shown]).unwrap_or_else(|e| e);
            lines.push(format!("info {}", out.trim())); // not user text
        }
        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
#[path = "keychain_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "keychain_macos_tests.rs"]
mod macos_tests;
