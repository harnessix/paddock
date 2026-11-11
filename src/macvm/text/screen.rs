//! The texts of the guest screen lock and of the login keychain of the
//! guest: the lines of the session (`run_screen.rs`), the replies of the
//! agent (`guest/main_loop_screen.rs`, `guest/keychain.rs`). No text holds
//! the password.

/// The platform of the agent cannot lock the screen (`bench.lock`).
pub fn screen_lock_unsupported() -> String {
    "this agent cannot lock the guest screen on this system".into()
}

/// The private call that locks the screen is missing in the guest.
pub fn screen_lock_missing(what: &str) -> String {
    format!("cannot lock the guest screen: {what} is not in this macOS")
}

/// The event of a lock: when, the last input in the guest, the uptime.
pub fn screen_locked(at: &str, idle_secs: Option<u64>, uptime: u64) -> String {
    let idle = match idle_secs {
        Some(s) => format!("the last input in the guest was {s} s before"), // not user text
        None => "the time of the last input in the guest is not known".into(),
    };
    format!("the guest screen locked at {at}: {idle}, the guest was up {uptime} s")
}

/// paddock types the password into the locked screen.
pub fn screen_typing(attempt: u32, of: u32) -> String {
    format!("typing the guest password into the locked guest screen, try {attempt} of {of}")
}

/// The screen unlocked: by paddock after some tries, or by someone else.
pub fn screen_unlocked(tries: u32) -> String {
    if tries == 0 {
        "the guest screen is unlocked".into()
    } else {
        format!("the guest screen is unlocked after {tries} tries of the guest password")
    }
}

/// The one line after the last try.
pub fn screen_stays_locked(tries: u32) -> String {
    format!(
        "The guest screen is still locked after {tries} tries of the guest password. Click into the app window and type the password of the guest account (the one of `paddock macvm setup`) to unlock it."
    )
}

/// No password reached the session, so paddock cannot unlock the screen.
pub fn screen_no_password() -> String {
    "The guest screen is locked, and this session has no guest password to unlock it. Click into the app window and type the password of the guest account.".into()
}

/// The password has a character with no key: nothing is typed. The
/// character is not named, as it is part of the password.
pub fn screen_cannot_type() -> String {
    "The guest screen is locked, and the guest password has a character with no key on the US layout, so paddock cannot type it. Click into the app window and type the password yourself.".into()
}

/// The private keyboard of the framework is missing.
pub fn screen_no_keyboard() -> String {
    "The guest screen is locked, and this macOS has no private keyboard of the VM for paddock to type the password with. Click into the app window and type the password yourself.".into()
}

/// The screen unlocked, or the report went stale, while paddock typed.
pub fn screen_typing_stopped() -> String {
    "paddock stopped typing the password: the guest screen is no longer reported locked".into()
}

/// The platform of the agent has no keychain calls.
pub fn keychain_unsupported() -> String {
    "this agent cannot reach the login keychain on this system".into()
}

/// The login keychain file is not in the home.
pub fn keychain_missing(path: &std::path::Path) -> String {
    format!("there is no login keychain at {}", path.display())
}

/// A call of the Security framework failed.
pub fn keychain_call_failed(call: &str, status: i32) -> String {
    format!("{call} of the login keychain failed with the Security error {status}")
}

/// The unlock went through, but the keychain still says locked.
pub fn keychain_still_locked(path: &std::path::Path) -> String {
    format!(
        "the login keychain {} is still locked: the guest password does not open it",
        path.display()
    )
}

/// What `keychain.unlock` did.
pub fn keychain_done(
    path: &std::path::Path,
    was_locked: bool,
    list: bool,
    default: bool,
) -> String {
    let mut s = format!(
        "the login keychain {} {}",
        path.display(),
        if was_locked {
            "is unlocked now"
        } else {
            "was unlocked"
        }
    );
    if list {
        s.push_str(", the search list is set to it");
    }
    if default {
        s.push_str(", the default keychain is set to it");
    }
    s
}

/// The keychain of the guest did not unlock: what an app may show.
pub fn keychain_failed(detail: &str) -> String {
    format!(
        "The login keychain of the guest did not unlock: {detail}. An app may ask for the password of the \"login\" keychain: type the password of the guest account there."
    )
}

/// The session has no password for the keychain.
pub fn keychain_no_password() -> String {
    "the session has no guest password: the login keychain of the guest stays as it is".into()
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
