//! Access to question state and presentation IDs.

use super::*;

impl Question {
    pub(super) fn card_id(&self) -> String {
        let generation = self.generation.load(Ordering::SeqCst);
        if generation == 0 {
            self.id.clone()
        } else {
            format!("{}-g{generation}", self.id)
        }
    }

    pub(super) fn next_card(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.published.store(false, Ordering::SeqCst);
    }

    pub fn asked_at(&self) -> u64 {
        self.asked_at.load(Ordering::SeqCst)
    }

    /// The connections that wait on it now.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }

    pub fn result(&self) -> Option<Settled> {
        self.result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Asker {
    /// The connection handler has consumed this waiter's local result.
    /// This does not assert socket admission, peer delivery or durability.
    pub(in crate::macvm) fn acknowledge_waiter(&self, q: &Question) {
        let _g = self.lock();
        let waiting = q.count();
        if waiting == 0 {
            return;
        }
        q.count.fetch_sub(1, Ordering::SeqCst);
        if waiting == 1 && q.retirement_ready.swap(false, Ordering::SeqCst) {
            if let Err(e) = broker::retire_connect_outcome(&self.env, &q.card_id()) {
                log::warn!(
                    "macvm: outcome {} retained without retirement: {e}",
                    q.card_id()
                );
            }
        }
    }
}
