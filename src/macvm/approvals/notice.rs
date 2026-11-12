//! Processing the pause notice and its recorded result.

use super::*;

#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_OUTCOME_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
}

pub(super) struct SelectedNotice {
    text: String,
    answer: ConnectAnswer,
    who: &'static str,
    targets: Vec<Triple>,
    affected: Vec<broker::AffectedDecision>,
}

impl Asker {
    /// `false`: retry later and keep other claims back. A notice that keeps
    /// failing is dropped: asking goes on and its evidence stays on disk.
    /// `now`: the clock of the poll. A poll without it only retries, so the
    /// claims of many connections in one moment count as one failure.
    pub(super) fn poll_notice(&self, g: &mut Inner, now: Option<u64>) -> bool {
        match self.poll_notice_once(g) {
            Ok(ready) => {
                g.notice_failures.clear();
                ready
            }
            Err(reason) => {
                let (Some(now), Some(id)) = (now, g.notice.clone()) else {
                    return false;
                };
                if !g.notice_failures.fail(&id, now) {
                    return false;
                }
                self.abandon_notice(g, reason);
                true
            }
        }
    }

    // A pause selection grants no access: `Deny all` denies once and `Ask one
    // by one` only resumes the queue. So it applies in any wake order.
    fn poll_notice_once(&self, g: &mut Inner) -> Result<bool, &'static str> {
        let Some(id) = g.notice.clone() else {
            return Ok(true);
        };
        if !g.notice_published {
            return Ok(true);
        }
        let Some(snapshot) = g.notice_snapshot.as_ref() else {
            return Err("unreadable");
        };
        let text = match broker::claim_connect_exact(&self.env, snapshot, false) {
            Ok(Some(text)) => text,
            Ok(None) if g.notice_selected.is_none() => return Ok(true),
            Ok(None) => {
                log::warn!("macvm: pause selection {id} disappeared; retained for review");
                return Err("unreadable");
            }
            Err(e) => {
                log::warn!("macvm: cannot claim pause notice {id}: {e}");
                return Err("unreadable");
            }
        };
        if g.notice_selected.is_none() {
            let Some((answer, who)) = Self::parse_answer(&text) else {
                log::warn!("macvm: invalid pause answer {id}");
                return Err("invalid");
            };
            // claim_connect_exact already matched the selected snapshot.
            // Older answers without one remain unresolved on disk.
            let open = g.queue.all();
            let targets: Vec<_> = if answer == ConnectAnswer::Deny {
                g.notice_targets
                    .iter()
                    .filter(|t| open.contains(t))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            let affected = targets
                .iter()
                .filter_map(|t| {
                    let q = g.map.get(t)?;
                    Some(broker::AffectedDecision {
                        id: q.card_id(),
                        app: q.who.app.clone(),
                        parent_app: q.who.parent_app.clone(),
                        team: q.who.team.clone(),
                        host: t.host.clone(),
                        port: t.port,
                        path: q.path.clone(),
                        guest_pid: t.identity.as_ref().and_then(|i| i.pid),
                        waiting: q.count(),
                    })
                })
                .collect();
            g.notice_selected = Some(SelectedNotice {
                text: text.clone(),
                answer,
                who,
                targets,
                affected,
            });
        }
        let selected = g
            .notice_selected
            .as_ref()
            .expect("selection retained above");
        if selected.text != text {
            log::warn!("macvm: pause selection {id} changed; retained for review");
            return Err("invalid");
        }
        #[cfg(test)]
        BEFORE_OUTCOME_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        if let Err(e) = broker::record_pause_outcome(
            &self.env,
            &id,
            selected.answer,
            selected.who,
            selected.affected.clone(),
            g.notice_snapshot
                .as_ref()
                .expect("published notice has a snapshot"),
            &selected.text,
        ) {
            log::warn!("macvm: cannot reconcile pause outcome {id}; selection retained: {e}");
            return Err("unreadable");
        }
        #[cfg(test)]
        PAUSE_RECORDED_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        let selected = g.notice_selected.take().expect("selection retained above");
        g.notice = None;
        g.notice_published = false;
        g.notice_count = None;
        g.notice_snapshot = None;
        g.notice_targets.clear();
        g.status_changed = true;
        let deny_all = !selected.answer.allows();
        let n = if deny_all {
            let n = selected.targets.len();
            for t in selected.targets {
                g.queue.remove(&t);
                if let Some(q) = g.map.get(&t).cloned() {
                    q.taken.store(true, Ordering::SeqCst);
                    g.ready.push(Answered {
                        triple: t,
                        question: q,
                        answer: ConnectAnswer::Deny,
                        who: selected.who,
                        unapplied: None,
                    });
                }
            }
            // Connections added after the selected presentation remain open.
            g.queue.resume();
            n
        } else {
            g.queue.resume();
            let n = g.queue.open();
            match broker::confirm_pause_resume(&self.env, &id) {
                Ok(()) => {
                    if let Err(e) = broker::retire_connect_outcome(&self.env, &id) {
                        log::warn!("macvm: pause outcome {id} retained without retirement: {e}");
                    }
                }
                Err(e) => {
                    log::warn!("macvm: cannot confirm pause resume {id}; retained for review: {e}")
                }
            }
            n
        };
        self.publish(
            QuestionState::Resumed,
            &id,
            None,
            Some((selected.answer.word(), selected.who)),
            Some(n as u64),
        );
        log::info!("macvm: {}", text::asks_resumed(deny_all, n));
        Ok(true)
    }

    /// Drop a notice whose selection cannot be applied: asking goes on, no
    /// question is denied, and the answer and outcome files stay for review.
    fn abandon_notice(&self, g: &mut Inner, reason: &'static str) {
        let Some(id) = g.notice.take() else {
            return;
        };
        let selected = g
            .notice_selected
            .take()
            .map(|s| s.answer.word())
            .unwrap_or("unknown");
        // An unanswered card is withdrawn; an answered one stays as evidence.
        if let Err(e) = broker::claim_connect(&self.env, &id, true) {
            log::warn!("macvm: cannot withdraw pause notice {id}: {e}");
        }
        g.notice_published = false;
        g.notice_count = None;
        g.notice_snapshot = None;
        g.notice_targets.clear();
        g.notice_failures.clear();
        g.queue.resume();
        let n = g.queue.open();
        log::warn!("macvm: {}", text::notice_unapplied(&self.profile, n));
        self.note_unapplied(
            g,
            UnappliedAnswer {
                target: text::UNAPPLIED_NOTICE.into(),
                selected: selected.into(),
                reason: reason.into(),
                result: text::UNAPPLIED_RESUMED.into(),
            },
        );
        self.publish(QuestionState::Resumed, &id, None, None, Some(n as u64));
    }
}
