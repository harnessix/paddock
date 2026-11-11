//! The progress of a golden build for the launcher: the last `setup` line
//! in `setup-progress.json` of the `macvm` folder (for `macvm status
//! --json`), the same line on the event bus, and on stdout under
//! `PADDOCK_SETUP_JSON=1` (for `paddock macvm setup --json`). Plus the
//! pause of the download: `setup.pause` in the `macvm` folder, which the
//! download loop (`download.rs`) polls.
//!
//! The rules:
//! - A build runs while a process holds the flock of `setup.lock`.
//! - `--pause-download` writes the pause file only while a build runs and
//!   is in the download; the download stops curl and waits.
//! - `--resume` removes the file; with no build running it starts one,
//!   which continues the partial download. A new build removes a pause
//!   file that a killed build left.
//! - A write error is logged and never fails the build.

use super::download::Tick;
use super::layout::Layout;
use super::text;
use crate::env::HostEnv;
use crate::eventstream::schema::{Event, Setup};
use crate::eventstream::{Bus, Line};
use std::path::PathBuf;

/// The file of the last progress line, in the `macvm` folder.
pub const PROGRESS_FILE: &str = "setup-progress.json";
/// The pause request of the download, in the `macvm` folder.
pub const PAUSE_FILE: &str = "setup.pause";
/// The least time between two download progress lines.
pub const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
/// The variable that makes the helper print its progress lines as JSON.
pub const JSON_VAR: &str = "PADDOCK_SETUP_JSON";

pub fn progress_file(layout: &Layout) -> PathBuf {
    layout.root.join(PROGRESS_FILE)
}

pub fn pause_file(layout: &Layout) -> PathBuf {
    layout.root.join(PAUSE_FILE)
}

/// True while a build holds the setup lock.
pub fn running(layout: &Layout) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::File::open(&layout.setup_lock) else {
        return false;
    };
    // SAFETY: a plain flock on a file that this function opened; the
    // shared lock goes with the file at the end of the function.
    let free = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0;
    !free
}

/// True while the pause file is there.
pub fn paused(layout: &Layout) -> bool {
    pause_file(layout).exists()
}

/// The last progress line of a build, when one wrote it.
pub fn read(layout: &Layout) -> Option<Setup> {
    let bytes = std::fs::read(progress_file(layout)).ok()?;
    match serde_json::from_slice::<Line>(&bytes).ok()?.event {
        Event::Setup(s) => Some(s),
        _ => None,
    }
}

/// What `--pause-download` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pause {
    Requested,
    NoBuild,
    /// The build is past the download, in this state.
    PastDownload(String),
}

impl Pause {
    pub fn line(&self) -> String {
        match self {
            Pause::Requested => text::pause_requested(),
            Pause::NoBuild => text::pause_no_build(),
            Pause::PastDownload(step) => text::pause_past_download(step),
        }
    }
}

/// `paddock macvm setup --pause-download`.
pub fn request_pause(layout: &Layout) -> std::io::Result<Pause> {
    if !running(layout) {
        return Ok(Pause::NoBuild);
    }
    if let Some(s) = read(layout) {
        if s.step != super::golden::State::Fetch.name() {
            return Ok(Pause::PastDownload(s.step));
        }
    }
    crate::privfs::write_atomic(&pause_file(layout), b"")?;
    Ok(Pause::Requested)
}

/// `paddock macvm setup --resume`: removes the pause file. True when a
/// build runs and continues by itself; false when the caller starts one.
pub fn resume(layout: &Layout) -> bool {
    let _ = std::fs::remove_file(pause_file(layout));
    running(layout)
}

/// The writer of the progress lines of one build.
pub struct Reporter {
    file: PathBuf,
    bus: Option<Bus>,
    json: bool,
    /// The state of the last line, for the lines of the terminal.
    pub step: String,
    /// When the last download progress line went out.
    last_progress: Option<std::time::Instant>,
}

impl Reporter {
    /// The reporter of a build in `layout` for the user of `env`.
    pub fn new(env: &HostEnv, layout: &Layout) -> Self {
        Self {
            file: progress_file(layout),
            bus: Some(Bus::of(env)),
            json: env.var(JSON_VAR) == Some("1"),
            step: String::new(),
            last_progress: None,
        }
    }

    /// A reporter that only writes the file. Tests use it.
    pub fn file_only(layout: &Layout) -> Self {
        Self {
            file: progress_file(layout),
            bus: None,
            json: false,
            step: String::new(),
            last_progress: None,
        }
    }

    /// Write one line: the file, the bus, and stdout under JSON_VAR.
    pub fn send(&mut self, s: Setup) {
        self.step = s.step.clone();
        let line = Line::now(Event::Setup(s));
        let text = line.to_text();
        // A refusal before the build made its folder leaves no file.
        let folder = self.file.parent().is_some_and(|d| d.is_dir());
        if folder {
            if let Err(e) = crate::privfs::write_atomic(&self.file, text.as_bytes()) {
                log::warn!("macvm: cannot write {}: {e}", self.file.display());
            }
        }
        if let Some(bus) = &self.bus {
            bus.send(&text);
        }
        if self.json {
            print!("{text}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }

    /// A line of the state `step` with nothing else.
    pub fn step(&mut self, step: &str) {
        self.send(Setup {
            step: step.to_string(),
            ..Default::default()
        });
    }

    /// The build stopped in `step` (the state that a later setup runs).
    pub fn stopped(&mut self, step: &str) {
        self.send(Setup {
            step: step.to_string(),
            stopped: true,
            ..Default::default()
        });
    }

    /// The current state failed with paddock's line.
    pub fn failed(&mut self, reason: &str) {
        self.send(Setup {
            step: self.step.clone(),
            failed: Some(reason.to_string()),
            ..Default::default()
        });
    }

    /// One report of the download watch. Progress goes out at most every
    /// [`PROGRESS_EVERY`]; the pause, the resume and the end always.
    pub fn download(&mut self, t: Tick) {
        let fetch = super::golden::State::Fetch.name().to_string();
        let s = match t {
            Tick::Progress { bytes, total, rate } => {
                let end = rate.is_none();
                let now = std::time::Instant::now();
                if !end && self.last_progress.is_some_and(|l| now - l < PROGRESS_EVERY) {
                    return;
                }
                self.last_progress = Some(now);
                Setup {
                    step: fetch,
                    bytes: Some(bytes),
                    total,
                    rate,
                    ..Default::default()
                }
            }
            Tick::Paused { bytes, total } => Setup {
                step: fetch,
                bytes: Some(bytes),
                total,
                paused: true,
                ..Default::default()
            },
            Tick::Resumed { bytes, total } => Setup {
                step: fetch,
                bytes: Some(bytes),
                total,
                ..Default::default()
            },
        };
        self.send(s);
    }

    /// The golden is ready: the `golden` event on the bus.
    pub fn golden_ready(&self, version: &str, bytes: u64) {
        if let Some(bus) = &self.bus {
            bus.publish(Event::Golden(crate::eventstream::schema::Golden {
                state: crate::eventstream::schema::GoldenState::Ready,
                version: Some(version.to_string()),
                bytes,
            }));
        }
    }
}

#[cfg(test)]
#[path = "setup_progress_tests.rs"]
mod tests;
