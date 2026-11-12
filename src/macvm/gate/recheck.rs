//! Recheck and settle pending decisions.

use super::*;

impl Firewall {
    pub(super) fn recheck_current_rules(&self, now: u64) {
        let Some(asker) = &self.asker else {
            return;
        };
        let mut any = false;
        for (t, q) in asker.open() {
            #[cfg(test)]
            RECHECK_BEFORE_MODE_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow().as_ref() {
                    hook();
                }
            });
            // Do not withdraw an unrelated question on every recheck. This
            // is only a filter; the decision below is made again under lock.
            let mut rules = self.rules();
            #[cfg(test)]
            RECHECK_BEFORE_FILE_LOCK_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow().as_ref() {
                    hook();
                }
            });
            let file_lock = match rules.decision_lock() {
                Ok(lock) => lock,
                Err(e) => {
                    log::warn!("macvm: cannot lock firewall rules: {e}");
                    self.rules_dirty
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
            };
            match rules.reload_if_changed() {
                Ok(true) => self
                    .rules_dirty
                    .store(true, std::sync::atomic::Ordering::SeqCst),
                Ok(false) => {}
                Err(e) => {
                    log::warn!("macvm: {e:#}");
                    self.rules_dirty
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
            }
            let candidate = {
                let mode = self.mode.read().unwrap_or_else(|e| e.into_inner());
                firewall::decide(*mode, self.api.as_ref(), &rules.all(), &q.who.conn(&t))
            };
            if matches!(candidate, Decision::Ask) {
                continue;
            }
            #[cfg(test)]
            RECHECK_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow().as_ref() {
                    hook();
                }
            });
            // The store may stall while closing the card. Release both rule
            // locks first and reload under the writer lock after it closes.
            drop((rules, file_lock));
            let (claim, decision) = asker.claim_decision_if(&t, &q, || {
                #[cfg(test)]
                RECHECK_REVALIDATE_HOOK.with(|hook| {
                    if let Some(hook) = hook.borrow().as_ref() {
                        hook();
                    }
                });
                let mut rules = self.rules();
                let _file_lock = rules.decision_lock().ok()?;
                if !self.refresh_answer_rules(&mut rules) {
                    return None;
                }
                let mode = self.mode.try_read().ok()?;
                let (answer, word) =
                    match firewall::decide(*mode, self.api.as_ref(), &rules.all(), &q.who.conn(&t))
                    {
                        Decision::Allow(r) => (
                            ConnectAnswer::AllowOnce,
                            r.unwrap_or_else(|| RULE_LISTED.into()),
                        ),
                        Decision::Deny(r) => {
                            (ConnectAnswer::Deny, r.unwrap_or_else(|| RULE_MODE.into()))
                        }
                        Decision::Ask => return None,
                    };
                Some((answer, word))
            });
            any |= claim.status_changed;
            let Some((answer, word)) = decision else {
                self.rules_dirty
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                continue;
            };
            debug_assert!(claim.claimed);
            any = true;
            let s = Settled {
                answer,
                allowed: answer.allows(),
                who: BY_RULE,
                word,
            };
            asker.finish(&t, &q, s, now);
        }
        if any {
            self.write_status();
        }
    }

    /// Apply the answers that came in: their rules into the session or
    /// the file, and the result to every waiter. Any thread may call it;
    /// each answer is applied once.
    pub fn settle(&self, now: u64) {
        let Some(asker) = &self.asker else {
            return;
        };
        let _ = self.current_rules(now);
        // Polling may wait on the approval store. No rule lock is held until
        // an answer has actually been claimed. Its later application, under
        // the file lock and after a fresh read, is the ordering point.
        let polled = asker.poll(now);
        #[cfg(test)]
        AFTER_POLL_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        let any = polled.status_changed
            || !polled.answers.is_empty()
            || self.status_retry.load(std::sync::atomic::Ordering::SeqCst);
        let answered = !polled.answers.is_empty();
        for a in polled.answers {
            let by = if a.who == "approver" {
                BY_APPROVER
            } else if a.who == "cli" {
                "cli"
            } else {
                BY_USER
            };
            // An unapplied selection changes no rule: its question closes
            // as a once denial, and the asker has said why.
            let word = match a.unapplied {
                Some(why) => why.word().to_string(),
                None => self.apply_selection(
                    a.answer,
                    &a.question.who,
                    &a.triple,
                    by,
                    now,
                    firewall_bus(),
                ),
            };
            let s = Settled {
                answer: a.answer,
                allowed: a.unapplied.is_none() && a.answer.allows() && word != RULE_STALE,
                who: a.who,
                word,
            };
            asker.finish(&a.triple, &a.question, s, now);
        }
        if answered
            || self
                .rules_dirty
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.recheck_current_rules(now);
        }
        if any {
            self.write_status();
        }
    }
}
