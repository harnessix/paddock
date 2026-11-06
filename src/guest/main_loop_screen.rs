//! The screen lock and the login keychain in the agent's main loop: the
//! `screen.lock` event the moment the lock state changes, the
//! `keychain.unlock` request of the host, and the bench ops `bench.lock`
//! and `bench.keychain`. The host types the password into a locked
//! screen (`macvm/run_screen.rs`); the agent only reports the state.

use super::Session;
use crate::guest::platform::Platform;
use crate::macvm::proto::{Event, Reply, ReplyData};

impl<P: Platform> Session<'_, P> {
    /// Read the lock state at each poll, and send `screen.lock` when it
    /// differs from what the host has. A new connection hears of a locked
    /// screen at once, and of an unlocked one only when it changes.
    pub(super) fn screen_poll(&mut self) -> Result<(), String> {
        let locked = self.p.screen_locked();
        let before = self.st.screen_locked.replace(locked);
        if !lock_changed(before, locked) {
            return Ok(());
        }
        let uptime = self.p.health().uptime;
        let idle_secs = self.p.idle_secs();
        if locked {
            log::warn!("the guest screen locked (up {uptime} s, idle {idle_secs:?} s)");
        } else {
            log::info!("the guest screen is unlocked (up {uptime} s)");
        }
        self.writer.send(&Event::ScreenLock {
            locked,
            uptime,
            idle_secs,
        })
    }

    /// `keychain.unlock`: the platform unlocks the login keychain. The
    /// password goes to no log and no reply.
    pub(super) fn keychain_unlock(&mut self, req: u64, password: &str) -> Reply {
        match self.p.keychain_unlock(password) {
            Ok(text) => {
                log::info!("keychain: {text}");
                Reply::data(req, ReplyData::Text { text })
            }
            Err((code, text)) => {
                log::warn!("keychain: {text}");
                Reply::error(req, code, text)
            }
        }
    }

    /// `bench.lock`: lock the guest screen now. The next poll sends the
    /// `screen.lock` event.
    pub(super) fn bench_lock(&mut self, req: u64) -> Reply {
        match self.p.lock_screen() {
            Ok(()) => {
                self.next_poll = std::time::Instant::now();
                Reply::ok(req)
            }
            Err((code, text)) => Reply::error(req, code, text),
        }
    }

    /// `bench.keychain`: the state of the login keychain as text.
    pub(super) fn bench_keychain(&mut self, req: u64) -> Reply {
        match self.p.keychain_info() {
            Ok(text) => Reply::data(req, ReplyData::Text { text }),
            Err((code, text)) => Reply::error(req, code, text),
        }
    }
}

/// Whether a poll that reads `locked` must send `screen.lock`: on a
/// change, and on the first read of a connection when the screen is
/// locked.
pub(super) fn lock_changed(before: Option<bool>, locked: bool) -> bool {
    match before {
        Some(b) => b != locked,
        None => locked,
    }
}
