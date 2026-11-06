//! The `ask` of the firewall: one question per (app, host, port), and at
//! most one of them per session on the screen. The question on the screen
//! is a card in the approvals store of the host, which the marker shows as
//! a panel and `paddock approvals answer` answers from a terminal. The
//! other open questions wait in the queue of `ask_queue.rs`. The session
//! helper (`paddock-vm app`) holds each connection while it waits.
//!
//! The rules:
//! - A second connection to the same triple waits on the same question.
//!   Every waiter gets the answer.
//! - After `WAIT` with no answer the connection is denied once and the
//!   question stays open, on the screen or in the queue. An answer that
//!   comes when nobody waits is kept: a rule for the `session` and
//!   `always` answers, and for `allow-once` and `deny` the next connection
//!   of the triple gets it.
//! - A question asked before the last wake of the host is denied once
//!   and asked again (a fresh `asked_at`).
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
use super::ask_status::OpenQuestion;
use super::firewall::Conn;
use super::proxy_header::Identity;
use super::text;
use crate::broker::{self, Connect, ConnectAnswer, Pending, CONNECT_CAPABILITY};
use crate::env::HostEnv;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long a connection waits for the answer.
pub const WAIT: Duration = Duration::from_secs(60);
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
}

/// The guest process of the first connection of a question, for the rule
/// of its answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

/// The end of a question: the answer, who gave it, and the rule word of
/// the log (a rule id, or `once`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub answer: ConnectAnswer,
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
    result: Mutex<Option<Settled>>,
}

impl Question {
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

/// What a connection gets from [`Asker::join`].
pub enum Join {
    /// Wait on this question. `true`: it is new and the approver hook
    /// runs first; [`Asker::enqueue`] queues it when the hook abstains.
    Wait(Arc<Question>, bool),
    /// An answer that came while nobody waited.
    Once(Settled),
    /// The question is older than the last wake: deny once, asked again.
    Stale,
}

/// An answer that [`Asker::poll`] found, for the firewall to apply.
pub struct Answered {
    pub triple: Triple,
    pub question: Arc<Question>,
    pub answer: ConnectAnswer,
    pub who: &'static str,
}

type OnWait = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    map: HashMap<Triple, Arc<Question>>,
    queue: Queue,
    /// The id of the notice card while asking is paused.
    notice: Option<String>,
    /// Answers that came while nobody waited, for the next connection.
    once: HashMap<Triple, Settled>,
}

/// The asker of one session.
pub struct Asker {
    env: HostEnv,
    /// The app profile name.
    profile: String,
    /// The name of the app in the question: `ChatGPT`.
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
        self.last_wake.fetch_max(now, Ordering::SeqCst);
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

    /// The card of a question on the screen.
    pub fn card(&self, q: &Question, t: &Triple, more: u64) -> Pending {
        let why_path = PathBuf::from(q.path.clone().unwrap_or_else(|| t.app.clone()));
        let mut detail = text::ask_panel(&self.label, &t.host, t.port, &why_path);
        // One read: the detail text and the count field must agree.
        let waiting = q.count();
        for line in [text::ask_count(waiting), text::ask_more(more)] {
            if !line.is_empty() {
                detail.push('\n');
                detail.push_str(&line);
            }
        }
        Pending {
            id: q.id.clone(),
            sandbox: format!("macvm-{}", self.profile), // not user text
            harness: self.profile.clone(),
            project: self.profile_dir.clone(),
            capability: CONNECT_CAPABILITY.into(),
            summary: text::ask_summary(&self.label, &t.host, t.port),
            detail,
            created: q.asked_at(),
            session_allowed: true,
            choices: ConnectAnswer::ALL
                .iter()
                .map(|a| a.word().to_string())
                .collect(),
            connect: Some(Connect {
                app: t.app.clone(),
                team: q.who.team.clone(),
                host: t.host.clone(),
                port: t.port,
                count: waiting,
                asked_at: q.asked_at(),
                pid: self.pid,
                path: q.path.clone(),
                profile: self.profile.clone(),
                label: self.label.clone(),
                more,
                notice: false,
                icon: self.icon.clone(),
            }),
            ..Default::default()
        }
    }

