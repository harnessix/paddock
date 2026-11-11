//! A child that nobody waits for stays a zombie of its parent until the
//! parent exits, and a long-lived paddock process (the session helper of a
//! macOS guest, the gateway, the bench) gathers one per start. Every
//! `spawn` whose child the caller does not wait for hands it here.

use std::process::Child;

/// Wait for the child in a thread of its own, so it never stays a zombie.
/// When this process exits first, the child goes to init as usual.
pub fn in_background(mut child: Child) {
    let spawned = std::thread::Builder::new()
        .name("reap".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(e) = spawned {
        log::warn!("cannot start a thread to wait for a child: {e}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// The state letter of a process in /proc, or None when it is gone.
    fn state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit(')').next()?.trim().chars().next()
    }

    #[test]
    fn a_child_handed_over_is_reaped_and_a_dropped_one_stays_a_zombie() {
        // The control: a dropped child of `true` stays a zombie.
        let dropped = Command::new("true").spawn().unwrap();
        let pid = dropped.id();
        drop(dropped);
        let t = Instant::now();
        while state(pid) != Some('Z') && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(state(pid), Some('Z'), "the test sees a zombie");
        // SAFETY: waitpid of a child of this process.
        unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) };
        // A child handed over is gone after its end.
        let child = Command::new("true").spawn().unwrap();
        let pid = child.id();
        in_background(child);
        let t = Instant::now();
        while state(pid).is_some() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(state(pid), None, "reaped");
    }
}
