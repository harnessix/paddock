//! The custody of a headless harness: a run that got its task up front
//! and has no user at a terminal (`paddock-host spawn`).
//!
//! Such a run ends when its work is done: the [`Driver`] with
//! [`Ends::WhenDone`] delivers what waits in the inbox, and closes the
//! input of the harness when the main session is at a safe point and its
//! inbox is empty. An entry that comes later waits in the inbox for the
//! next run of the session.
//!
//! The output of the custody (what the custodian tells the user) goes into
//! a [`Captured`] buffer: the run of a spawned child returns it as its
//! output.
//!
//! [`Driver`]: super::harness::Driver
//! [`Ends::WhenDone`]: super::harness::Ends::WhenDone

use super::harness::{ChannelIo, Control};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// The stdio of a headless harness. No user types into the run: the
/// control channel has no sender.
pub fn io<R, W>(from: R, to: W) -> ChannelIo<R, W> {
    let (_, control) = std::sync::mpsc::channel::<Control>();
    ChannelIo { from, to, control }
}

/// The output of a custody, kept in memory. Only the last 4 MiB stay, as
/// for the captured output of a sandbox.
#[derive(Clone, Default)]
pub struct Captured(Arc<Mutex<Vec<u8>>>);

const KEEP: usize = 4 << 20;

impl Captured {
    /// What the custody wrote so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        b.extend_from_slice(buf);
        if b.len() > 2 * KEEP {
            let cut = b.len() - KEEP;
            b.drain(..cut);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_keeps_the_end() {
        let mut c = Captured::default();
        let big = vec![b'a'; 2 * KEEP];
        c.write_all(&big).unwrap();
        c.write_all(b"end\n").unwrap();
        let t = c.text();
        assert!(t.len() <= 2 * KEEP);
        assert!(t.ends_with("aend\n"));
    }
}
