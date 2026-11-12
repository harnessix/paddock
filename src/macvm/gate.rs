//! The firewall of one session, behind the proxy's gate: the mode, the host
//! list of mode `api` or `allowlist`, the rules of the profile, the asker
//! of mode `ask`, the app of the session, and the connection log.
//! `connection` is the decision for one connection, with its log line;
//! `Opened` writes the line of the close with the bytes and the length.
//!
//! In mode `ask` only the app of the session asks (`owner.rs`). A
//! connection of the guest system that no rule allows is denied at once,
//! with the rule word `system`, logged with its host and its process and
//! counted per host in `questions.json`. Pure: tests on Linux with a fake
//! store.

use super::approvals::{Asker, Join, Question, Settled, Triple, Who, BY_RULE, ON_WAIT_EVERY};
use super::ask_status::{self, AskStatus, SystemHost};
use super::connlog::{self, Entry};
use super::firewall::{self, Conn, Decision, Mode, Rule, Rules, Scope, BY_APPROVER, BY_USER};
use super::owner::AppOwner;
use super::proxy_header::Identity;
use crate::broker::ConnectAnswer;
use crate::eventstream::schema::GrantSource;
use crate::proxy::allow::Allowlist;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod asking;
mod decision;
#[path = "gate_held.rs"]
mod held;
mod recheck;
use held::Held;
pub use held::Opened;
#[cfg(test)]
type GrantEventHook = Box<dyn Fn(crate::eventstream::Event)>;

#[cfg(test)]
thread_local! {
    static RECHECK_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static RECHECK_BEFORE_MODE_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static RECHECK_REVALIDATE_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static RECHECK_BEFORE_FILE_LOCK_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static AFTER_POLL_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static HOOK_SELECTION_RECORDED: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static RULE_EFFECTS_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static GRANT_EVENT_HOOK: std::cell::RefCell<Option<GrantEventHook>> = std::cell::RefCell::new(None);
    static MODE_STARTED_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
}

/// The rule words of the log for a decision without a rule id.
pub const RULE_MODE: &str = "mode";
pub const RULE_EGRESS: &str = "egress";
pub const RULE_LISTED: &str = "listed";
pub const RULE_ASK: &str = "ask";
pub const RULE_ONCE: &str = "once";
pub const RULE_TIMEOUT: &str = "timeout";
pub const RULE_STALE: &str = "stale";
/// A connection of the guest system in mode `ask`: denied, never asked.
pub const RULE_SYSTEM: &str = "system";
/// How often the firewall of a session looks for answers when no
/// connection waits.
pub const TICK: Duration = Duration::from_secs(1);

fn firewall_bus() -> Option<&'static crate::eventstream::Bus> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        crate::eventstream::default_bus()
    }
}

fn question_key(t: &Triple, who: &Who, id: Option<&Identity>) -> Triple {
    Triple {
        identity: Some(super::approvals::QuestionIdentity {
            who: who.clone(),
            pid: id.and_then(|i| i.pid),
            path: id.and_then(|i| i.path.clone()),
            // A PID does not identify a process generation. Bind this key
            // to this connection, even when the guest supplied a PID.
            connection: crate::broker::new_request_id(),
        }),
        ..t.clone()
    }
}

pub struct Firewall {
    /// The mode now: `paddock app network` changes it while the session
    /// runs, and every new connection reads it.
    mode: Arc<std::sync::RwLock<Mode>>,
    // A delayed earlier mode change must not undo a later switch to none.
    mode_revision: std::sync::atomic::AtomicU64,
    api: Option<Allowlist>,
    rules: Mutex<Rules>,
    // A connection can refresh the file between two periodic rechecks.
    rules_dirty: std::sync::atomic::AtomicBool,
    // Serialize diagnostic session snapshots without holding the rules lock
    // while the file is written.
    session_write_lock: Mutex<()>,
    asker: Option<Asker>,
    /// The app of the session. `None`: every process may ask (tests).
    owner: Option<AppOwner>,
    system: Mutex<Vec<SystemHost>>,
    // Serialize snapshots and writes so an older snapshot cannot land last.
    status_lock: Mutex<()>,
    status_retry: std::sync::atomic::AtomicBool,
    /// The apps of the guest that showed a window in this session besides
    /// the app: a browser or a sign-in window that the app opened. Their
    /// connections ask like the app's.
    windowed: Mutex<std::collections::BTreeSet<String>>,
    /// The warning of a connection that the guest proxy could not name
    /// is written once per session.
    unknown_said: std::sync::atomic::AtomicBool,
    log: PathBuf,
    /// The client streams of the open connections, by number, so a switch
    /// to mode `none` shuts them down.
    open: Arc<Mutex<std::collections::BTreeMap<u64, Held>>>,
    next_open: std::sync::atomic::AtomicU64,
    /// The profile whose `changes[]` gets the rules of the answers.
    changes: Option<(PathBuf, PathBuf)>,
}

