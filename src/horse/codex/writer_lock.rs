//! The writer lock of a Codex thread (`thread-writer-locks/<id>.lock`).
//!
//! `flock` belongs to the open file, not to the process. A process that
//! forks copies its open files into the child, and the child holds them
//! until it runs `exec`, which closes them. So a lock can stay taken for a
//! few milliseconds after its owner closed the file: a thread of paddock
//! that starts a process at that time holds a copy. Codex also reports
//! `thread/closed` around the time it drops the lock. A short wait covers
//! both, and a lock that a writer really holds is still refused.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

/// The longest wait for a writer lock that is taken.
pub const WAIT: Duration = Duration::from_secs(1);

/// Take the lock on `f` without blocking, and try again until `wait`
/// ends. False when the lock is still taken.
pub fn take(f: &File, wait: Duration) -> bool {
    let end = Instant::now() + wait;
    loop {
        // SAFETY: flock on an fd that `f` owns; the lock goes with the file.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
