//! Start a program that this process just wrote.
//!
//! `ETXTBSY` ("Text file busy"): another thread of this process forked
//! while the file was still open for writing, and the child holds that
//! descriptor until it execs. The file itself is whole, so a start that
//! tries again for a short, bounded time succeeds.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// How long [`spawn`] tries again while the file is busy.
const BUSY_WAIT: Duration = Duration::from_secs(5);

/// `cmd.spawn()`, tried again while the program file is busy.
pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    let end = Instant::now() + BUSY_WAIT;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < end => {
                std::thread::sleep(Duration::from_millis(20));
            }
            r => return r,
        }
    }
}