impl Firewall {
    /// A guest app other than the session's showed a window: from now on
    /// its connections ask like the app's. `system` (the guest prompts)
    /// never counts.
    pub fn saw_window_of(&self, app: &str) {
        if app.is_empty() || app == "system" {
            return;
        }
        self.windowed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(app.to_string());
    }

    fn windowed(&self, id: Option<&Identity>) -> bool {
        let Some(id) = id else {
            return false;
        };
        let set = self.windowed.lock().unwrap_or_else(|e| e.into_inner());
        [id.app.as_deref(), id.parent_app.as_deref()]
            .into_iter()
            .flatten()
            .any(|a| set.contains(a))
    }
}

impl Firewall {
    pub fn new(
        mode: Mode,
        api: Option<Allowlist>,
        rules: Rules,
        asker: Option<Asker>,
        log: PathBuf,
    ) -> Self {
        Self {
            mode: Arc::new(std::sync::RwLock::new(mode)),
            mode_revision: std::sync::atomic::AtomicU64::new(0),
            api,
            rules: Mutex::new(rules),
            rules_dirty: std::sync::atomic::AtomicBool::new(false),
            session_write_lock: Mutex::new(()),
            asker,
            owner: None,
            system: Mutex::new(Vec::new()),
            status_lock: Mutex::new(()),
            status_retry: std::sync::atomic::AtomicBool::new(false),
            windowed: Mutex::new(Default::default()),
            unknown_said: std::sync::atomic::AtomicBool::new(false),
            log,
            open: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            next_open: std::sync::atomic::AtomicU64::new(0),
            changes: None,
        }
    }

    /// The rules of the answers go to the grant changes of this profile
    /// folder; `home` shows paths with `~` in the event log.
    pub fn with_changes(mut self, profile_dir: PathBuf, home: PathBuf) -> Self {
        self.changes = Some((profile_dir, home));
        self
    }

    /// A rule an answer added or replaced, into the profile grant changes.
    fn note_rule(&self, r: &Rule, action: &str, by: &str, now: u64) {
        let Some((dir, home)) = &self.changes else {
            return;
        };
        use super::grants::{self, Applies, Change, Source, What};
        let source = if by == BY_APPROVER {
            Source::Approver
        } else if by == "cli" {
            Source::You
        } else {
            Source::QuestionPanel
        };
        let value = super::text::grant_historical(&[r.verdict.word(), r.scope.word()].join(" "));
        let target = format!("{}:{}", r.host, r.port.text()); // not user text
        let c = Change::new(
            now,
            What::Rule,
            action,
            &target,
            &value,
            Applies::Unknown,
            source,
        );
        grants::note_bounded(&super::session::Profile::new(dir.clone()), c, home);
    }

