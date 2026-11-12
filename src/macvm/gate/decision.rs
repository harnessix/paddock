//! Applying selected answers and recording their effects.

use super::*;

impl Firewall {
    // The file lock is held by the caller during this refresh and decision.
    pub(super) fn refresh_answer_rules(&self, rules: &mut Rules) -> bool {
        match rules.reload_if_changed() {
            Ok(changed) => {
                if changed {
                    self.rules_dirty
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                true
            }
            Err(e) => {
                log::warn!("macvm: cannot refresh firewall rules for answer: {e:#}");
                self.rules_dirty
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                false
            }
        }
    }

    /// Test entry point for a decision already selected outside the queue.
    #[cfg(test)]
    pub(super) fn apply(
        &self,
        answer: ConnectAnswer,
        who: &Who,
        t: &Triple,
        by: &str,
        now: u64,
    ) -> String {
        self.apply_with_bus(answer, who, t, by, now, firewall_bus())
    }

    #[cfg(test)]
    pub(super) fn apply_with_bus(
        &self,
        answer: ConnectAnswer,
        who: &Who,
        t: &Triple,
        by: &str,
        now: u64,
        bus: Option<&crate::eventstream::Bus>,
    ) -> String {
        self.apply_selection(answer, who, t, by, now, bus)
    }

    pub(super) fn apply_selection(
        &self,
        answer: ConnectAnswer,
        who: &Who,
        t: &Triple,
        by: &str,
        now: u64,
        bus: Option<&crate::eventstream::Bus>,
    ) -> String {
        let mut changes = Vec::new();
        let word = {
            let mut rules = self.rules();
            let lock = rules.decision_lock();
            let readable = match &lock {
                Ok(_) => self.refresh_answer_rules(&mut rules),
                Err(e) => {
                    log::warn!("macvm: cannot lock firewall rules for answer: {e}");
                    false
                }
            };
            match (&lock, readable) {
                (Ok(lock), true) => {
                    self.apply_locked(answer, who, t, by, now, &mut rules, lock, &mut changes)
                }
                _ if answer == ConnectAnswer::DenyAlways => {
                    self.fallback_deny(who, t, by, now, &mut rules, &mut changes)
                }
                _ => RULE_STALE.into(),
            }
        };
        // Grants and followers can block indefinitely. They use snapshots,
        // after both rule locks have been released.
        let source = if by == BY_APPROVER {
            GrantSource::Approver
        } else if by == "cli" {
            GrantSource::You
        } else {
            GrantSource::QuestionPanel
        };
        let session_changed = changes.iter().any(|(r, _)| r.scope == Scope::Session);
        for (rule, action) in &changes {
            #[cfg(test)]
            RULE_EFFECTS_HOOK.with(|hook| {
                if let Some(hook) = hook.borrow().as_ref() {
                    hook();
                }
            });
            self.note_rule(rule, action, by, now);
            if let Some(asker) = &self.asker {
                let event = crate::macvm::events_stream::historical_rule_grant(
                    asker.profile(),
                    rule,
                    action,
                    source,
                );
                #[cfg(test)]
                GRANT_EVENT_HOOK.with(|hook| {
                    if let Some(hook) = hook.borrow().as_ref() {
                        hook(event.clone());
                    }
                });
                if let Some(bus) = bus {
                    bus.publish(event);
                }
            }
        }
        if session_changed {
            self.write_session_rules();
        }
        word
    }

    pub(super) fn fallback_deny(
        &self,
        who: &Who,
        t: &Triple,
        by: &str,
        now: u64,
        rules: &mut Rules,
        changes: &mut Vec<(Rule, &'static str)>,
    ) -> String {
        let Some(mut rule) =
            firewall::rule_of_answer(ConnectAnswer::DenyAlways, &who.conn(t), by, now)
        else {
            return RULE_STALE.into();
        };
        rule.scope = Scope::Session;
        rule.id = rule.id.replacen("rule-", "session-", 1);
        let before = rules.session.clone();
        match rules.add(rule, false) {
            Ok(applied) => {
                let applied = applied.clone();
                // A fallback must not erase an independent session
                // exception at the same scope. Keep both; normal rule
                // precedence decides between them.
                for old in &before {
                    if !rules.session.iter().any(|current| current.id == old.id) {
                        rules.session.push(old.clone());
                    }
                }
                for old in before
                    .iter()
                    .filter(|old| !rules.session.iter().any(|r| r.id == old.id))
                {
                    changes.push((old.clone(), "remove"));
                }
                changes.push((applied.clone(), "add"));
                log::warn!("macvm: deny-always could not be persisted; installed session deny {} for {}:{}", applied.id, applied.host, applied.port.text());
                applied.id
            }
            Err(e) => {
                log::warn!("macvm: cannot install fallback session deny: {e:#}");
                RULE_STALE.into()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_locked(
        &self,
        answer: ConnectAnswer,
        who: &Who,
        t: &Triple,
        by: &str,
        now: u64,
        rules: &mut Rules,
        lock: &std::fs::File,
        changes: &mut Vec<(Rule, &'static str)>,
    ) -> String {
        // Inspect persistent and session rules separately. A pre-existing
        // session exception may outrank a file deny for future connections,
        // but it must not cause a stale answer to create another grant.
        // winner observes expiry and each set's specificity.
        if answer.allows()
            && [&rules.file, &rules.session].into_iter().any(|list| {
                firewall::winner(list, &who.conn(t))
                    .is_some_and(|r| r.verdict == firewall::Verdict::Deny)
            })
        {
            return RULE_STALE.into();
        }
        match firewall::rule_of_answer(answer, &who.conn(t), by, now) {
            Some(r) => {
                let wildcard = answer == ConnectAnswer::AllowSite
                    && firewall::site::registrable_site(&t.host).is_some();
                if answer.allows() {
                    let mut scopes = vec![r.clone()];
                    if wildcard {
                        let mut subdomains = r.clone();
                        subdomains.host = format!("*.{}", r.host);
                        scopes.push(subdomains);
                    }
                    // Selections and rules have no shared generation or
                    // trustworthy ordering. Reject overlapping active file
                    // or session denies, including a session rule that add
                    // would otherwise replace at the same scope.
                    if rules.file.iter().chain(&rules.session).any(|deny| {
                        deny.verdict == firewall::Verdict::Deny
                            && deny.expires.is_none_or(|e| e > crate::state::now())
                            && scopes
                                .iter()
                                .any(|scope| firewall::rule_scopes_overlap(scope, deny))
                    }) {
                        return RULE_STALE.into();
                    }
                }
                let add = |rules: &mut Rules, rule: &Rule| -> anyhow::Result<Rule> {
                    let added = rules
                        .add(rule.clone(), answer == ConnectAnswer::AllowEverything)?
                        .clone();
                    if wildcard {
                        let mut w = added.clone();
                        w.id.push_str("-subdomains");
                        w.host = format!("*.{}", added.host);
                        rules.add(w, false)?;
                    }
                    Ok(added)
                };
                let before = rules.file.clone();
                let session_before = rules.session.clone();
                let applied = match add(rules, &r) {
                    Ok(added) => added,
                    Err(e) => {
                        rules.file = before;
                        log::warn!("macvm: {e:#}");
                        if answer == ConnectAnswer::DenyAlways {
                            log::warn!("macvm: deny-always could not be installed; only this connection is denied");
                        }
                        return RULE_STALE.into();
                    }
                };
                if applied.scope == Scope::Session {
                    for old in &session_before {
                        if !rules.session.iter().any(|current| current.id == old.id) {
                            rules.session.push(old.clone());
                        }
                    }
                }
                if applied.scope == Scope::Always {
                    if let Err(e) = rules.save_locked(lock) {
                        rules.file = before;
                        if answer.allows() {
                            // No existing session grant is deleted. In the
                            // event of an uncooperative writer, refresh while
                            // still locked; unreadable state fails closed.
                            let refreshed = self.refresh_answer_rules(rules);
                            log::warn!("macvm: cannot safely apply firewall answer: {e:#}; refreshed: {refreshed}");
                            return RULE_STALE.into();
                        }
                        log::warn!("macvm: cannot save firewall deny: {e:#}");
                        return self.fallback_deny(who, t, by, now, rules, changes);
                    }
                }
                for removed in before
                    .iter()
                    .filter(|old| !rules.file.iter().any(|current| current.id == old.id))
                {
                    changes.push((removed.clone(), "remove"));
                }
                for removed in session_before
                    .iter()
                    .filter(|old| !rules.session.iter().any(|current| current.id == old.id))
                {
                    changes.push((removed.clone(), "remove"));
                }
                changes.push((applied.clone(), "add"));
                if wildcard {
                    let wildcard_id = format!("{}-subdomains", applied.id);
                    if let Some(subdomains) = rules
                        .file
                        .iter()
                        .chain(&rules.session)
                        .find(|rule| rule.id == wildcard_id)
                    {
                        changes.push((subdomains.clone(), "add"));
                    }
                }
                applied.id
            }
            None => RULE_ONCE.into(),
        }
    }

    pub(super) fn write_session_rules(&self) {
        let _writer = self
            .session_write_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let session = self.rules().session.clone();
        let path = self.log.with_file_name("session-rules.json");
        let metadata = self.log.with_file_name("session-rules.meta.json");
        // The vector remains readable by older CLIs. Even a successfully
        // written snapshot can lag a mutation; the sidecar is advisory and
        // is never evidence that the vector is current or complete.
        if let Err(e) = crate::privfs::write_atomic(&metadata, br#"{"version":1,"complete":false}"#)
        {
            log::warn!("macvm: cannot write session rules metadata: {e}");
        }
        if let Ok(data) = serde_json::to_vec(&session) {
            if let Err(e) = crate::privfs::write_atomic(&path, data) {
                log::warn!("macvm: cannot write session rules: {e}");
            }
        }
    }
}
