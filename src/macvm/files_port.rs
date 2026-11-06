//! The limits of the `guest-files` port (1032), item 14 of the security
//! checklist: 16 MiB per transfer, 64 transfers per minute, and a transfer
//! that sends no byte for 10 s closes. The [`Limiter`] is pure with a Linux
//! test; the listener side that reads a transfer into the profile is macOS
//! only.
//!
//! A transfer is a [`FileHeader`] line, then exactly `size` bytes. The
//! token comes first in the header: a wrong token closes the connection
//! before any byte is read. Each kind lands in its own folder of the
//! profile, and the file name of the header is taken as a plain name
//! only, never a path.

use super::proto::FileKind;
use super::session::Profile;
use super::text::TransferWhy;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The most bytes of one transfer.
pub const TRANSFER_MAX: u64 = 16 << 20;
/// The most transfers that may start in one minute.
pub const PER_MINUTE: usize = 64;
/// A transfer that sends no byte for this long closes.
pub const IDLE: Duration = Duration::from_secs(10);
/// The longest header line.
pub const HEADER_MAX: usize = 4096;

/// The admission of transfers: the size and the rate.
#[derive(Debug, Default)]
pub struct Limiter {
    starts: VecDeque<Instant>,
}

impl Limiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a transfer that starts at `now` with `size` bytes, or the
    /// reason of its refusal. An admitted transfer counts for a minute.
    pub fn admit(&mut self, now: Instant, size: u64) -> Result<(), TransferWhy> {
        if size > TRANSFER_MAX {
            return Err(TransferWhy::TooLarge { size });
        }
        let minute = Duration::from_secs(60);
        while let Some(first) = self.starts.front() {
            if now.saturating_duration_since(*first) >= minute {
                self.starts.pop_front();
            } else {
                break;
            }
        }
        if self.starts.len() >= PER_MINUTE {
            return Err(TransferWhy::TooMany);
        }
        self.starts.push_back(now);
        Ok(())
    }

    /// The transfers of the last minute.
    pub fn recent(&self) -> usize {
        self.starts.len()
    }
}

/// True when a transfer whose last byte came at `last` is idle at `now`.
pub fn idle_closes(last: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last) >= IDLE
}

/// A plain file name: not empty, no slash, no NUL, not `.` or `..`, at
/// most 255 bytes.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\0'])
}

/// Where a transfer of `kind` named `name` lands in the profile. The
/// agent log and the crash reports go under `logs/guest/`, the clipboard
/// under `drop/.clipboard/` (swept at quit), a file of the guest under
/// `drop/.guest/`.
pub fn destination(profile: &Profile, kind: FileKind, name: &str) -> PathBuf {
    let dir = match kind {
        FileKind::Log | FileKind::Crash => profile.logs().join("guest"),
        FileKind::Clipboard => profile.drop_dir().join(super::drop::CLIPBOARD_DIR),
        FileKind::Drop => profile.drop_dir().join(".guest"),
    };
    dir.join(name)
}

#[cfg(target_os = "macos")]
pub use mac::handler;

