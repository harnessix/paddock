//! Synchronize the guest wall clock with the host using the guest's sudo password.
use super::configure::{Plan, Ran, SUDO};
use std::time::Duration;

pub const TOLERANCE_MS: u64 = 2_000;
const LIMIT: Duration = Duration::from_secs(15);

pub fn needs_set(guest_ms: u64, host_ms: u64) -> bool {
    host_ms != 0 && guest_ms.abs_diff(host_ms) > TOLERANCE_MS
}

/// Format a UTC date in the format accepted by macOS /bin/date.
pub fn date_arg(ms: u64) -> Result<String, String> {
    let seconds = libc::time_t::try_from(ms / 1000).map_err(|_| "clock time is out of range")?;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers are valid, and gmtime_r initializes tm on success.
    if unsafe { libc::gmtime_r(&seconds, tm.as_mut_ptr()) }.is_null() {
        return Err("clock time is out of range".into());
    }
    // SAFETY: gmtime_r succeeded.
    let tm = unsafe { tm.assume_init() };
    let year = tm.tm_year + 1900;
    if !(1970..=9999).contains(&year) {
        return Err("clock time is out of range".into());
    }
    Ok(format!(
        "{:02}{:02}{:02}{:02}{:04}.{:02}",
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        year,
        tm.tm_sec
    ))
}

/// Read the guest clock when the request is handled, not at the handshake.
pub fn follow_host(
    host_ms: u64,
    password: &str,
    spawn: impl FnOnce(&Plan, Option<&str>, Duration) -> std::io::Result<Ran>,
) -> Result<(), String> {
    let guest_ms = crate::macvm::proto::now_ms();
    if !needs_set(guest_ms, host_ms) {
        return Ok(());
    }
    if password.is_empty() {
        return Err("clock.set needs the guest password".into());
    }
    let date = date_arg(host_ms)?;
    let plan = Plan {
        argv: vec![
            SUDO.into(),
            "-S".into(),
            "-p".into(),
            "".into(),
            "--".into(),
            "/bin/date".into(),
            "-u".into(),
            date,
        ],
        password_on_stdin: true,
    };
    log::info!(
        "clock: guest was {} ms {} the host",
        guest_ms.abs_diff(host_ms),
        if guest_ms < host_ms {
            "behind"
        } else {
            "ahead of"
        }
    );
    // Do not expose subprocess output or errors: they could contain the password.
    let ran = spawn(&plan, Some(password), LIMIT)
        .map_err(|_| "clock.set could not start sudo".to_string())?;
    if ran.timed_out {
        return Err("clock.set timed out".into());
    }
    if !ran.ok() {
        return Err(crate::macvm::text::clock_set_failed(ran.code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tolerance_and_utc_dates() {
        assert!(!needs_set(10_000, 12_000));
        assert!(needs_set(10_000, 12_001));
        assert!(!needs_set(10_000, 0));
        assert_eq!(date_arg(0).unwrap(), "010100001970.00");
        assert_eq!(date_arg(1_704_067_199_999).unwrap(), "123123592023.59");
        assert_eq!(date_arg(1_709_164_800_000).unwrap(), "022900002024.00");
        assert!(date_arg(u64::MAX).is_err());
    }
    #[test]
    fn password_only_goes_to_stdin_and_errors_are_clean() {
        let secret = "planted-password";
        let err = follow_host(1, secret, |plan, stdin, limit| {
            assert_eq!(stdin, Some(secret));
            assert_eq!(limit, LIMIT);
            assert!(plan.password_on_stdin);
            assert_eq!(&plan.argv[5..], ["/bin/date", "-u", "010100001970.00"]);
            assert!(!plan.argv.join(" ").contains(secret));
            Err(std::io::Error::other(secret))
        })
        .unwrap_err();
        assert!(!err.contains(secret));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    #[test]
    fn macos_date_accepts_the_utc_argument() {
        let arg = super::date_arg(1_709_164_800_000).unwrap();
        let out = std::process::Command::new("/bin/date")
            .args(["-j", "-u", "-f", "%m%d%H%M%Y.%S", &arg, "+%s"])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1709164800");
    }
}
