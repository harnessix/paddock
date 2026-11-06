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
use crate::proxy::allow::Allowlist;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

pub struct Firewall {
    pub mode: Mode,
    api: Option<Allowlist>,
    rules: Mutex<Rules>,
    asker: Option<Asker>,
    /// The app of the session. `None`: every process may ask (tests).
    owner: Option<AppOwner>,
    system: Mutex<Vec<SystemHost>>,
    /// The warning of a connection that the guest proxy could not name
    /// is written once per session.
    unknown_said: std::sync::atomic::AtomicBool,
    log: PathBuf,
}

/// An allowed connection: its log line waits for the close.
#[derive(Debug)]
pub struct Opened {
    entry: Entry,
    log: PathBuf,
    started: Instant,
}

impl Opened {
    pub fn close(self, up: u64, down: u64) {
        let secs = self.started.elapsed().as_secs();
        let e = self.entry.closed(up, down, secs);
        if let Err(err) = connlog::record_at(&self.log, e) {
            log::debug!("macvm: cannot write the connection log: {err}");
        }
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
            mode,
            api,
            rules: Mutex::new(rules),
            asker,
            owner: None,
            system: Mutex::new(Vec::new()),
            unknown_said: std::sync::atomic::AtomicBool::new(false),
            log,
        }
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
        let (paused, open) = self.asker.as_ref().map(Asker::snapshot).unwrap_or_default();
        let system = self
            .system
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let s = AskStatus {
            paused,
            open,
            system,
        };
        ask_status::write(&self.status_path(), &s);
    }

    /// The rules of a decision. When the file changed, every open
    /// question that a rule now decides ends with that rule.
    fn current_rules(&self, now: u64) -> Vec<Rule> {
        let (all, changed) = {
            let mut r = self.rules();
            let changed = r.reload_if_changed().unwrap_or_else(|e| {
                log::warn!("macvm: {e:#}");
                false
            });
            (r.all(), changed)
        };
        if changed {
            self.recheck(&all, now);
        }
        all
    }

    fn recheck(&self, all: &[Rule], now: u64) {
        let Some(asker) = &self.asker else {
            return;
        };
        let mut any = false;
        for (t, q) in asker.open() {
            let (answer, word) =
                match firewall::decide(self.mode, self.api.as_ref(), all, &q.who.conn(&t)) {
                    Decision::Allow(r) => (
                        ConnectAnswer::AllowOnce,
                        r.unwrap_or_else(|| RULE_LISTED.into()),
                    ),
                    Decision::Deny(r) => {
                        (ConnectAnswer::Deny, r.unwrap_or_else(|| RULE_MODE.into()))
                    }
                    Decision::Ask => continue,
                };
            any = true;
            let s = Settled {
                answer,
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
        let answered = asker.poll(now);
        let any = !answered.is_empty();
        for a in answered {
            let by = if a.who == "approver" {
                BY_APPROVER
            } else {
                BY_USER
            };
            let word = self.apply(a.answer, &a.question.who, &a.triple, by, now);
            let s = Settled {
                answer: a.answer,
                who: a.who,
                word,
            };
            asker.finish(&a.triple, &a.question, s, now);
        }
        if any {
            self.write_status();
        }
    }

    /// The rule of an answer into the session or the file. The rule word
    /// of the log: the id of the new rule, or `once`.
    fn apply(&self, answer: ConnectAnswer, who: &Who, t: &Triple, by: &str, now: u64) -> String {
        match firewall::rule_of_answer(answer, &who.conn(t), by, now) {
            Some(r) => {
                let id = r.id.clone();
                let scope = r.scope;
                let mut rules = self.rules();
                match rules.add(r, false) {
                    Ok(_) if scope == Scope::Always => {
                        if let Err(e) = rules.save() {
                            log::warn!("macvm: {e:#}");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("macvm: {e:#}"),
                }
                id
            }
            None => RULE_ONCE.into(),
        }
    }

    /// Ask about `t` and wait for the answer: `Err` is the rule word of a
    /// refusal without an answer (`timeout`, `stale`).
    pub fn ask(
        &self,
        asker: &Asker,
        t: &Triple,
        who: Who,
        id: Option<&Identity>,
        now: u64,
    ) -> Result<Settled, String> {
        let (q, hook) = match asker.join(t, who, id, now) {
            Join::Once(s) => return Ok(s),
            Join::Stale => {
                self.write_status();
                return Err(RULE_STALE.into());
            }
            Join::Wait(q, hook) => (q, hook),
        };
        if hook {
            if let Some(a) = asker.run_hook(t, &q) {
                let word = self.apply(a, &q.who, t, BY_APPROVER, now);
                let s = Settled {
                    answer: a,
                    who: "approver",
                    word,
                };
                asker.finish(t, &q, s.clone(), now);
                return Ok(s);
            }
            asker.enqueue(t, now);
        }
        self.write_status();
        self.wait_on(asker, t, &q)
    }

    fn wait_on(&self, asker: &Asker, t: &Triple, q: &Arc<Question>) -> Result<Settled, String> {
        let start = Instant::now();
        let mut nudged = Instant::now();
        asker.nudge();
        loop {
            self.settle(crate::state::now());
            if let Some(s) = q.result() {
                return Ok(s);
            }
            if start.elapsed() >= asker.wait() {
                if let Some(s) = asker.timeout(t, q, crate::state::now()) {
                    return Ok(s);
                }
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
        let all = self.current_rules(now);
        let decision = firewall::decide(self.mode, self.api.as_ref(), &all, &conn);
        match decision {
            Decision::Allow(rule) => {
                let word = rule.unwrap_or_else(|| match self.mode {
                    Mode::Api | Mode::Allowlist => RULE_LISTED.into(),
                    _ => RULE_EGRESS.into(),
                });
                Ok(self.opened(entry(connlog::ALLOW, &word)))
            }
            Decision::Deny(rule) => {
                let word = rule.unwrap_or_else(|| RULE_MODE.into());
                self.record(entry(connlog::DENY, &word));
                Err(word)
            }
            Decision::Ask => {
                if self.owner.as_ref().is_some_and(|o| !o.owns(id)) {
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
                    app: conn.parent_app.unwrap_or(conn.app).to_string(),
                    host: host.to_string(),
                    port,
                };
                let who = Who {
                    app: conn.app.to_string(),
                    parent_app: conn.parent_app.map(str::to_string),
                    team: conn.team.map(str::to_string),
                };
                match self.ask(asker, &triple, who, id, now) {
                    Ok(s) if s.answer.allows() => Ok(self.opened(entry(connlog::ALLOW, &s.word))),
                    Ok(s) => {
                        self.record(entry(connlog::DENY, &s.word));
                        Err(s.word)
                    }
                    Err(word) => {
                        self.record(entry(connlog::DENY, &word));
                        Err(word)
                    }
                }
            }
        }
    }

    fn opened(&self, entry: Entry) -> Opened {
        Opened {
            entry,
            log: self.log.clone(),
            started: Instant::now(),
        }
    }

    /// The gate of the proxy.
    pub fn gate(self: Arc<Self>) -> crate::proxy::Gate {
        crate::proxy::Gate(Arc::new(
            move |host: &str, port: u16, id: Option<&Identity>| {
                let opened = self.connection(host, port, id)?;
                Ok(crate::proxy::Opened(Box::new(move |up, down| {
                    opened.close(up, down)
                })))
            },
        ))
    }
}

/// The answer word of a `ConnectAnswer` for a log line or a test.
pub fn answer_word(a: ConnectAnswer) -> &'static str {
    a.word()
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
    let mode = Mode::parse(mode_word).unwrap_or(Mode::Ask);
    let api = matches!(mode, Mode::Api | Mode::Allowlist).then(|| Allowlist::parse(allow).0);
    // Each instance of an app has its own rules file: `NAME--N.toml`.
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
    let asker = (mode == Mode::Ask).then(|| {
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
    let fw = Arc::new(Firewall::new(mode, api, rules, asker, log).with_owner(owner));
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

/// The host slept when the wall clock moved this much further than the
/// clock that stops in sleep.
pub const SLEEP_MIN_SECS: u64 = 30;

/// A watch for a sleep of the host, from the two clocks: the wall clock
/// runs through a sleep, `Instant` does not (`CLOCK_UPTIME_RAW` on macOS).
/// The session loop asks it every few seconds and tells the asker about a
/// wake, so a question from before the sleep is asked again.
pub struct SleepWatch {
    wall: u64,
    mono: Instant,
}

impl SleepWatch {
    pub fn new(now_wall: u64) -> Self {
        Self {
            wall: now_wall,
            mono: Instant::now(),
        }
    }

    /// Whether the two elapsed times show a sleep.
    pub fn slept(wall_elapsed: u64, mono_elapsed: u64) -> bool {
        wall_elapsed > mono_elapsed + SLEEP_MIN_SECS
    }

    /// The wake time, when the host slept since the last check.
    pub fn check(&mut self, now_wall: u64) -> Option<u64> {
        let wall = now_wall.saturating_sub(self.wall);
        let mono = self.mono.elapsed().as_secs();
        self.wall = now_wall;
        self.mono = Instant::now();
        Self::slept(wall, mono).then_some(now_wall)
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
