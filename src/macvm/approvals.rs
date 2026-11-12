//! The `ask` of the firewall: one question per process identity, host and port, and at
//! most one of them per session on the screen. The question on the screen
//! is a card in the approvals store of the host, which the marker shows as
//! a panel and `paddock approvals answer` answers from a terminal. The
//! other open questions wait in the queue of `ask_queue.rs`. The session
//! helper (`paddock-vm app`) holds each connection while it waits.
//!
//! The rules:
//! - Production keys identify one connection, because the guest header has
//!   no stable process-generation identity. An answer cannot be transferred
//!   to a later connection on the strength of a reused or missing PID.
//! - After the ten-minute maximum wait the connection is denied and the
//!   question stays open. A late session or persistent selection can still
//!   create its rule; a late once selection has no waiter and is not reused.
//! - The number of open questions is bounded; excess connections fail closed.
//! - A question asked before the last wake is retired and fails closed;
//!   a later connection can ask a fresh question.
//! - The card is the truth of the question on the screen: a restart of
//!   the marker loses nothing, because the question is a file.
//! - The approver hook of `approver.rs` answers first when its
//!   configuration names `connect`, before the question joins the queue.
//! - Too many open questions pause the asking of the session: one notice
//!   card replaces the panel. `deny` on it denies every open question
//!   once, `allow-once` goes on with one question at a time.
//!
//! Pure with Linux tests: the store is a folder, the wake a number. The
//! firewall (`gate.rs`) applies the answers and runs the waits.

use super::ask_queue::{Queue, Step};
use super::ask_status::{OpenQuestion, UnappliedAnswer};
use super::firewall::Conn;
use super::proxy_header::Identity;
use super::text;
use crate::broker::{self, ConnectAnswer, Pending};
use crate::env::HostEnv;
#[cfg(test)]
use crate::eventstream::schema::Question as EventQuestion;
use crate::eventstream::schema::QuestionState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[cfg(test)]
pub(super) type EventHook = Box<dyn Fn(EventQuestion)>;
#[cfg(test)]
type WriteHook = Box<dyn Fn(&Pending) -> std::io::Result<()>>;
#[cfg(test)]
pub(super) type ConnectHook = Box<dyn Fn() -> Option<ConnectAnswer>>;

#[cfg(test)]
thread_local! {
    pub(super) static EVENT_HOOK: std::cell::RefCell<Option<EventHook>> = std::cell::RefCell::new(None);
    static WRITE_HOOK: std::cell::RefCell<Option<WriteHook>> = std::cell::RefCell::new(None);
    pub(super) static CLAIM_CLOSE_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    pub(super) static POLL_STORE_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    pub(super) static TEST_CONNECT_HOOK: std::cell::RefCell<Option<ConnectHook>> = std::cell::RefCell::new(None);
    pub(super) static PAUSE_RECORDED_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
}

/// How long a connection waits for the answer.
pub const WAIT: Duration = Duration::from_secs(600);
/// How often a waiter looks for the answer file.
const POLL: Duration = Duration::from_millis(200);
/// How often a waiter calls `on_wait` (the marker restart).
pub const ON_WAIT_EVERY: Duration = Duration::from_secs(10);
/// The `who` of an answer that a rule gave to an open question.
pub const BY_RULE: &str = "rule";

/// What a question is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Triple {
    pub app: String,
    pub host: String,
    pub port: u16,
    pub identity: Option<QuestionIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QuestionIdentity {
    pub who: Who,
    pub pid: Option<u32>,
    pub path: Option<String>,
    pub connection: String,
}

/// The guest process of the first connection of a question, for the rule
/// of its answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Who {
    pub app: String,
    pub parent_app: Option<String>,
    pub team: Option<String>,
}

impl Who {
    pub fn conn<'a>(&'a self, t: &'a Triple) -> Conn<'a> {
        Conn {
            app: &self.app,
            parent_app: self.parent_app.as_deref(),
            team: self.team.as_deref(),
            host: &t.host,
            port: t.port,
        }
    }
}

