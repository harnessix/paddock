//! `paddock events --follow`: listen on the bus, print each line, and add
//! the lines that only the follower knows: `stream started` first,
//! `stream lost` after a missed line, and `disk` every
//! [`Opts::disk_every`] while a session runs or a golden builds.

use super::bus::{Bus, Follower};
use super::schema::{Disk, Event, Line, Stream, StreamState};
use crate::env::HostEnv;
use anyhow::Result;
use std::io::Write;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// The timing of a follower.
#[derive(Debug, Clone, Copy)]
pub struct Opts {
    /// `--json`: the lines as they are. Else one short text per line.
    pub json: bool,
    /// The free space line while something runs or builds.
    pub disk_every: Duration,
    /// How often the follower looks at the lost marker and the stop.
    pub tick: Duration,
    /// `--notify`: this follower shows the `notice` lines, so paddock on
    /// macOS does not post them itself while it runs.
    pub notify: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            json: true,
            disk_every: Duration::from_secs(30),
            tick: Duration::from_secs(1),
            notify: false,
        }
    }
}

/// Follow until `stop` says so or the reader of `out` goes. `busy` tells
/// whether a session runs or a golden builds: then it returns the disk
/// line.
pub fn follow(
    bus: &Bus,
    out: &mut dyn Write,
    stop: &dyn Fn() -> bool,
    disk: &dyn Fn() -> Option<Disk>,
    opts: Opts,
) -> Result<()> {
    let follower = Follower::bind_with(bus, std::process::id(), opts.notify)?;
    let (tx, rx) = crate::eventstream::bus::queue();
    follower.serve(tx)?;
    let started = Line::now(Event::Stream(Stream {
        state: StreamState::Started,
    }));
    if !emit(out, &started.to_text(), opts.json) {
        return Ok(());
    }
    let mut next_disk = Instant::now();
    while !stop() {
        let text = match rx.recv_timeout(opts.tick) {
            Ok(line) => Some(line + "\n"),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("the event socket {} closed", follower.path().display())
            }
        };
        if let Some(text) = text {
            if !emit(out, &text, opts.json) {
                return Ok(());
            }
        }
        if follower.take_lost() {
            let lost = Line::now(Event::Stream(Stream {
                state: StreamState::Lost,
            }));
            if !emit(out, &lost.to_text(), opts.json) {
                return Ok(());
            }
        }
        if Instant::now() >= next_disk {
            next_disk = Instant::now() + opts.disk_every;
            if let Some(d) = disk() {
                if !emit(out, &Line::now(Event::Disk(d)).to_text(), opts.json) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// Check a pipe or socket without writing to it. Regular files remain writable.
pub fn reader_gone(fd: std::os::fd::RawFd) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    // SAFETY: p is a valid pollfd and the timeout is zero.
    let n = unsafe { libc::poll(&mut p, 1, 0) };
    n > 0 && p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// Print one line; false when the reader is gone.
fn emit(out: &mut dyn Write, text: &str, json: bool) -> bool {
    let r = if json {
        out.write_all(text.as_bytes())
    } else {
        writeln!(out, "{}", human(text.trim_end()))
    };
    r.and_then(|_| out.flush()).is_ok()
}

/// The text form of a line: the time, the kind, then `key=value` of the
/// other fields in the order of the line.
pub fn human(text: &str) -> String {
    let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_string();
    };
    let mut out = format!(
        "{} {}",
        o.get("t").and_then(|t| t.as_str()).unwrap_or("-"),
        o.get("kind").and_then(|k| k.as_str()).unwrap_or("-")
    );
    for (k, v) in &o {
        if matches!(k.as_str(), "v" | "t" | "kind") {
            continue;
        }
        let v = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out.push_str(&format!(" {k}={v}"));
    }
    out
}

/// The disk line of `env`: the free space of the `macvm` folder while a
/// session runs or a golden builds.
pub fn disk_now(env: &HostEnv) -> Option<Disk> {
    let layout = crate::macvm::layout::Layout::new(env);
    let busy = !crate::macvm::manage::running_sessions(&layout).is_empty()
        || crate::macvm::setup_progress::running(&layout);
    if !busy {
        return None;
    }
    let free = crate::macvm::golden::free_bytes(
        &existing_ancestor(&layout.root),
        env.var("PADDOCK_MACVM_FAKE_FREE"),
    )
    .ok()?;
    Some(Disk {
        dir: layout.root,
        free,
    })
}

/// `path`, or its nearest folder that exists.
pub fn existing_ancestor(path: &std::path::Path) -> std::path::PathBuf {
    path.ancestors()
        .find(|p| p.exists())
        .unwrap_or(std::path::Path::new("/"))
        .to_path_buf()
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
