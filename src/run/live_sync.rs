//! Threads that keep the login files and the session history of a running
//! sandbox in line with the host while it runs.

use crate::state::{AuthSync, SyncStep};
use std::time::{Duration, Instant};

/// How often a running sandbox and the host bring its login files in line.
pub(super) const LOGIN_EVERY: Duration = Duration::from_secs(2);

/// Keeps the login files of a running sandbox and the host in line, both
/// ways (`AuthSync::sync_now`), so a refreshed token reaches the host and
/// every other sandbox on the same login within seconds, not at the end.
pub(super) struct LoginSync {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<AuthSync>>,
}

impl LoginSync {
    pub(super) fn start(mut auth: AuthSync, quiet: bool) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            while !s.load(std::sync::atomic::Ordering::SeqCst) {
                for step in auth.sync_now() {
                    match step {
                        SyncStep::ToStore(f) if !quiet => {
                            log::info!("saved a refreshed login to {}", f.display())
                        }
                        SyncStep::ToSandbox(f) if !quiet => {
                            log::info!("the sandbox got the newer login of {}", f.display())
                        }
                        _ => {}
                    }
                }
                let start = std::time::Instant::now();
                while start.elapsed() < LOGIN_EVERY && !s.load(std::sync::atomic::Ordering::SeqCst)
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            // One last look before the copy back at the end.
            auth.sync_now();
            auth
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    pub(super) fn finish(mut self) -> AuthSync {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread
            .take()
            .expect("started")
            .join()
            .expect("login sync thread")
    }
}

/// History copied to the host while the sandbox runs, so the Omarchy agents
/// panel counts new sessions before the sandbox exits. Logins are not synced
/// here: a login goes back to its store only at exit.
pub(super) struct HistorySync {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<(Option<crate::history::CopyBack>, usize)>,
}

/// How often history is copied.
pub(super) const HISTORY_EVERY: Duration = Duration::from_secs(300);

impl HistorySync {
    pub(super) fn start(history: Option<crate::history::CopyBack>, every: Duration) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut copied = 0;
            let mut at = Instant::now();
            while history.is_some() && !flag.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
                if at.elapsed() < every {
                    continue;
                }
                at = Instant::now();
                if let Some(h) = &history {
                    match h.copy_out() {
                        Ok(files) => copied += files.len(),
                        Err(e) => log::debug!("cannot copy the session history: {e:#}"),
                    }
                }
            }
            (history, copied)
        });
        Self { stop, thread }
    }

    /// Stop the thread and return the history and the files it copied.
    pub(super) fn finish(self) -> (Option<crate::history::CopyBack>, usize) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread
            .join()
            .expect("the history thread does not panic")
    }
}