/// The selected answer, its effective access decision, and the log rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub answer: ConnectAnswer,
    /// A selected allow can fail closed when the rule state is unsafe.
    pub allowed: bool,
    pub who: &'static str,
    pub word: String,
}

/// One open question, shared by every connection that waits on it.
pub struct Question {
    pub id: String,
    pub who: Who,
    path: Option<String>,
    asked_at: AtomicU64,
    count: AtomicU64,
    /// The answer file was read; the firewall applies it.
    taken: AtomicBool,
    /// Each presentation has a distinct public id; ordinary rewrites keep it.
    generation: AtomicU64,
    published: AtomicBool,
    /// This question has a recorded selection in the approval store.
    accepted_file: AtomicBool,
    retirement_ready: AtomicBool,
    selection: Mutex<Option<(Pending, String)>>,
    result: Mutex<Option<Settled>>,
}

/// What a connection gets from [`Asker::join`].
pub enum Join {
    /// Wait on this question. `true`: it is new and the approver hook
    /// runs first; [`Asker::enqueue`] queues it when the hook abstains.
    Wait(Arc<Question>, bool),
    /// The question is stale or the bounded queue is full: deny this attempt.
    Stale,
}

/// An answer that [`Asker::poll`] found, for the firewall to apply.
pub struct Answered {
    pub triple: Triple,
    pub question: Arc<Question>,
    pub answer: ConnectAnswer,
    pub who: &'static str,
    /// `Some`: close the question as a once denial without the effect of
    /// the selection. The firewall applies no rule for it.
    pub unapplied: Option<Unapplied>,
}

pub struct Polled {
    pub answers: Vec<Answered>,
    pub status_changed: bool,
}

pub struct Claim {
    pub claimed: bool,
    pub status_changed: bool,
}

type OnWait = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    map: HashMap<Triple, Arc<Question>>,
    queue: Queue,
    /// The id of the notice card while asking is paused.
    notice: Option<String>,
    notice_root: Option<String>,
    notice_generation: u64,
    notice_published: bool,
    notice_count: Option<usize>,
    notice_created: u64,
    notice_targets: Vec<Triple>,
    notice_snapshot: Option<Pending>,
    notice_selected: Option<notice::SelectedNotice>,
    /// A published card could not be updated; retry it on the next poll.
    rewrite_pending: bool,
    status_changed: bool,
    ready: Vec<Answered>,
    /// Failed polls of an answered notice, per second.
    notice_failures: streak::Streak,
    /// Failed claims of the question on the screen, per second.
    shown_failures: streak::Streak,
    /// The latest selections that did not apply, for the status.
    unapplied: std::collections::VecDeque<UnappliedAnswer>,
}

/// Bound memory and queued questions even when connections have no stable
/// process identity and their waiters time out before a selection arrives.
pub(super) const MAX_OPEN_QUESTIONS: usize = 128;

/// The asker of one session.
pub struct Asker {
    env: HostEnv,
    /// The app profile name.
    profile: String,
    /// The name of the app in the question: `Sample Notes`.
    label: String,
    profile_dir: PathBuf,
    icon: Option<PathBuf>,
    /// The session helper, whose death ends every question.
    pid: u32,
    wait: Duration,
    poll: Duration,
    inner: Mutex<Inner>,
    last_wake: AtomicU64,
    hook: Option<crate::approver::HookConfig>,
    on_wait: Option<OnWait>,
    /// Models a platform wake fence that production does not have.
    #[cfg(test)]
    wake_fence: AtomicBool,
}

impl Asker {
    pub fn new(env: HostEnv, profile: &str, label: &str, profile_dir: PathBuf, pid: u32) -> Self {
        Self {
            env,
            profile: profile.to_string(),
            label: label.to_string(),
            profile_dir,
            icon: None,
            pid,
            wait: WAIT,
            poll: POLL,
            inner: Mutex::new(Inner::default()),
            last_wake: AtomicU64::new(0),
            hook: None,
            on_wait: None,
            #[cfg(test)]
            wake_fence: AtomicBool::new(false),
        }
    }