    /// The notice card of a paused session.
    fn notice_card(&self, id: &str, open: usize, now: u64) -> Pending {
        Pending {
            id: id.to_string(),
            sandbox: format!("macvm-{}", self.profile), // not user text
            harness: self.profile.clone(),
            project: self.profile_dir.clone(),
            capability: CONNECT_CAPABILITY.into(),
            summary: text::asks_notice_summary(&self.label, open),
            detail: text::asks_notice_detail(&self.label, &self.profile, open),
            created: now,
            choices: [ConnectAnswer::AllowOnce, ConnectAnswer::Deny]
                .iter()
                .map(|a| a.word().to_string())
                .collect(),
            connect: Some(Connect {
                app: self.label.clone(),
                asked_at: now,
                pid: self.pid,
                profile: self.profile.clone(),
                label: self.label.clone(),
                more: open as u64,
                notice: true,
                icon: self.icon.clone(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn write_card(&self, p: &Pending) {
        if let Err(e) = broker::write_pending(&self.env, p) {
            log::warn!("macvm: cannot write the connect question {}: {e}", p.id);
        }
    }

    fn remove_files(&self, id: &str) {
        let dir = broker::approvals_dir(&self.env);
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
        let _ = std::fs::remove_file(dir.join(format!("{id}.answer")));
    }

    /// The answer in the store, when there is one. A file that holds no
    /// answer counts as deny.
    fn read_answer(&self, id: &str) -> Option<ConnectAnswer> {
        let file = broker::approvals_dir(&self.env).join(format!("{id}.answer"));
        let text = std::fs::read_to_string(file).ok()?;
        Some(ConnectAnswer::parse_file(&text).unwrap_or(ConnectAnswer::Deny))
    }

    /// Bring the store up to date with the queue. `rewrite`: the card on
    /// the screen changed (its count or the questions behind it).
    fn pump(&self, g: &mut Inner, now: u64, rewrite: bool) {
        let mut shown_now = false;
        for step in g.queue.pump(now) {
            match step {
                Step::Show(t) => {
                    shown_now = true;
                    if let Some(q) = g.map.get(&t) {
                        self.write_card(&self.card(q, &t, g.queue.more() as u64));
                    }
                }
                Step::Withdraw(t) => {
                    if let Some(q) = g.map.get(&t) {
                        self.remove_files(&q.id);
                    }
                }
                Step::Pause => {
                    let open = g.queue.open();
                    log::warn!(
                        "macvm: {}",
                        text::asks_paused(&self.label, &self.profile, open)
                    );
                    let id = broker::new_request_id();
                    self.write_card(&self.notice_card(&id, open, now));
                    g.notice = Some(id);
                }
            }
        }
        if rewrite && !shown_now {
            if let Some(t) = g.queue.shown().cloned() {
                if let Some(q) = g.map.get(&t) {
                    self.write_card(&self.card(q, &t, g.queue.more() as u64));
                }
            }
            if let Some(id) = g.notice.clone() {
                self.write_card(&self.notice_card(&id, g.queue.open(), now));
            }
        }
    }

    /// A connection to `t` asks: join the open question of the triple, or
    /// open a new one.
    pub fn join(&self, t: &Triple, who: Who, id: Option<&Identity>, now: u64) -> Join {
        let mut g = self.lock();
        if let Some(s) = g.once.remove(t) {
            return Join::Once(s);
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
                    result: Mutex::new(None),
                });
                g.map.insert(t.clone(), q.clone());
                (q, true)
            }
        };
        if !fresh && q.asked_at() < self.last_wake() && !q.taken.load(Ordering::SeqCst) {
            // The user slept through the panel: deny this one, ask anew.
            q.asked_at.store(now, Ordering::SeqCst);
            self.pump(&mut g, now, true);
            return Join::Stale;
        }
        q.count.fetch_add(1, Ordering::SeqCst);
        if fresh && self.hook.is_some() {
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
        if g.map.contains_key(t) {
            g.queue.add(t.clone());
        }
        self.pump(&mut g, now, true);
    }

    /// Look for the answers of the question on the screen and of the
    /// notice, and put the next question up when the rate allows it.
    pub fn poll(&self, now: u64) -> Vec<Answered> {
        let mut g = self.lock();
        let mut out = Vec::new();
        if let Some(t) = g.queue.shown().cloned() {
            if let Some(q) = g.map.get(&t).cloned() {
                if let Some(a) = self.read_answer(&q.id) {
                    q.taken.store(true, Ordering::SeqCst);
                    self.remove_files(&q.id);
                    g.queue.remove(&t);
                    out.push(Answered {
                        triple: t,
                        question: q,
                        answer: a,
                        who: "user",
                    });
                }
            }
        }
        if let Some(id) = g.notice.clone() {
            if let Some(a) = self.read_answer(&id) {
                self.remove_files(&id);
                g.notice = None;
                let deny_all = !a.allows();
                let before = out.len();
                if deny_all {
                    for t in g.queue.take_all() {
                        if let Some(q) = g.map.get(&t).cloned() {
                            q.taken.store(true, Ordering::SeqCst);
                            self.remove_files(&q.id);
                            out.push(Answered {
                                triple: t,
                                question: q,
                                answer: ConnectAnswer::Deny,
                                who: "user",
                            });
                        }
                    }
                } else {
                    g.queue.resume();
                }
                let n = if deny_all {
                    out.len() - before
                } else {
                    g.queue.open()
                };
                log::info!("macvm: {}", text::asks_resumed(deny_all, n));
            }
        }
        let changed = !out.is_empty();
        self.pump(&mut g, now, changed);
        out
    }

    /// The question is decided: every waiter gets `s`, the question and
    /// its files go. An answer of the user or the approver that came when
    /// nobody waited and makes no rule is kept for the next connection.
    pub fn finish(&self, t: &Triple, q: &Arc<Question>, s: Settled, now: u64) {
        let mut g = self.lock();
        if g.map.get(t).is_some_and(|m| Arc::ptr_eq(m, q)) {
            g.map.remove(t);
        }
        g.queue.remove(t);
        self.remove_files(&q.id);
        let once = matches!(s.answer, ConnectAnswer::AllowOnce | ConnectAnswer::Deny);
        if q.count() == 0 && once && s.who != BY_RULE {
            g.once.insert(t.clone(), s.clone());
        }
        *q.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
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
        if g.queue.shown() == Some(t) {
            self.pump(&mut g, now, true);
        }
        None
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
                    shown: shown.as_ref() == Some(&t),
                })
            })
            .collect();
        (g.queue.paused(), open)
    }
}

#[cfg(test)]
#[path = "approvals_tests.rs"]
mod tests;