/// The listener side: one thread per connection, the header, the
/// limiter, the bytes with the idle deadline, then the file for the
/// session.
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::macvm::proto::{self, FileHeader};
    use crate::macvm::text;
    use crate::macvm::vz_mac_agent::{AgentEvent, Handler, Sink};
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    /// The handler of port 1032 for [`crate::macvm::vz_mac_agent::Listener`].
    pub fn handler(
        token: String,
        profile: Profile,
        limiter: Arc<Mutex<Limiter>>,
        sink: Sink,
    ) -> Handler {
        Arc::new(move |fd: OwnedFd| {
            let (token, profile, limiter, sink) = (
                token.clone(),
                profile.clone(),
                limiter.clone(),
                sink.clone(),
            );
            std::thread::spawn(move || {
                let stream = UnixStream::from(fd);
                if let Err(line) = receive(stream, &token, &profile, &limiter, &sink) {
                    log::warn!("macvm: {line}");
                    sink(AgentEvent::FileRefused(line));
                }
            });
        })
    }

    fn receive(
        mut stream: UnixStream,
        token: &str,
        profile: &Profile,
        limiter: &Mutex<Limiter>,
        sink: &Sink,
    ) -> Result<(), String> {
        stream
            .set_read_timeout(Some(IDLE))
            .map_err(|e| text::io_failed("cannot set the read timeout", &e.to_string()))?;
        let line = match proto::read_raw_line(&mut stream, HEADER_MAX) {
            Ok(Some(l)) => l,
            Ok(None) => return Ok(()),
            Err(e) => {
                return Err(text::io_failed(
                    "cannot read the transfer header",
                    &format!("{e:#}"),
                ))
            }
        };
        let header: FileHeader = serde_json::from_str(&line)
            .map_err(|_| text::transfer_refused("?", TransferWhy::BadName))?;
        if header.token != token {
            return Err(text::transfer_refused(&header.name, TransferWhy::BadToken));
        }
        if !plain_name(&header.name) {
            return Err(text::transfer_refused(&header.name, TransferWhy::BadName));
        }
        limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(Instant::now(), header.size)
            .map_err(|why| text::transfer_refused(&header.name, why))?;
        let dest = destination(profile, header.kind, &header.name);
        let dir = dest.parent().expect("a folder");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| text::io_failed("cannot make the transfer folder", &e.to_string()))?;
        let partial = dest.with_extension("partial");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&partial)
            .map_err(|e| text::io_failed("cannot make the transfer file", &e.to_string()))?;
        let mut left = header.size;
        let mut buf = [0u8; 64 * 1024];
        while left > 0 {
            let want = buf.len().min(left as usize);
            match stream.read(&mut buf[..want]) {
                Ok(0) => {
                    let _ = std::fs::remove_file(&partial);
                    return Err(text::io_failed(
                        "the guest closed the transfer early",
                        &header.kind(),
                    ));
                }
                Ok(n) => {
                    file.write_all(&buf[..n]).map_err(|e| {
                        text::io_failed("cannot write the transfer", &e.to_string())
                    })?;
                    left -= n as u64;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    let _ = std::fs::remove_file(&partial);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Err(text::transfer_refused(
                        &header.name,
                        TransferWhy::Idle(IDLE),
                    ));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&partial);
                    return Err(text::io_failed("cannot read the transfer", &e.to_string()));
                }
            }
        }
        file.sync_all().ok();
        drop(file);
        std::fs::rename(&partial, &dest)
            .map_err(|e| text::io_failed("cannot commit the transfer", &e.to_string()))?;
        log::info!("macvm: received {}", header.kind());
        sink(AgentEvent::File {
            kind: header.kind,
            name: header.name,
            path: dest,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Item 14: the 65th transfer in a minute is refused, a 16 MiB plus
    /// one byte transfer is refused, an idle of 10 s closes.
    #[test]
    fn limits() {
        let mut l = Limiter::new();
        let t0 = Instant::now();
        for i in 0..PER_MINUTE {
            assert_eq!(
                l.admit(t0 + Duration::from_millis(i as u64), 1),
                Ok(()),
                "{i}"
            );
        }
        assert_eq!(l.recent(), 64);
        assert_eq!(
            l.admit(t0 + Duration::from_secs(30), 1),
            Err(TransferWhy::TooMany)
        );
        // The window slides: after a minute the first ones expire.
        assert_eq!(l.admit(t0 + Duration::from_secs(60), 1), Ok(()));
        assert_eq!(
            l.admit(t0 + Duration::from_secs(61), TRANSFER_MAX),
            Ok(()),
            "exactly 16 MiB passes"
        );
        assert_eq!(
            l.admit(t0 + Duration::from_secs(62), TRANSFER_MAX + 1),
            Err(TransferWhy::TooLarge {
                size: TRANSFER_MAX + 1
            })
        );
        // A refused transfer does not count.
        let before = l.recent();
        let _ = l.admit(t0 + Duration::from_secs(62), TRANSFER_MAX + 1);
        assert_eq!(l.recent(), before);
        assert!(!idle_closes(t0, t0 + Duration::from_millis(9_999)));
        assert!(idle_closes(t0, t0 + IDLE));
        assert!(idle_closes(t0, t0 + Duration::from_secs(11)));
        assert!(plain_name("agent.log") && plain_name("clip-7.json"));
        for bad in ["", ".", "..", "a/b", "../x", "a\0b", &"x".repeat(256)] {
            assert!(!plain_name(bad), "{bad:?}");
        }
        let p = Profile::new("/p");
        assert_eq!(
            destination(&p, FileKind::Log, "agent.log"),
            PathBuf::from("/p/logs/guest/agent.log")
        );
        assert_eq!(
            destination(&p, FileKind::Crash, "a.ips"),
            PathBuf::from("/p/logs/guest/a.ips")
        );
        assert_eq!(
            destination(&p, FileKind::Clipboard, "clip-7.json"),
            PathBuf::from("/p/drop/.clipboard/clip-7.json")
        );
        assert_eq!(
            destination(&p, FileKind::Drop, "x"),
            PathBuf::from("/p/drop/.guest/x")
        );
    }
}
