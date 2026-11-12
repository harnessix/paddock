//! Each pause ID names one immutable set of queued questions.
use super::*;

impl Asker {
    // The caller holds Inner: claim before any firewall side effects. Every
    // claimed selection closes its question, so the queue always advances.
    // `false`: the text cannot be read. The claim checks the answer and its
    // card, so this is damaged evidence; the question stays open, and the
    // poll of the screen closes it as a once denial when it stays so.
    pub(super) fn accepted(
        &self,
        g: &mut Inner,
        t: &Triple,
        q: &Arc<Question>,
        text: &str,
    ) -> bool {
        let parsed = Self::parse_answer(text);
        let request = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v.get("request").cloned())
            .and_then(|v| serde_json::from_value::<Pending>(v).ok());
        let (Some((answer, who)), Some(request)) = (parsed, request) else {
            log::warn!("macvm: cannot read connect answer {}", q.card_id());
            return false;
        };
        let binding = (request, text.to_owned());
        if answer.allows() && !self.wake_order_fenced() {
            self.refuse_selection(
                g,
                t,
                q,
                Some((answer, who)),
                Some(binding),
                Unapplied::WakeOrder,
            );
            return true;
        }
        *q.selection.lock().unwrap_or_else(|e| e.into_inner()) = Some(binding);
        q.taken.store(true, Ordering::SeqCst);
        q.accepted_file.store(true, Ordering::SeqCst);
        q.published.store(false, Ordering::SeqCst);
        g.queue.remove(t);
        g.status_changed = true;
        g.ready.push(Answered {
            triple: t.clone(),
            question: q.clone(),
            answer,
            who,
            unapplied: None,
        });
        true
    }

    pub(super) fn rotate_notice(&self, g: &mut Inner) -> bool {
        if !g.notice_published || g.notice_targets == g.queue.all() {
            return true;
        }
        let Some(old) = g.notice_snapshot.as_ref() else {
            return false;
        };
        match broker::claim_connect_exact(&self.env, old, true) {
            Ok(Some(_)) => return false, // Keep the selected version and its targets.
            Ok(None) => {}
            Err(e) => {
                log::warn!("macvm: cannot replace pause presentation {}: {e}", old.id);
                return false;
            }
        }
        let Some(next) = g.notice_generation.checked_add(1) else {
            return false;
        };
        let Some(root) = g.notice_root.as_ref() else {
            return false;
        };
        g.notice_generation = next;
        g.notice = Some(format!("{root}-g{next}"));
        g.notice_published = false;
        g.notice_count = None;
        g.notice_snapshot = None;
        g.notice_targets.clear();
        g.status_changed = true;
        true
    }
}
