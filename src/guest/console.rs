//! The lines of the agent on the serial console of the guest. The host
//! attaches the virtio console of every VM to a file (`logs/console.log`
//! of a session, `console.log` of the golden), but a macOS guest writes
//! nothing there on a normal boot: only the installer does. So the agent
//! writes its start line and the output of `sw_vers` to the console at
//! each start. The console log then shows when the guest came up with
//! which agent and which macOS, also when the control connection never
//! came up, which is when the log is read.
//!
//! The device is the first `/dev/cu.*virtio*` (or `/dev/tty.*virtio*`).
//! The agent opens it with `O_NONBLOCK | O_NOCTTY`, so a console that
//! nobody drains never blocks the agent; a write that would block is
//! retried a few times and then given up. Every failure is one line of
//! the agent log and nothing more.

use crate::macvm::text;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The tries of a write that would block, and the gap between them.
const TRIES: u32 = 20;
const GAP: Duration = Duration::from_millis(10);
/// The most bytes that go to the console at one start.
const MAX: usize = 4096;

/// The console device among the names of `/dev`: a `cu.` device with
/// `virtio` in its name first (it never waits for a carrier), then a
/// `tty.` one. Sorted, so the pick is stable. Pure.
pub fn pick_device(names: &[String]) -> Option<String> {
    let virtio = |n: &&String| n.to_ascii_lowercase().contains("virtio");
    let mut cu: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("cu."))
        .filter(virtio)
        .collect();
    cu.sort();
    if let Some(n) = cu.first() {
        return Some((*n).clone());
    }
    let mut tty: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("tty."))
        .filter(virtio)
        .collect();
    tty.sort();
    tty.first().map(|n| (*n).clone())
}

/// The serial devices that were there, for the line when none fits.
pub fn seen(names: &[String]) -> String {
    let mut v: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| n.starts_with("cu.") || n.starts_with("tty."))
        .collect();
    v.sort_unstable();
    if v.is_empty() {
        return "none".into();
    }
    v.truncate(10);
    v.join(", ")
}

/// The text for the console: the start line, then each line of `sw_vers`
/// (`ProductVersion: 27.2`). At most [`MAX`] bytes. Pure.
pub fn lines(start: &str, sw_vers: &str) -> String {
    let mut s = String::new();
    s.push_str(start.trim());
    s.push('\n');
    for l in sw_vers.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        s.push_str(l);
        s.push('\n');
    }
    if s.len() > MAX {
        let mut cut = MAX;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Write the text to the device without blocking.
pub fn write_to(device: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(device)?;
    let mut rest = text.as_bytes();
    let mut tries = 0;
    while !rest.is_empty() {
        match f.write(rest) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => rest = &rest[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && tries < TRIES => {
                tries += 1;
                std::thread::sleep(GAP);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Find the console in `dev` and write the text. The device on Ok, the
/// line for the agent log on an error.
pub fn announce_in(dev: &Path, text: &str) -> Result<PathBuf, String> {
    let names: Vec<String> = std::fs::read_dir(dev)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let Some(name) = pick_device(&names) else {
        return Err(text::console_no_device(&seen(&names)));
    };
    let device = dev.join(name);
    write_to(&device, text)
        .map(|()| device.clone())
        .map_err(|e| text::console_write_failed(&device.display().to_string(), &e.to_string()))
}

/// The start of the agent in the guest: the start line and `sw_vers` on
/// the console, and one line of the agent log either way.
pub fn announce(start: &str) {
    let sw_vers = std::process::Command::new("/usr/bin/sw_vers")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    match announce_in(Path::new("/dev"), &lines(start, &sw_vers)) {
        Ok(device) => log::info!("console: wrote the start lines to {}", device.display()),
        Err(line) => log::warn!("{line}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_virtio_cu_device_is_picked_first() {
        let n = names(&[
            "tty.virtio",
            "cu.Bluetooth-Incoming-Port",
            "cu.virtio1",
            "cu.virtio",
            "disk0",
        ]);
        assert_eq!(pick_device(&n).as_deref(), Some("cu.virtio"));
        let n = names(&["tty.Virtio-Console", "cu.Bluetooth"]);
        assert_eq!(pick_device(&n).as_deref(), Some("tty.Virtio-Console"));
        assert_eq!(pick_device(&names(&["cu.Bluetooth", "null"])), None);
        assert_eq!(seen(&names(&["null", "tty.b", "cu.a"])), "cu.a, tty.b");
        assert_eq!(seen(&names(&["null"])), "none");
    }

    #[test]
    fn the_lines_have_the_start_and_the_version() {
        let s = lines(
            "paddock-guest 0.9 starts\n",
            "ProductName:\t\tmacOS\nProductVersion:\t\t27.2\n\nBuildVersion:\t\t27C5\n",
        );
        assert_eq!(
            s,
            "paddock-guest 0.9 starts\nProductName:\t\tmacOS\nProductVersion:\t\t27.2\nBuildVersion:\t\t27C5\n"
        );
        // No sw_vers: the start line alone.
        assert_eq!(lines("x", ""), "x\n");
        let long = "é".repeat(MAX);
        assert!(lines("x", &long).len() <= MAX);
    }

    #[test]
    fn the_text_reaches_the_device_and_a_missing_one_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let e = announce_in(dir.path(), "a\n").unwrap_err();
        assert!(e.contains("no virtio serial console"), "{e}");
        // A plain file stands in for the device: appended, never truncated.
        let dev = dir.path().join("cu.virtio");
        std::fs::write(&dev, "before\n").unwrap();
        assert_eq!(announce_in(dir.path(), "a\nb\n").unwrap(), dev);
        assert_eq!(std::fs::read_to_string(&dev).unwrap(), "before\na\nb\n");
        // A device that cannot be opened is a line, not a panic.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("cu.virtio")).unwrap();
        let e = announce_in(dir.path(), "a\n").unwrap_err();
        assert!(e.contains("cannot write to the serial console"), "{e}");
    }
}
