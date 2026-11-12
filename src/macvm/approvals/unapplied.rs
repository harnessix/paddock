//! Selections that close their question without their effect.
//!
//! No platform contract orders a physical sleep against the presentation,
//! the selection and the end of its application. A periodic wake timestamp
//! is not such a contract. So a human allow can come from a card that was
//! on the screen before a sleep that the wake detector has not seen yet.
//! Such an allow must not install a rule. It must not wedge the queue
//! either: the question closes as a once denial, the next question comes
//! up, and the session log and `paddock app status` say why. A denial and
//! a pause selection grant no access, so they apply in any wake order.

use super::*;

/// The status keeps this many unapplied selections, newest last.
pub(super) const UNAPPLIED_KEEP: usize = 20;

/// Why a selection closed its question as a once denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unapplied {
    /// No fence orders the selection after the last physical wake.
    WakeOrder,
    /// The answer of the card on the screen could not be read for
    /// [`super::streak::STREAK_SECONDS`] seconds. The store records only a
    /// valid selection, so no outcome is written: the answer file stays on
    /// disk for review, and the panel no longer lists the card.
    Invalid,
}

impl Unapplied {
    /// The rule word of the connection log and of the recorded outcome.
    pub fn word(self) -> &'static str {
        match self {
            Self::WakeOrder => broker::UNVERIFIED_RULE,
            Self::Invalid => "invalid",
        }
    }
}

/// The note of `paddock approvals answer` after it recorded `answer` for
/// `card`. `Some`: the card says that its session does not apply an allow,
/// so the selection closes the question as a once denial.
pub fn cli_answer_note(card: &Pending, answer: ConnectAnswer) -> Option<String> {
    let c = card.connect.as_ref()?;
    if card.capability != broker::CONNECT_CAPABILITY || c.notice || !answer.allows() {
        return None;
    }
    let unfenced = text::ask_allow_unfenced(&c.profile, &c.host, c.port);
    card.detail
        .lines()
        .any(|line| line == unfenced)
        .then(|| text::connect_allow_unapplied(&c.profile, &c.host, c.port))
}

impl Asker {
    pub(super) fn wake_order_fenced(&self) -> bool {
        #[cfg(test)]
        {
            self.wake_fence.load(Ordering::SeqCst)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    /// Model an external platform wake fence. Production has none.
    #[cfg(test)]
    pub(crate) fn set_wake_fence_for_test(&self, fenced: bool) {
        self.wake_fence.store(fenced, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn wake_fenced_for_test(self) -> Self {
        self.set_wake_fence_for_test(true);
        self
    }

    /// Close `q` as a once denial without the effect of its selection. The
    /// caller holds Inner and has claimed the answer of `q`.
    pub(super) fn refuse_selection(
        &self,
        g: &mut Inner,
        t: &Triple,
        q: &Arc<Question>,
        selected: Option<(ConnectAnswer, &'static str)>,
        binding: Option<(Pending, String)>,
        why: Unapplied,
    ) {
        let (answer, who) = selected.unwrap_or((ConnectAnswer::Deny, "user"));
        log::warn!(
            "macvm: {}",
            text::answer_unapplied(&self.profile, &t.host, t.port, answer.word(), why.word())
        );
        if let Some(binding) = binding {
            *q.selection.lock().unwrap_or_else(|e| e.into_inner()) = Some(binding);
            q.accepted_file.store(true, Ordering::SeqCst);
        }
        q.taken.store(true, Ordering::SeqCst);
        q.published.store(false, Ordering::SeqCst);
        g.queue.remove(t);
        self.note_unapplied(
            g,
            UnappliedAnswer {
                target: format!("{}:{}", t.host, t.port),
                selected: answer.word().into(),
                reason: why.word().into(),
                result: text::UNAPPLIED_DENIED.into(),
            },
        );
        g.ready.push(Answered {
            triple: t.clone(),
            question: q.clone(),
            answer,
            who,
            unapplied: Some(why),
        });
    }

    pub(super) fn note_unapplied(&self, g: &mut Inner, entry: UnappliedAnswer) {
        g.unapplied.push_back(entry);
        while g.unapplied.len() > UNAPPLIED_KEEP {
            g.unapplied.pop_front();
        }
        g.status_changed = true;
    }

    /// The latest selections that did not apply, for `paddock app status`.
    pub fn unapplied(&self) -> Vec<UnappliedAnswer> {
        self.lock().unapplied.iter().cloned().collect()
    }
}
