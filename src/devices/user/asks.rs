//! Whether the next request of a user device shows a card: the `ask` of
//! its `device_call` line on the event stream.

use super::super::config::Mode;
use super::Gate;

impl Gate {
    /// The next `check` shows a card: no session answer yet, and a paid
    /// device without a budget or a mode other than `allow`. It only looks.
    pub fn asks(&self) -> bool {
        let known = *self.session.lock().unwrap();
        if known.or_else(|| self.stored_session()).is_some() {
            return false;
        }
        if let Some((_, _, None)) = &self.paid {
            let budget = self
                .ledger()
                .ok()
                .and_then(|l| l.budget(self.root(), &self.name).ok());
            if !matches!(budget, Some(Some(_))) {
                return true;
            }
        }
        self.mode != Mode::Allow
    }
}
