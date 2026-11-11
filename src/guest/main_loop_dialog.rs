//! The disk dialogs in the agent's main loop: every window poll looks for
//! a disk dialog (`dialogs.rs`), presses Ignore, and sends the `dialog`
//! event, so the host names it in the session log.

use super::Session;
use crate::guest::dialogs;
use crate::guest::platform::Platform;
use crate::guest::windows::RawWindow;
use crate::macvm::proto::Event;
use std::time::Instant;

impl<P: Platform> Session<'_, P> {
    /// Try the due dialog windows of `raw`, and tell the host.
    pub(super) fn dialogs(&mut self, raw: &[RawWindow]) -> Result<(), String> {
        let now = Instant::now();
        for w in self.st.dialogs.due(raw, now) {
            let seen = self.p.dialog_dismiss(&w);
            if !dialogs::relevant(&w.owner, &seen.buttons) {
                // Another alert of the notification center: the user's,
                // and it shows on the host already.
                self.st.dialogs.tried(w.id, true, now);
                continue;
            }
            let dismissed = seen.pressed.is_some();
            let text = dialogs::describe(&seen);
            if dismissed {
                log::info!("a disk dialog of {}: {text}", w.owner);
            } else {
                log::warn!("a disk dialog of {}: {text}", w.owner);
            }
            if self.st.dialogs.tried(w.id, dismissed, now) {
                self.writer.send(&Event::Dialog {
                    owner: w.owner.clone(),
                    text,
                    dismissed,
                })?;
            }
        }
        Ok(())
    }
}
