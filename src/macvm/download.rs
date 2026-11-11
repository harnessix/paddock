//! The watch of the restore image download: the progress lines, the
//! progress events, and the pause. Plain code over a child process, so the
//! Linux tests run it with `sh` in place of curl; `vz_mac_image.rs` runs
//! it with curl.
//!
//! The rules:
//! - One terminal line per 5 percent with the time left, as before.
//! - One progress event per [`Watch::every`] with the bytes, the total and
//!   the rate.
//! - The pause file stops the child (curl continues a partial file with
//!   `-C -`), and the watch waits until the file is gone, then starts the
//!   child again. The paused event and the resumed event go out once each.

use super::text::{self, SetupPhase};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

/// The time between two looks of the watch at the child and the pause.
pub const EVERY: Duration = Duration::from_secs(1);

/// One report of the watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    Progress {
        bytes: u64,
        total: Option<u64>,
        rate: Option<u64>,
    },
    Paused {
        bytes: u64,
        total: Option<u64>,
    },
    Resumed {
        bytes: u64,
        total: Option<u64>,
    },
}

/// What the download watches and where it reports.
pub struct Watch<'a> {
    /// The pause file (`setup_progress::pause_file`).
    pub pause: PathBuf,
    /// The time between two looks at the file and the child.
    pub every: Duration,
    pub report: &'a mut dyn FnMut(Tick),
}

impl<'a> Watch<'a> {
    /// A watch that never pauses and reports nowhere.
    pub fn none(report: &'a mut dyn FnMut(Tick)) -> Self {
        Self {
            pause: PathBuf::new(),
            every: Duration::from_secs(1),
            report,
        }
    }

    fn paused(&self) -> bool {
        !self.pause.as_os_str().is_empty() && self.pause.exists()
    }
}

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Run `spawn` until the child ends well, with the pauses of `watch`.
/// `line` gets the terminal lines. An exit that is not a success is the
/// error, with the stderr of the child.
pub fn run(
    spawn: &mut dyn FnMut() -> Result<Child>,
    dest: &Path,
    size: Option<u64>,
    watch: &mut Watch,
    line: &mut dyn FnMut(String),
) -> Result<()> {
    let total = size.filter(|t| *t > 0);
    let mut last_pct = 0u64;
    loop {
        if watch.paused() {
            wait_paused(dest, total, watch);
        }
        if total.is_some_and(|t| len(dest) >= t) {
            // The pause came after the last byte: curl would refuse a
            // range past the end.
            line(text::progress(
                SetupPhase::Download,
                100,
                Some(Duration::ZERO),
            ));
            return Ok(());
        }
        let mut child = spawn()?;
        let begin = Instant::now();
        let start_bytes = len(dest);
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut e, &mut err);
                    }
                    bail!(text::download_failed(&status.to_string(), err.trim()));
                }
                line(text::progress(
                    SetupPhase::Download,
                    100,
                    Some(Duration::ZERO),
                ));
                (watch.report)(Tick::Progress {
                    bytes: len(dest),
                    total,
                    rate: None,
                });
                return Ok(());
            }
            std::thread::sleep(watch.every);
            let have = len(dest);
            if watch.paused() {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            let done = have.saturating_sub(start_bytes) as f64;
            let rate = done / begin.elapsed().as_secs_f64().max(0.001);
            (watch.report)(Tick::Progress {
                bytes: have,
                total,
                rate: Some(rate as u64),
            });
            if let Some(total) = total {
                let pct = (have * 100 / total) / 5 * 5;
                if pct > last_pct {
                    last_pct = pct;
                    let left = (rate > 0.0)
                        .then(|| Duration::from_secs_f64(total.saturating_sub(have) as f64 / rate));
                    line(text::progress(SetupPhase::Download, pct as u8, left));
                }
            }
        }
    }
}

/// Report the pause, wait until the pause file is gone, report the resume.
fn wait_paused(dest: &Path, total: Option<u64>, watch: &mut Watch) {
    (watch.report)(Tick::Paused {
        bytes: len(dest),
        total,
    });
    while watch.paused() {
        std::thread::sleep(watch.every);
    }
    (watch.report)(Tick::Resumed {
        bytes: len(dest),
        total,
    });
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