    /// The mode now.
    pub fn mode(&self) -> Mode {
        *self.mode.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Switch the mode and close connections that the new mode denies.
    pub fn set_mode(&self, mode: Mode) -> u64 {
        let revision = self
            .mode_revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        #[cfg(test)]
        MODE_STARTED_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        if mode == Mode::None {
            // None denies every connection without consulting rules or the
            // approval queue, either of which may be waiting on external I/O.
            let mut current = self.mode.write().unwrap_or_else(|e| e.into_inner());
            *current = mode;
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            let closed = open.len() as u64;
            for (_, held) in std::mem::take(&mut *open) {
                let _ = held.stream.shutdown(std::net::Shutdown::Both);
            }
            return closed;
        }
        let _ = self.current_rules(crate::state::now());
        let rules = self.rules();
        let mut current = self.mode.write().unwrap_or_else(|e| e.into_inner());
        if self.mode_revision.load(std::sync::atomic::Ordering::SeqCst) != revision {
            return 0;
        }
        *current = mode;
        let rules = rules.all();
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let mut closed = 0;
        open.retain(|_, held| {
            if held.denied(mode, self.api.as_ref(), &rules) {
                let _ = held.stream.shutdown(std::net::Shutdown::Both);
                closed += 1;
                false
            } else {
                true
            }
        });
        closed
    }

    /// The connections that are open now.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Only the processes of this app ask in mode `ask`.
    pub fn with_owner(mut self, owner: AppOwner) -> Self {
        self.owner = Some(owner);
        self
    }

    /// The file of the open questions, next to the connection log.
    pub fn status_path(&self) -> PathBuf {
        ask_status::path_next_to(&self.log)
    }

    fn write_status(&self) {
        let _status = self.status_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (paused, open) = self.asker.as_ref().map(Asker::snapshot).unwrap_or_default();
        let unapplied = self
            .asker
            .as_ref()
            .map(Asker::unapplied)
            .unwrap_or_default();
        let system = self
            .system
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let s = AskStatus {
            paused,
            open,
            system,
            unapplied,
        };
        self.status_retry.store(
            !ask_status::write(&self.status_path(), &s),
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    /// Read the rules for a decision. A later settle rechecks open questions
    /// if this refresh observed a change.
    fn current_rules(&self, _now: u64) -> Vec<Rule> {
        let (all, changed) = {
            let mut r = self.rules();
            let changed = r.reload_if_changed().unwrap_or_else(|e| {
                log::warn!("macvm: {e:#}");
                false
            });
            (r.all(), changed)
        };
        if changed {
            self.rules_dirty
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        all
    }

    /// A connection of the guest system: counted per host.
    fn count_system(&self, host: &str, port: u16, process: &str) {
        {
            let mut s = self.system.lock().unwrap_or_else(|e| e.into_inner());
            ask_status::count_system(&mut s, host, port, process);
        }
        self.write_status();
    }

    /// The hosts of the guest system denied so far, most denied first.
    pub fn system_denied(&self) -> Vec<SystemHost> {
        self.system
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn rules(&self) -> std::sync::MutexGuard<'_, Rules> {
        self.rules.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn asker(&self) -> Option<&Asker> {
        self.asker.as_ref()
    }

    fn record(&self, e: Entry) {
        if let Err(err) = connlog::record_at(&self.log, e) {
            log::debug!("macvm: cannot write the connection log: {err}");
        }
    }

    /// The decision for one connection, with its log line. `Ok` carries
    /// what to call at the close, `Err` the rule word of the refusal.
    pub fn connection(
        &self,
        host: &str,
        port: u16,
        id: Option<&Identity>,
    ) -> Result<Opened, String> {
        let now = crate::state::now();
        let name = id
            .map(Identity::app_name)
            .unwrap_or_else(|| "unknown".into());
        let conn = match id {
            Some(i) => Conn::of(i, &name, host, port),
            None => Conn::new(&name, host, port),
        };
        let team = conn.team.map(str::to_string);
        let entry = |verdict: &str, rule: &str| {
            Entry::new(now, &name, team.as_deref(), host, port, verdict, rule)
        };
        if self.mode() == Mode::None {
            self.record(entry(connlog::DENY, RULE_MODE));
            return Err(RULE_MODE.into());
        }
        let _ = self.current_rules(now);
        let (mode, decision) = {
            let mut rules = self.rules();
            let _file_lock = match rules.decision_lock() {
                Ok(lock) => lock,
                Err(e) => {
                    log::warn!("macvm: cannot lock firewall rules: {e}");
                    self.record(entry(connlog::DENY, RULE_STALE));
                    return Err(RULE_STALE.into());
                }
            };
            match rules.reload_if_changed() {
                Ok(changed) => {
                    if changed {
                        self.rules_dirty
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                Err(e) => {
                    log::warn!("macvm: {e:#}");
                    self.record(entry(connlog::DENY, RULE_STALE));
                    return Err(RULE_STALE.into());
                }
            }
            let mode = self.mode.read().unwrap_or_else(|e| e.into_inner());
            (
                *mode,
                firewall::decide(*mode, self.api.as_ref(), &rules.all(), &conn),
            )
        };
        match decision {
            Decision::Allow(rule) => {
                let word = rule.unwrap_or_else(|| match mode {
                    Mode::Api | Mode::Allowlist => RULE_LISTED.into(),
                    _ => RULE_EGRESS.into(),
                });
                Ok(self.opened(entry(connlog::ALLOW, &word), true))
            }
            Decision::Deny(rule) => {
                let word = rule.unwrap_or_else(|| RULE_MODE.into());
                self.record(entry(connlog::DENY, &word));
                Err(word)
            }
            Decision::Ask => {
                if self.owner.as_ref().is_some_and(|o| !o.owns(id)) && !self.windowed(id) {
                    // The guest system never asks. A connection whose
                    // process the guest proxy could not name is no proof
                    // of the app either: said once, then counted.
                    if id.is_none_or(|i| i.pid.is_none())
                        && !self
                            .unknown_said
                            .swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        log::warn!("macvm: {}", super::text::ask_unknown_process(host, port));
                    }
                    self.record(entry(connlog::DENY, RULE_SYSTEM));
                    self.count_system(host, port, &name);
                    return Err(RULE_SYSTEM.into());
                }
                let Some(asker) = &self.asker else {
                    self.record(entry(connlog::DENY, RULE_ASK));
                    return Err(RULE_ASK.into());
                };
                self.record(entry(connlog::ASK, RULE_ASK));
                let triple = Triple {
                    identity: None,
                    app: conn.parent_app.unwrap_or(conn.app).to_string(),
                    host: host.to_string(),
                    port,
                };
                let who = Who {
                    app: conn.app.to_string(),
                    parent_app: conn.parent_app.map(str::to_string),
                    team: conn.team.map(str::to_string),
                };
                match self.ask_with_receipt(asker, &triple, who, id, now) {
                    Ok((s, q)) => {
                        let decision = if s.allowed {
                            Ok(self.opened(entry(connlog::ALLOW, &s.word), s.who == BY_RULE))
                        } else {
                            self.record(entry(connlog::DENY, &s.word));
                            Err(s.word)
                        };
                        asker.acknowledge_waiter(&q);
                        decision
                    }
                    Err(word) => {
                        self.record(entry(connlog::DENY, &word));
                        Err(word)
                    }
                }
            }
        }
    }

    fn opened(&self, entry: Entry, require_allow: bool) -> Opened {
        Opened {
            entry,
            log: self.log.clone(),
            started: Instant::now(),
            require_allow,
        }
    }

    /// The gate of the proxy.
    pub fn gate(self: Arc<Self>) -> crate::proxy::Gate {
        crate::proxy::Gate(Arc::new(
            move |host: &str, port: u16, id: Option<&Identity>| {
                let opened = self.connection(host, port, id)?;
                let require_allow = opened.require_allow;
                let mut stale = opened.entry.clone();
                stale.verdict = connlog::DENY.into();
                stale.rule = RULE_STALE.into();
                let n = self
                    .next_open
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let (held, gone) = (self.open.clone(), self.open.clone());
                let fw = self.clone();
                let host = host.to_string();
                let id = id.cloned();
                Ok(crate::proxy::Opened::new(move |up, down| {
                    gone.lock().unwrap_or_else(|e| e.into_inner()).remove(&n);
                    opened.close(up, down)
                })
                .holding(move |client| {
                    if let Ok(stream) = client.try_clone() {
                        let entry = Held {
                            stream,
                            host,
                            port,
                            id,
                            require_allow,
                        };
                        let _ = fw.current_rules(crate::state::now());
                        let mut rules = fw.rules();
                        let file_lock = rules.decision_lock();
                        let invalid = if file_lock.is_err() {
                            true
                        } else {
                            match rules.reload_if_changed() {
                                Ok(changed) => {
                                    if changed {
                                        fw.rules_dirty
                                            .store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    false
                                }
                                Err(_) => true,
                            }
                        };
                        let mode = fw.mode.read().unwrap_or_else(|e| e.into_inner());
                        if invalid || entry.rejected_at_hold(*mode, fw.api.as_ref(), &rules.all()) {
                            let _ = entry.stream.shutdown(std::net::Shutdown::Both);
                            drop((file_lock, rules, mode));
                            fw.record(stale);
                            Err(RULE_STALE.into())
                        } else {
                            held.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(n, entry);
                            Ok(())
                        }
                    } else {
                        let _ = client.shutdown(std::net::Shutdown::Both);
                        fw.record(stale);
                        Err(RULE_STALE.into())
                    }
                }))
            },
        ))
    }
}

/// The answer word of a `ConnectAnswer` for a log line or a test.
pub fn answer_word(a: ConnectAnswer) -> &'static str {
    a.word()
}

/// The mode used when a session's firewall starts. A word that is no
/// mode fails closed: rules never allow traffic under `none`.
pub(crate) fn startup_mode(mode_word: &str) -> Mode {
    Mode::parse(mode_word).unwrap_or(Mode::None)
}

/// The firewall of a session at its start: the mode word of the session,
/// the host list of mode `api` or `allowlist`, the rules file of the
/// profile with its seeds, the asker of mode `ask` (with the approver
/// hook of the system configuration and the marker restart), and a
/// fresh connection log. `pid` is the session helper that asks. `side`
/// is the app of the session ([`app_side`]): only its processes ask, and
/// its icon shows on the panel.
#[allow(clippy::too_many_arguments)]
pub fn build(
    env: &crate::env::HostEnv,
    app: &str,
    profile_dir: &std::path::Path,
    log: PathBuf,
    mode_word: &str,
    allow: &[String],
    pid: u32,
    side: AppSide,
) -> Arc<Firewall> {
    let mode = startup_mode(mode_word);
    // Keep the allowlist available for live network mode changes.
    let api = Some(Allowlist::parse(allow).0);
    let rules_key = super::instance::key(app, super::instance::of_profile(profile_dir));
    let mut rules = Rules::load(env, &rules_key).unwrap_or_else(|e| {
        log::warn!("macvm: {e:#}");
        Rules::broken(Rules::path(env, &rules_key))
    });
    let profile = crate::app::app_profile(env, app).ok();
    let label = profile
        .as_ref()
        .map(crate::app::app_label)
        .unwrap_or_else(|| app.to_string());
    rules.seed(&firewall::seed_hosts(profile.as_ref()), crate::state::now());
    let asker = Some({
        let hook = crate::approver::load_system(&env.user).unwrap_or_else(|e| {
            log::warn!("macvm: the approver is off: {e:#}");
            None
        });
        let on_wait = env.clone();
        Asker::new(env.clone(), app, &label, profile_dir.to_path_buf(), pid)
            .hook(hook)
            .icon(side.icon.clone())
            .on_wait(Box::new(move || crate::mark::spawn(&on_wait)))
    });
    let owner = side.owner;
    connlog::rotate_for_session(&log);
    ask_status::clear(&ask_status::path_next_to(&log));
    let _ = std::fs::remove_file(log.with_file_name("session-rules.json"));
    let _ = std::fs::remove_file(log.with_file_name("session-rules.meta.json"));
    let fw = Arc::new(
        Firewall::new(mode, api, rules, asker, log)
            .with_owner(owner)
            .with_changes(profile_dir.to_path_buf(), env.home.clone()),
    );
    if fw.asker.is_some() {
        start_ticks(&fw);
    }
    fw
}

/// The app of a session, for the firewall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppSide {
    pub owner: AppOwner,
    pub icon: Option<PathBuf>,
}

/// The app side of a session from its host bundle: its bundle id and its
/// two paths in the guest (the copy in the home and the share). Without a
/// bundle nothing of the session asks. A `--bench` session also counts
/// `curl` as the app, for the Mac check of the panel.
pub fn app_side(bundle: Option<&super::appcopy::HostBundle>, bench: bool) -> AppSide {
    let owner = match bundle {
        Some(b) => AppOwner::new(&b.bundle_id, &[&b.guest_target(), &b.guest_source()]),
        None => AppOwner::nobody(),
    };
    AppSide {
        owner: if bench {
            owner.with_bench_tools()
        } else {
            owner
        },
        icon: bundle.and_then(|b| b.icon.clone()),
    }
}

/// A thread that applies the answers that come while no connection
/// waits, and puts the next question up when the rate allows it. It ends
/// with the firewall.
fn start_ticks(fw: &Arc<Firewall>) {
    let weak = Arc::downgrade(fw);
    let spawned = std::thread::Builder::new()
        .name("firewall-asks".into()) // not user text
        .spawn(move || loop {
            std::thread::sleep(TICK);
            let Some(fw) = weak.upgrade() else {
                break;
            };
            fw.settle(crate::state::now());
        });
    if let Err(e) = spawned {
        log::warn!("macvm: no thread for the firewall questions: {e}");
    }
}

#[path = "gate_sleep.rs"]
mod sleep;
pub use sleep::{SleepWatch, SLEEP_MIN_SECS};

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gate_recovery_tests.rs"]
mod recovery_tests;