    /// The wait and the poll, for tests.
    pub fn timing(mut self, wait: Duration, poll: Duration) -> Self {
        self.wait = wait;
        self.poll = poll;
        self
    }

    /// Another rate limit of the queue, for tests.
    pub fn rate(self, max: usize) -> Self {
        self.lock().queue = Queue::with_rate(max);
        self
    }

    /// The icon of the app on the host, for the panel.
    pub fn icon(mut self, icon: Option<PathBuf>) -> Self {
        self.icon = icon;
        self
    }

    /// The approver hook, which answers before the user when it may.
    pub fn hook(mut self, hook: Option<crate::approver::HookConfig>) -> Self {
        self.hook = hook.filter(|h| {
            h.capabilities
                .iter()
                .any(|c| c == broker::CONNECT_CAPABILITY)
        });
        self
    }

    /// Called at the start of each wait and every 10 s of it: the marker
    /// restart.
    pub fn on_wait(mut self, f: OnWait) -> Self {
        self.on_wait = Some(f);
        self
    }

    pub fn nudge(&self) {
        if let Some(f) = &self.on_wait {
            f();
        }
    }

    pub fn wait(&self) -> Duration {
        self.wait
    }

    pub fn poll_every(&self) -> Duration {
        self.poll
    }

    /// The host woke at `now`: every question asked before it is stale.
    pub fn note_wake(&self, now: u64) {
        let mut g = self.lock();
        self.last_wake.fetch_max(now, Ordering::SeqCst);
        self.reconcile_wake(&mut g);
    }

    pub fn last_wake(&self) -> u64 {
        self.last_wake.load(Ordering::SeqCst)
    }

    /// The open questions.
    pub fn waiting(&self) -> usize {
        self.lock().map.len()
    }

