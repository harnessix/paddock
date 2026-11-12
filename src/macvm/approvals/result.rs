//! Record local outcomes and keep waiter results until consumed.

use super::*;

impl Asker {
    /// The question is decided: each existing waiter gets `s`. A late once
    /// decision is not cached for another unverified connection.
    pub fn finish(&self, t: &Triple, q: &Arc<Question>, s: Settled, now: u64) {
        let mut g = self.lock();
        if !g.map.get(t).is_some_and(|m| Arc::ptr_eq(m, q)) {
            return;
        }
        if !q.taken.load(Ordering::SeqCst) {
            return;
        }
        let no_waiter = q.count() == 0;
        let mut recorded = false;
        if q.accepted_file.load(Ordering::SeqCst) {
            let binding = q
                .selection
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match binding
                .as_ref()
                .ok_or_else(|| std::io::Error::other("selection binding missing"))
                .and_then(|binding| {
                    broker::record_connect_outcome_bound(
                        &self.env,
                        &q.card_id(),
                        s.answer,
                        s.allowed,
                        &s.word,
                        s.who,
                        no_waiter,
                        binding,
                    )
                }) {
                Ok(()) => recorded = true,
                Err(e) => log::warn!(
                    "macvm: cannot record connect outcome {}; selection retained: {e}",
                    q.card_id()
                ),
            }
        }
        g.map.remove(t);
        g.queue.remove(t);
        g.status_changed = true;
        let answer = Some((s.answer.word(), s.who));
        self.publish(
            QuestionState::Answered,
            &q.card_id(),
            Some((t, q.path.clone())),
            answer,
            None,
        );
        let eligible = s.word != "stale";
        q.retirement_ready
            .store(recorded && eligible, Ordering::SeqCst);
        *q.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        // Merely placing a result in memory does not prove that a waiter
        // consumed it. Only a result with no waiter can be retired here.
        if recorded && eligible && no_waiter {
            if let Err(e) = broker::retire_connect_outcome(&self.env, &q.card_id()) {
                log::warn!(
                    "macvm: outcome {} retained without retirement: {e}",
                    q.card_id()
                );
            }
        }
        self.pump(&mut g, now, true);
    }

    /// `WAIT` passed for one connection. `Some`: the answer came just now.
    /// Else the connection stops waiting and the question stays open.
    pub fn timeout(&self, t: &Triple, q: &Question, now: u64) -> Option<Settled> {
        let mut g = self.lock();
        if let Some(s) = q.result() {
            return Some(s);
        }
        q.count.fetch_sub(1, Ordering::SeqCst);
        g.status_changed = true;
        if g.queue.shown() == Some(t) {
            self.pump(&mut g, now, true);
        }
        None
    }
}
