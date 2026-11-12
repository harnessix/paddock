//! Waiting for a selected local decision and its receipt.

use super::*;

impl Firewall {
    /// Ask without a downstream acknowledgement. Its evidence stays retained.
    pub fn ask(
        &self,
        asker: &Asker,
        t: &Triple,
        who: Who,
        id: Option<&Identity>,
        now: u64,
    ) -> Result<Settled, String> {
        self.ask_with_receipt(asker, t, who, id, now)
            .map(|(settled, _)| settled)
    }

    /// Ask about `t` and wait for the answer: `Err` is the rule word of a
    /// refusal without an answer (`timeout`, `stale`).
    pub(super) fn ask_with_receipt(
        &self,
        asker: &Asker,
        t: &Triple,
        who: Who,
        id: Option<&Identity>,
        now: u64,
    ) -> Result<(Settled, Arc<Question>), String> {
        // The guest does not provide a process-generation identity, so a
        // later connection cannot safely inherit this connection's answer.
        let key = question_key(t, &who, id);
        let t = &key;
        let (q, hook) = match asker.join(t, who, id, now) {
            Join::Stale => {
                self.write_status();
                return Err(RULE_STALE.into());
            }
            Join::Wait(q, hook) => (q, hook),
        };
        if hook {
            if let Some(a) = asker.run_hook(t, &q) {
                // Claim without rule locks: the store may block. Application
                // then reloads and orders the result with cooperating writers.
                let claim = asker.claim_decision(t, &q);
                let settled = if claim.claimed {
                    let word = match asker.record_hook_selection(t, &q, a) {
                        Ok(()) => {
                            #[cfg(test)]
                            HOOK_SELECTION_RECORDED.with(|hook| {
                                if let Some(hook) = hook.borrow().as_ref() {
                                    hook();
                                }
                            });
                            self.apply_selection(a, &q.who, t, BY_APPROVER, now, firewall_bus())
                        }
                        Err(e) => {
                            log::warn!("macvm: cannot retain approver selection: {e}");
                            RULE_STALE.into()
                        }
                    };
                    let s = Settled {
                        answer: a,
                        allowed: a.allows() && word != RULE_STALE,
                        who: "approver",
                        word,
                    };
                    asker.finish(t, &q, s.clone(), now);
                    Some(s)
                } else {
                    None
                };
                if claim.status_changed || settled.is_some() {
                    self.write_status();
                }
                if let Some(s) = settled {
                    self.recheck_current_rules(now);
                    return Ok((s, q));
                }
            }
            asker.enqueue(t, now);
        }
        self.write_status();
        self.recheck_current_rules(now);
        self.wait_on(asker, t, &q)
    }

    fn wait_on(
        &self,
        asker: &Asker,
        t: &Triple,
        q: &Arc<Question>,
    ) -> Result<(Settled, Arc<Question>), String> {
        let start = Instant::now();
        let mut nudged = Instant::now();
        asker.nudge();
        loop {
            self.settle(crate::state::now());
            if let Some(s) = q.result() {
                return Ok((s, q.clone()));
            }
            if start.elapsed() >= asker.wait() {
                if let Some(s) = asker.timeout(t, q, crate::state::now()) {
                    return Ok((s, q.clone()));
                }
                log::info!(
                    "macvm: connection to {}:{} gave up after the maximum wait of {} seconds",
                    t.host,
                    t.port,
                    asker.wait().as_secs()
                );
                self.write_status();
                return Err(RULE_TIMEOUT.into());
            }
            if nudged.elapsed() >= ON_WAIT_EVERY {
                nudged = Instant::now();
                asker.nudge();
            }
            std::thread::sleep(asker.poll_every());
        }
    }
}