    pub fn paused(&self) -> bool {
        self.lock().queue.paused()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The app profile of the session.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Bring the store up to date with the queue. `rewrite`: the card on
    /// the screen changed (its count or the questions behind it).
    fn pump(&self, g: &mut Inner, now: u64, rewrite: bool) {
        if !self.reconcile_wake(g) {
            return;
        }
        let mut shown_now = false;
        for step in g.queue.pump(now) {
            g.status_changed = true;
            match step {
                Step::Show(t) => {
                    shown_now = true;
                    if let Some(q) = g.map.get(&t) {
                        let published = q.published.load(Ordering::SeqCst);
                        if self.write_card(&self.card(q, &t, g.queue.more() as u64), published) {
                            g.rewrite_pending = false;
                            q.published.store(true, Ordering::SeqCst);
                            if !published {
                                g.status_changed = true;
                                self.publish(
                                    QuestionState::Open,
                                    &q.card_id(),
                                    Some((&t, q.path.clone())),
                                    None,
                                    None,
                                );
                            }
                        } else {
                            g.rewrite_pending = true;
                        }
                    }
                }
                Step::Withdraw(t) => {
                    if let Some(q) = g.map.get(&t).cloned() {
                        let published = q.published.load(Ordering::SeqCst);
                        let result = if published {
                            broker::claim_connect_question(
                                &self.env,
                                &self.card(&q, &t, 0).cleaned(),
                                true,
                            )
                        } else {
                            Ok(None)
                        };
                        match result {
                            Ok(Some(text)) => {
                                self.accepted(g, &t, &q, &text);
                            }
                            Ok(None) => {
                                if published {
                                    self.publish(
                                        QuestionState::Withdrawn,
                                        &q.card_id(),
                                        Some((&t, q.path.clone())),
                                        None,
                                        None,
                                    );
                                }
                                q.next_card();
                            }
                            Err(e) => {
                                log::warn!(
                                    "macvm: cannot hide connect question {}: {e}",
                                    q.card_id()
                                );
                                g.queue.undo_pause(&t);
                                return;
                            }
                        }
                    }
                }
                Step::Pause => {
                    let open = g.queue.open();
                    log::warn!(
                        "macvm: {}",
                        text::asks_paused(&self.label, &self.profile, open)
                    );
                    let id = broker::new_request_id();
                    g.notice_root = Some(id.clone());
                    g.notice_generation = 0;
                    g.notice = Some(id);
                    g.notice_count = None;
                    g.notice_created = now;
                    g.notice_targets.clear();
                    g.notice_snapshot = None;
                    g.notice_selected = None;
                }
            }
        }
        if !shown_now {
            if let Some(t) = g.queue.shown().cloned() {
                if let Some(q) = g.map.get(&t) {
                    let published = q.published.load(Ordering::SeqCst);
                    if rewrite || g.rewrite_pending || !published {
                        if self.write_card(&self.card(q, &t, g.queue.more() as u64), published) {
                            g.rewrite_pending = false;
                            q.published.store(true, Ordering::SeqCst);
                            if !published {
                                g.status_changed = true;
                                self.publish(
                                    QuestionState::Open,
                                    &q.card_id(),
                                    Some((&t, q.path.clone())),
                                    None,
                                    None,
                                );
                            }
                        } else {
                            g.rewrite_pending = true;
                        }
                    }
                }
            }
            if g.notice.is_some() {
                if !self.rotate_notice(g) {
                    return;
                }
                let id = g.notice.clone().expect("notice remains present");
                let open = g.queue.open();
                if rewrite || g.rewrite_pending || !g.notice_published {
                    let card = self.notice_card(&id, open, g.notice_created);
                    if self.write_card(&card, g.notice_published) {
                        g.notice_targets = g.queue.all();
                        g.notice_snapshot = Some(card.cleaned());
                        g.rewrite_pending = false;
                        if g.notice_count != Some(open) {
                            self.publish(QuestionState::Paused, &id, None, None, Some(open as u64));
                        }
                        if !g.notice_published {
                            g.status_changed = true;
                        }
                        g.notice_published = true;
                        g.notice_count = Some(open);
                    } else {
                        g.rewrite_pending = true;
                    }
                }
            }
        }
    }

    /// A connection to `t` asks: join the open question of the triple, or
    /// open a new one.
    pub fn join(&self, t: &Triple, who: Who, id: Option<&Identity>, now: u64) -> Join {
        let mut g = self.lock();
        if !self.reconcile_wake(&mut g) {
            return Join::Stale;
        }
        if self.last_wake() != 0 && now <= self.last_wake() {
            return Join::Stale;
        }
        if !g.map.contains_key(t) && g.map.len() >= MAX_OPEN_QUESTIONS {
            log::warn!("macvm: {MAX_OPEN_QUESTIONS} firewall questions are open; refusing this connection as stale");
            return Join::Stale;
        }
        let (q, fresh) = match g.map.get(t) {
            Some(q) => (q.clone(), false),
            None => {
                let q = Arc::new(Question {
                    id: broker::new_request_id(),
                    who,
                    path: id.and_then(|i| i.path.clone()),
                    asked_at: AtomicU64::new(now),
                    count: AtomicU64::new(0),
                    taken: AtomicBool::new(false),
                    generation: AtomicU64::new(0),
                    published: AtomicBool::new(false),
                    accepted_file: AtomicBool::new(false),
                    retirement_ready: AtomicBool::new(false),
                    selection: Mutex::new(None),
                    result: Mutex::new(None),
                });
                g.map.insert(t.clone(), q.clone());
                (q, true)
            }
        };
        q.count.fetch_add(1, Ordering::SeqCst);
        g.status_changed = true;
        #[cfg(test)]
        let hook_enabled =
            self.hook.is_some() || TEST_CONNECT_HOOK.with(|hook| hook.borrow().is_some());
        #[cfg(not(test))]
        let hook_enabled = self.hook.is_some();
        if fresh && hook_enabled {
            return Join::Wait(q, true);
        }
        if fresh {
            g.queue.add(t.clone());
        }
        self.pump(&mut g, now, true);
        Join::Wait(q, false)
    }

    /// The approver hook on a new question. `None`: it abstains.
    pub fn run_hook(&self, t: &Triple, q: &Question) -> Option<ConnectAnswer> {
        #[cfg(test)]
        if let Some(answer) =
            TEST_CONNECT_HOOK.with(|hook| hook.borrow().as_ref().and_then(|hook| hook()))
        {
            return Some(answer);
        }
        let hook = self.hook.as_ref()?;
        let (answer, note) = crate::approver::run_hook_connect(hook, &self.card(q, t, 0));
        if let Some(a) = answer {
            log::info!(
                "macvm: the approver answered {} for {}:{} ({note})",
                a.word(),
                t.host,
                t.port
            );
        }
        answer
    }

    /// The hook abstained: the question joins the queue.
    pub fn enqueue(&self, t: &Triple, now: u64) {
        let mut g = self.lock();
        if !self.reconcile_wake(&mut g) {
            return;
        }
        if g.map
            .get(t)
            .is_some_and(|q| !q.taken.load(Ordering::SeqCst))
        {
            g.queue.add(t.clone());
        }
        self.pump(&mut g, now, true);
    }

    /// Claim a rule or hook decision before it performs any side effects.
    /// An answer already published to the card or notice wins the claim.
    pub fn claim_decision(&self, t: &Triple, q: &Arc<Question>) -> Claim {
        self.claim_decision_if(t, q, || Some(())).0
    }

    pub fn record_hook_selection(
        &self,
        t: &Triple,
        q: &Question,
        answer: ConnectAnswer,
    ) -> std::io::Result<()> {
        let card = self.card(q, t, 0).cleaned();
        broker::record_hook_connect_selection(&self.env, &card, answer)?;
        let text = broker::claim_connect_exact(&self.env, &card, false)?
            .ok_or_else(|| std::io::Error::other("hook selection missing"))?;
        *q.selection.lock().unwrap_or_else(|e| e.into_inner()) = Some((card, text));
        q.accepted_file.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Close the presentation before consulting the decision. The returned
    /// value can hold the decision's guards until the claim is complete.
    /// No approval-store operation runs while those guards are held.
    pub fn claim_decision_if<T>(
        &self,
        t: &Triple,
        q: &Arc<Question>,
        decide: impl FnOnce() -> Option<T>,
    ) -> (Claim, Option<T>) {
        let mut g = self.lock();
        let mut decision = None;
        let claimed = if !self.reconcile_wake(&mut g)
            || !self.poll_notice(&mut g, None)
            || !g.map.get(t).is_some_and(|m| Arc::ptr_eq(m, q))
            || q.taken.load(Ordering::SeqCst)
        {
            false
        } else {
            let result = if q.published.load(Ordering::SeqCst) {
                #[cfg(test)]
                CLAIM_CLOSE_HOOK.with(|hook| {
                    if let Some(hook) = hook.borrow().as_ref() {
                        hook();
                    }
                });
                broker::claim_connect_question(&self.env, &self.card(q, t, 0).cleaned(), true)
            } else {
                Ok(None)
            };
            match result {
                Ok(Some(text)) => {
                    // A selection that cannot be read stays open; the
                    // poll of the screen gives it up after a few seconds.
                    self.accepted(&mut g, t, q, &text);
                    false
                }
                Ok(None) => {
                    let was_published = q.published.swap(false, Ordering::SeqCst);
                    decision = decide();
                    if decision.is_some() {
                        q.taken.store(true, Ordering::SeqCst);
                        g.queue.remove(t);
                        g.status_changed = true;
                        true
                    } else {
                        // A withdrawn card must never be reused: a late
                        // writer may still have its old id.
                        if was_published {
                            self.publish(
                                QuestionState::Withdrawn,
                                &q.card_id(),
                                Some((t, q.path.clone())),
                                None,
                                None,
                            );
                            q.next_card();
                            g.rewrite_pending = true;
                            g.status_changed = true;
                        }
                        false
                    }
                }
                Err(e) => {
                    log::warn!("macvm: cannot claim connect question {}: {e}", q.card_id());
                    false
                }
            }
        };
        (
            Claim {
                claimed,
                status_changed: std::mem::take(&mut g.status_changed),
            },
            decision,
        )
    }

    /// Look for the answers of the question on the screen and of the
    /// notice, and put the next question up when the rate allows it.
    pub fn poll(&self, now: u64) -> Polled {
        let mut g = self.lock();
        if !self.reconcile_wake(&mut g) {
            return Polled {
                answers: std::mem::take(&mut g.ready),
                status_changed: std::mem::take(&mut g.status_changed),
            };
        }
        if let Some(t) = g.queue.shown().cloned() {
            if let Some(q) = g.map.get(&t).cloned() {
                if q.published.load(Ordering::SeqCst) {
                    #[cfg(test)]
                    POLL_STORE_HOOK.with(|hook| {
                        if let Some(hook) = hook.borrow().as_ref() {
                            hook();
                        }
                    });
                    let read = match broker::claim_connect_question(
                        &self.env,
                        &self.card(&q, &t, 0).cleaned(),
                        false,
                    ) {
                        Ok(Some(text)) if self.accepted(&mut g, &t, &q, &text) => Claimed::Read,
                        Ok(Some(_)) => Claimed::Damaged,
                        Ok(None) => Claimed::Read,
                        Err(e) => {
                            log::warn!("macvm: cannot claim connect answer {}: {e}", q.card_id());
                            if broker::is_store_lock_error(&e) {
                                Claimed::Unavailable
                            } else {
                                Claimed::Damaged
                            }
                        }
                    };
                    // A damaged, missing or unreadable card or answer must
                    // not keep every later question behind it. A short
                    // failure, such as a write that is still going on, is
                    // retried for a few seconds first. A store that cannot
                    // be locked says nothing about the answer: it is
                    // retried and never closes it.
                    match read {
                        Claimed::Read => g.shown_failures.clear(),
                        Claimed::Damaged if g.shown_failures.fail(&q.card_id(), now) => {
                            g.shown_failures.clear();
                            self.refuse_selection(&mut g, &t, &q, None, None, Unapplied::Invalid);
                        }
                        Claimed::Damaged | Claimed::Unavailable => {}
                    }
                }
            }
        }
        let notice_ready = self.poll_notice(&mut g, Some(now));
        let changed = !g.ready.is_empty();
        if notice_ready {
            self.pump(&mut g, now, changed);
        }
        Polled {
            answers: std::mem::take(&mut g.ready),
            status_changed: std::mem::take(&mut g.status_changed),
        }
    }

    /// The open questions that wait for an answer of the user.
    pub fn open(&self) -> Vec<(Triple, Arc<Question>)> {
        let g = self.lock();
        g.queue
            .all()
            .into_iter()
            .filter_map(|t| g.map.get(&t).cloned().map(|q| (t, q)))
            .collect()
    }

    /// What `paddock app status` shows: paused, and the open questions.
    pub fn snapshot(&self) -> (bool, Vec<OpenQuestion>) {
        let g = self.lock();
        let shown = g.queue.shown().cloned();
        let open = g
            .queue
            .all()
            .into_iter()
            .filter_map(|t| {
                let q = g.map.get(&t)?;
                Some(OpenQuestion {
                    app: t.app.clone(),
                    host: t.host.clone(),
                    port: t.port,
                    waiting: q.count(),
                    asked_at: q.asked_at(),
                    shown: shown.as_ref() == Some(&t) && q.published.load(Ordering::SeqCst),
                })
            })
            .collect();
        (g.queue.paused(), open)
    }
}

#[path = "approvals_cards.rs"]
mod cards;
mod notice;
mod presentation;
mod question;
mod result;
mod streak;

/// What a poll found for the answer of the question on the screen.
enum Claimed {
    /// Read: an accepted answer, or none yet.
    Read,
    /// The card or its answer is damaged, missing or cannot be read.
    Damaged,
    /// The store could not be locked: nothing is known.
    Unavailable,
}
mod unapplied;
mod wake;

pub use unapplied::{cli_answer_note, Unapplied};

#[cfg(test)]
mod fake_state;
#[cfg(test)]
mod notice_tests;
#[cfg(test)]
mod unapplied_tests;
#[cfg(test)]
mod wake_tests;

#[cfg(test)]
#[path = "approvals_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "approvals_recovery_tests.rs"]
mod recovery_tests;
