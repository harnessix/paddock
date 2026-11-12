//! A call on the main thread from another thread, with a bound on the
//! wait. Some APIs of macOS must run on the main thread: since macOS 14
//! the Text Input Sources calls assert it. The threads of computer use
//! send such a call to the main queue and wait for its result.

use std::sync::mpsc;
use std::time::Duration;

/// A job for the main thread.
pub type Job = Box<dyn FnOnce() + Send>;

/// Run `f` on the main thread and give its result. When `on_main` is
/// true, this thread is the main thread and `f` runs here. Else `post`
/// sends the job to the main thread (the main dispatch queue), and the
/// call waits at most `wait` for it. None: the main thread did not run
/// the job in time. The job can still run later, and its result is lost.
pub fn run<T: Send + 'static>(
    on_main: bool,
    post: impl FnOnce(Job),
    f: impl FnOnce() -> T + Send + 'static,
    wait: Duration,
) -> Option<T> {
    if on_main {
        return Some(f());
    }
    let (tx, rx) = mpsc::sync_channel(1);
    post(Box::new(move || {
        let _ = tx.send(f());
    }));
    rx.recv_timeout(wait).ok()
}

#[cfg(test)]
#[path = "main_call_tests.rs"]
mod tests;
