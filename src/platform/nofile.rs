//! The limit of open files. macOS starts every process of a login with a
//! soft limit of 256, which a device server with its 64 connections, the
//! pipes of its programs and the sockets of a run can reach. paddock
//! raises the soft limit to the hard one, at most [`CAP`]. Linux starts
//! with 1024 and paddock leaves it, because programs that close every
//! descriptor up to the limit get slow with a high one.

/// `OPEN_MAX` of macOS: `setrlimit` refuses a higher soft limit there
/// even when the hard limit is unlimited.
pub const CAP: u64 = 10240;

/// The new soft limit for `soft` and `hard`, or None when `soft` is high
/// enough already. Never above `hard` or `cap`, never below `soft`.
pub fn target(soft: u64, hard: u64, cap: u64) -> Option<u64> {
    let want = hard.min(cap);
    (want > soft).then_some(want)
}

/// Raise the soft limit of open files of this process (macOS only; a
/// no-op on Linux). A failure is logged and changes nothing.
pub fn raise() {
    #[cfg(target_os = "macos")]
    {
        let mut l = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit writes the struct that it gets.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) } != 0 {
            log::debug!("getrlimit: {}", std::io::Error::last_os_error());
            return;
        }
        let Some(want) = target(l.rlim_cur, l.rlim_max, CAP) else {
            return;
        };
        let new = libc::rlimit {
            rlim_cur: want,
            rlim_max: l.rlim_max,
        };
        // SAFETY: setrlimit reads the struct that it gets.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &new) } != 0 {
            log::debug!(
                "setrlimit NOFILE {want}: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_goes_up_to_the_hard_one_within_the_cap() {
        // The macOS default: 256 soft, unlimited hard.
        assert_eq!(target(256, i64::MAX as u64, CAP), Some(10240));
        assert_eq!(target(256, 4096, CAP), Some(4096));
        assert_eq!(target(10240, i64::MAX as u64, CAP), None);
        assert_eq!(target(20000, i64::MAX as u64, CAP), None, "never lower");
        assert_eq!(target(256, 256, CAP), None);
    }
}
