//! Retire presentations that were open before the recorded wake.

use super::*;

impl Asker {
    // The caller holds Inner, ordering reconciliation with joins, claims and
    // notice processing. Store claims serialize with answer publication.
    pub(super) fn reconcile_wake(&self, g: &mut Inner) -> bool {
        let wake = self.last_wake();
        if wake == 0 {
            return true;
        }
        // Second-resolution timestamps cannot order events in the wake second.
        if g.notice.is_some() && g.notice_created <= wake {
            let id = g.notice.as_ref().expect("checked above").clone();
            if g.notice_published {
                match broker::claim_connect(&self.env, &id, true) {
                    Ok(Some(_)) => {
                        // The selection has no trustworthy ordering relative
                        // to physical wake. Keep it and any outcome on disk.
                        log::warn!(
                            "macvm: pause selection {id} has unknown wake ordering; retained"
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!("macvm: cannot reconcile pause notice {id}: {e}");
                        return false;
                    }
                }
            }
            g.notice = None;
            g.notice_published = false;
            g.notice_count = None;
            g.notice_snapshot = None;
            g.notice_targets.clear();
            g.notice_selected = None;
            g.queue.resume();
            g.status_changed = true;
        }
        let stale: Vec<_> = g
            .map
            .iter()
            .filter(|(_, q)| q.asked_at() <= wake && !q.taken.load(Ordering::SeqCst))
            .map(|(t, q)| (t.clone(), q.clone()))
            .collect();
        for (t, q) in stale {
            let published = q.published.load(Ordering::SeqCst);
            let selection = if published {
                match broker::claim_connect_question(
                    &self.env,
                    &self.card(&q, &t, 0).cleaned(),
                    true,
                ) {
                    Ok(text) => text,
                    Err(e) => {
                        log::warn!(
                            "macvm: cannot reconcile stale question {}: {e}",
                            q.card_id()
                        );
                        return false;
                    }
                }
            } else {
                None
            };
            let (answer, who) = match selection.as_deref() {
                Some(text) => {
                    let Some((answer, who)) = Self::parse_answer(text) else {
                        return false;
                    };
                    // This answer was not observed by the helper before the
                    // wake. Its file has no reliable wake-order evidence.
                    let snapshot = serde_json::from_str::<serde_json::Value>(text)
                        .ok()
                        .and_then(|v| v.get("request").cloned())
                        .and_then(|v| serde_json::from_value::<Pending>(v).ok());
                    if let Some(snapshot) = snapshot {
                        if let Err(e) = broker::record_wake_selection(
                            &self.env,
                            &q.card_id(),
                            answer,
                            who,
                            &snapshot,
                            text,
                        ) {
                            log::warn!(
                                "macvm: cannot record uncertain wake selection {}: {e}",
                                q.card_id()
                            );
                        }
                    }
                    (answer, who)
                }
                None => (ConnectAnswer::Deny, "wake"),
            };
            q.taken.store(true, Ordering::SeqCst);
            q.published.store(false, Ordering::SeqCst);
            *q.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(Settled {
                answer,
                allowed: false,
                who,
                word: "stale".into(),
            });
            g.queue.remove(&t);
            g.map.remove(&t);
            g.status_changed = true;
            if published {
                self.publish(
                    QuestionState::Withdrawn,
                    &q.card_id(),
                    Some((&t, q.path.clone())),
                    None,
                    None,
                );
            }
        }
        true
    }
}
