//! Transfers to the host on the `guest-files` port (1032): a header
//! line, then the bytes. A transfer that sends no byte for 10 s closes.

use super::platform::Fail;
use crate::macvm::proto::{write_line, ErrorCode, FileHeader, FileKind};
use crate::macvm::text;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A transfer that sends no byte for this long closes.
pub const STALL: Duration = Duration::from_secs(10);

/// Send one file to the host. `dial` opens the `guest-files` connection.
pub fn send(
    token: &str,
    kind: FileKind,
    path: &Path,
    dial: impl FnOnce() -> std::io::Result<UnixStream>,
) -> Result<(), Fail> {
    let io = |what: &str, e: std::io::Error| (ErrorCode::Io, text::io_failed(what, &e.to_string()));
    let mut file = std::fs::File::open(path).map_err(|e| io("cannot open the file", e))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let header = FileHeader {
        token: token.to_string(),
        kind,
        name,
        size,
    };
    log::info!("sending {}", header.kind());
    let stream = dial().map_err(|e| io("cannot dial the guest-files port", e))?;
    let progress = Arc::new(AtomicU64::new(0));
    let watchdog = {
        let stream = stream
            .try_clone()
            .map_err(|e| io("cannot clone the socket", e))?;
        let progress = progress.clone();
        std::thread::spawn(move || {
            let mut last = u64::MAX;
            loop {
                std::thread::sleep(STALL);
                let now = progress.load(Ordering::Relaxed);
                if now == u64::MAX {
                    return;
                }
                if now == last {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return;
                }
                last = now;
            }
        })
    };
    let result = (|| {
        let mut out = &stream;
        write_line(&mut out, &header).map_err(|e| (ErrorCode::Io, format!("{e:#}")))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| io("cannot read the file", e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| io("the host stopped reading", e))?;
            progress.fetch_add(n as u64, Ordering::Relaxed);
        }
        out.flush().map_err(|e| io("cannot flush", e))?;
        Ok(())
    })();
    progress.store(u64::MAX, Ordering::Relaxed);
    let _ = stream.shutdown(std::net::Shutdown::Write);
    drop(watchdog);
    result
}
