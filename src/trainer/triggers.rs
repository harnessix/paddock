//! Pure trigger accounting. Advancing the active clock never contacts a vendor.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Signal {
    Periodic { interval: u64 },
    RepeatedFailure { check: String, streak: u64 },
    RepeatedQuestion { question: String, count: u64 },
    NoCommit { since_active_ms: u64 },
    PrematureStatus { sequence: u64 },
    Requested { sequence: u64 },
    BeforeDone { sequence: u64 },
}

impl Signal {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Periodic { .. } => "scheduled active-time review",
            Self::RepeatedFailure { .. } => "repeated failing check",
            Self::RepeatedQuestion { .. } => "repeated question",
            Self::NoCommit { .. } => "extended active work without a commit",
            Self::PrematureStatus { .. } => "completion claimed before the checklist passed",
            Self::Requested { .. } => "worker requested review",
            Self::BeforeDone { .. } => "review before final completion",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub periodic_ms: u64,
    pub no_commit_ms: u64,
    pub min_interval_ms: u64,
    pub budget: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            periodic_ms: 45 * 60 * 1000,
            no_commit_ms: 90 * 60 * 1000,
            min_interval_ms: 5 * 60 * 1000,
            budget: 8,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        let document = crate::manifest::user_config_doc(text)?;
        let Some(table) = document.get("trainer") else {
            return Ok(Self::default());
        };
        let table = table.as_table().context("[trainer] must be a table")?;
        for key in table.keys() {
            if !["enabled", "interval", "no_commit", "min_interval", "budget"]
                .contains(&key.as_str())
            {
                anyhow::bail!("unknown [trainer] key {key}");
            }
        }
        let mut cfg = Self::default();
        if let Some(value) = table.get("enabled") {
            cfg.enabled = value
                .as_bool()
                .context("[trainer] enabled must be true or false")?;
        }
        for (key, destination) in [
            ("interval", &mut cfg.periodic_ms),
            ("no_commit", &mut cfg.no_commit_ms),
            ("min_interval", &mut cfg.min_interval_ms),
        ] {
            if let Some(value) = table.get(key) {
                let text = value
                    .as_str()
                    .with_context(|| format!("[trainer] {key} must be a duration"))?;
                *destination = crate::timer::parse_duration(text)?
                    .checked_mul(1000)
                    .context("trainer duration is too large")?;
            }
        }
        if let Some(value) = table.get("budget") {
            cfg.budget = value
                .as_integer()
                .and_then(|n| u64::try_from(n).ok())
                .context("[trainer] budget must be nonnegative")?;
        }
        Ok(cfg)
    }
    pub fn load(env: &crate::env::HostEnv) -> anyhow::Result<Self> {
        let path = env.paddock_config().join("config.toml");
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestReservation {
    Reserved,
    Already,
    Capped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct FinalReview {
    token: u64,
    started_ms: u64,
    ready: bool,
    #[serde(default)]
    failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalDecision {
    Start(u64),
    Wait,
    Proceed,
    Failed,
    Capped,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub active_ms: u64,
    pub last_commit_active_ms: u64,
    pub last_launch_ms: Option<u64>,
    pub launches: u64,
    periodic_mark: u64,
    failures: BTreeMap<String, u64>,
    failure_generation: BTreeMap<String, u64>,
    questions: BTreeMap<String, u64>,
    no_commit_reported: bool,
    seen: BTreeSet<String>,
    #[serde(default)]
    observed: BTreeMap<String, u64>,
    #[serde(default)]
    manual_requests: BTreeSet<u64>,
    #[serde(default)]
    final_review: Option<FinalReview>,
    pending: VecDeque<(String, Signal)>,
    #[serde(default)]
    reduced_seen: BTreeMap<String, u64>,
}

impl State {
    fn queue(&mut self, key: String, signal: Signal) {
        if self.pending.len() >= 1024 || self.seen.len() >= 8192 {
            return;
        }
        if self.seen.insert(key.clone()) {
            self.pending.push_back((key, signal));
        }
    }
    pub fn active(&mut self, elapsed_ms: u64, cfg: &Config) {
        self.active_ms = self.active_ms.saturating_add(elapsed_ms);
        if let Some(n) = self.active_ms.checked_div(cfg.periodic_ms) {
            if n > self.periodic_mark {
                self.periodic_mark = n;
                self.queue(format!("periodic:{n}"), Signal::Periodic { interval: n });
            }
        }
        if cfg.no_commit_ms > 0
            && !self.no_commit_reported
            && self.active_ms.saturating_sub(self.last_commit_active_ms) >= cfg.no_commit_ms
        {
            self.no_commit_reported = true;
            self.queue(
                format!("no-commit:{}", self.last_commit_active_ms),
                Signal::NoCommit {
                    since_active_ms: self.last_commit_active_ms,
                },
            );
        }
    }
    pub fn commit(&mut self) {
        self.last_commit_active_ms = self.active_ms;
        self.no_commit_reported = false;
    }
    pub fn check(&mut self, check: &str, passed: Option<bool>) {
        if check.is_empty()
            || check.len() > 4096
            || self.failures.len() >= 4096 && !self.failures.contains_key(check)
        {
            return;
        }
        match passed {
            Some(true) => {
                self.failures.remove(check);
                let generation = self.failure_generation.entry(check.into()).or_default();
                *generation = generation.saturating_add(1);
            }
            Some(false) => {
                let n = self.failures.entry(check.into()).or_default();
                *n = n.saturating_add(1);
                if *n == 3 {
                    let generation = self.failure_generation.get(check).copied().unwrap_or(0);
                    let key = format!("failure:{check}:{generation}");
                    self.queue(
                        key,
                        Signal::RepeatedFailure {
                            check: check.into(),
                            streak: 3,
                        },
                    );
                }
            }
            None => {}
        }
    }
    pub fn question(&mut self, question: &str) {
        let normalized = question.trim();
        if normalized.is_empty()
            || normalized.len() > 4096
            || self.questions.len() >= 4096 && !self.questions.contains_key(normalized)
        {
            return;
        }
        let count = self.questions.entry(normalized.into()).or_default();
        *count = count.saturating_add(1);
        if *count == 2 {
            self.queue(
                format!("question:{normalized}"),
                Signal::RepeatedQuestion {
                    question: normalized.into(),
                    count: 2,
                },
            );
        }
    }
    pub fn status(&mut self, sequence: u64, text: &str, checklist_passed: bool) {
        let label = text.trim().to_ascii_lowercase();
        if !checklist_passed && (label.starts_with("done") || label.starts_with("partial")) {
            self.queue(
                format!("premature:{sequence}"),
                Signal::PrematureStatus { sequence },
            );
        }
    }
    pub fn request(&mut self, sequence: u64) {
        self.queue(
            format!("request:{sequence}"),
            Signal::Requested { sequence },
        );
    }
    pub fn before_done(&mut self, sequence: u64) {
        self.queue(
            format!("before-done:{sequence}"),
            Signal::BeforeDone { sequence },
        );
    }
    /// Reserve one launch, including a direct host request.
    pub fn reserve(&mut self, now_ms: u64, cfg: &Config) -> bool {
        if self.launches >= cfg.budget
            || self
                .last_launch_ms
                .is_some_and(|last| now_ms.saturating_sub(last) < cfg.min_interval_ms)
        {
            return false;
        }
        self.last_launch_ms = Some(now_ms);
        self.launches = self.launches.saturating_add(1);
        true
    }

    /// Reserve a worker request once, even when the transport retries it.
    pub fn reserve_request(
        &mut self,
        request: u64,
        now_ms: u64,
        cfg: &Config,
    ) -> RequestReservation {
        if self.manual_requests.contains(&request) {
            return RequestReservation::Already;
        }
        if self.manual_requests.len() >= 8192 || !self.reserve(now_ms, cfg) {
            return RequestReservation::Capped;
        }
        self.manual_requests.insert(request);
        RequestReservation::Reserved
    }

    /// A final report waits for one review, unless the run's cap or budget
    /// forbids a new review. Expired attempts cannot block a worker forever.
    pub fn final_decision(&mut self, now_ms: u64, cfg: &Config) -> FinalDecision {
        if let Some(review) = self.final_review {
            if review.ready {
                self.final_review = None;
                return FinalDecision::Proceed;
            }
            if review.failed {
                if self.launches >= cfg.budget
                    || self
                        .last_launch_ms
                        .is_some_and(|last| now_ms.saturating_sub(last) < cfg.min_interval_ms)
                {
                    return FinalDecision::Failed;
                }
                self.final_review = None;
            } else if now_ms.saturating_sub(review.started_ms) < 20 * 60 * 1000 {
                return FinalDecision::Wait;
            } else {
                // A lost trainer is a failed review, not proof of completion.
                self.final_review = Some(FinalReview {
                    failed: true,
                    ..review
                });
                return FinalDecision::Failed;
            }
        }
        if !self.reserve(now_ms, cfg) {
            return FinalDecision::Capped;
        }
        let token = self.launches;
        self.final_review = Some(FinalReview {
            token,
            started_ms: now_ms,
            ready: false,
            failed: false,
        });
        FinalDecision::Start(token)
    }
    pub fn final_finished(&mut self, token: u64, success: bool) {
        if let Some(review) = &mut self.final_review {
            if review.token == token {
                review.ready = success;
                review.failed = !success;
            }
        }
    }

    /// Take one pending signal only when both cap and budget permit it.
    pub fn take(&mut self, now_ms: u64, cfg: &Config) -> Option<Signal> {
        self.take_adaptive(now_ms, cfg, &BTreeSet::new())
    }
    pub fn take_adaptive(
        &mut self,
        now_ms: u64,
        cfg: &Config,
        reduced: &BTreeSet<String>,
    ) -> Option<Signal> {
        loop {
            let (_, signal) = self.pending.front()?;
            let reason = signal.reason().to_owned();
            let can_reduce = matches!(
                signal,
                Signal::Periodic { .. }
                    | Signal::RepeatedFailure { .. }
                    | Signal::RepeatedQuestion { .. }
                    | Signal::NoCommit { .. }
            );
            if self.launches >= cfg.budget
                || self
                    .last_launch_ms
                    .is_some_and(|last| now_ms.saturating_sub(last) < cfg.min_interval_ms)
            {
                return None;
            }
            if reduced.contains(&reason) && can_reduce {
                let n = self.reduced_seen.entry(reason).or_default();
                *n = n.saturating_add(1);
                if *n % 2 == 1 {
                    self.pending.pop_front();
                    continue;
                }
            }
            if !self.reserve(now_ms, cfg) {
                return None;
            }
            return self.pending.pop_front().map(|(_, signal)| signal);
        }
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// Incorporate only observations not previously seen in this run. Hashes
/// and occurrence numbers keep duplicate failures distinct across rescans.
pub fn observe_records(state: &mut State, source: &super::brief::Source) {
    use sha2::{Digest as _, Sha256};
    let passed = source.checklist_known
        && !source.checklist.is_empty()
        && source.checklist.iter().all(|item| item.passed);
    let mut current = BTreeMap::<String, u64>::new();
    for observation in &source.observations {
        if matches!(
            observation,
            super::brief::Observation::Turn { .. } | super::brief::Observation::Event(_)
        ) {
            continue;
        }
        let Ok(encoded) = serde_json::to_vec(observation) else {
            continue;
        };
        let digest = Sha256::digest(&encoded);
        let key = crate::manifest::hex(&digest);
        let ordinal = current.entry(key.clone()).or_default();
        *ordinal = ordinal.saturating_add(1);
        if *ordinal <= state.observed.get(&key).copied().unwrap_or(0) {
            continue;
        }
        let mut sequence_bytes = [0u8; 8];
        sequence_bytes.copy_from_slice(&digest[..8]);
        let sequence = u64::from_be_bytes(sequence_bytes).wrapping_add(*ordinal);
        match observation {
            super::brief::Observation::Check { name, passed } => state.check(name, *passed),
            super::brief::Observation::Question(text) => state.question(text),
            super::brief::Observation::Status(text) => state.status(sequence, text, passed),
            super::brief::Observation::Commit(_) => state.commit(),
            _ => {}
        }
    }
    for (key, count) in current {
        state
            .observed
            .entry(key)
            .and_modify(|old| *old = (*old).max(count))
            .or_insert(count);
    }
}

/// Serialize decisions with a per-run lock. The closure must not start a
/// trainer or perform other slow operations.
pub fn transact<T>(
    store: &crate::horse::Store,
    run: &str,
    f: impl FnOnce(&mut State) -> T,
) -> anyhow::Result<T> {
    if !crate::view::valid_id(run) {
        anyhow::bail!("invalid run id");
    }
    use std::os::fd::AsRawFd as _;
    let directory = store.root().join("trainer/locks");
    crate::privfs::dir(&directory)?;
    let guard = crate::privfs::append(&directory.join(format!("{run}.lock")))?;
    // SAFETY: guard owns this descriptor until the transaction returns.
    if unsafe { libc::flock(guard.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let key = format!("trainer/triggers/{run}.json");
    let mut state = match store.read_ref(&key)? {
        Some(bytes) => serde_json::from_slice(&bytes)?,
        None => State::default(),
    };
    let result = f(&mut state);
    store.write_ref(&key, &serde_json::to_vec(&state)?)?;
    Ok(result)
}

/// Observe a target and reserve at most one launch atomically. Callers start
/// the sandbox only after this function releases the store lock.
pub fn tick(
    store: &crate::horse::Store,
    run: &str,
    source: &super::brief::Source,
    active_ms: u64,
    now_ms: u64,
    cfg: &Config,
) -> anyhow::Result<Option<Signal>> {
    transact(store, run, |state| {
        observe_records(state, source);
        if active_ms > 0 {
            state.active(active_ms, cfg);
        }
        if cfg.enabled {
            state.take(now_ms, cfg)
        } else {
            None
        }
    })
}

/// As tick, with automatic trigger rates adjusted by explicit feedback.
pub fn tick_adaptive(
    store: &crate::horse::Store,
    run: &str,
    source: &super::brief::Source,
    active_ms: u64,
    now_ms: u64,
    cfg: &Config,
    reduced: &BTreeSet<String>,
) -> anyhow::Result<Option<Signal>> {
    transact(store, run, |state| {
        observe_records(state, source);
        if active_ms > 0 {
            state.active(active_ms, cfg);
        }
        if cfg.enabled {
            state.take_adaptive(now_ms, cfg, reduced)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_feedback_reduces_only_matching_automatic_triggers() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 1,
            no_commit_ms: 0,
            min_interval_ms: 0,
            budget: 9,
        };
        let mut state = State::default();
        let reduced = BTreeSet::from(["scheduled active-time review".to_string()]);
        state.active(1, &cfg);
        assert!(state.take_adaptive(1, &cfg, &reduced).is_none());
        state.active(1, &cfg);
        assert!(matches!(
            state.take_adaptive(2, &cfg, &reduced),
            Some(Signal::Periodic { .. })
        ));
        state.request(42);
        assert!(matches!(
            state.take_adaptive(3, &cfg, &reduced),
            Some(Signal::Requested { .. })
        ));
        assert_eq!(state.launches, 2);
    }

    #[test]
    fn final_report_waits_once_for_its_review_and_obeys_budget() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 0,
            budget: 1,
        };
        let mut state = State::default();
        let FinalDecision::Start(token) = state.final_decision(100, &cfg) else {
            panic!("review did not start")
        };
        assert_eq!(state.final_decision(101, &cfg), FinalDecision::Wait);
        state.final_finished(token + 1, true);
        assert_eq!(state.final_decision(102, &cfg), FinalDecision::Wait);
        state.final_finished(token, true);
        assert_eq!(state.final_decision(103, &cfg), FinalDecision::Proceed);
        assert_eq!(state.final_decision(104, &cfg), FinalDecision::Capped);
    }

    #[test]
    fn failed_final_review_does_not_approve_completion() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 10,
            budget: 2,
        };
        let mut state = State::default();
        let FinalDecision::Start(first) = state.final_decision(100, &cfg) else {
            panic!("review did not start")
        };
        state.final_finished(first, false);
        assert_eq!(state.final_decision(101, &cfg), FinalDecision::Failed);
        let FinalDecision::Start(second) = state.final_decision(110, &cfg) else {
            panic!("review did not retry")
        };
        state.final_finished(second, false);
        assert_eq!(state.final_decision(121, &cfg), FinalDecision::Failed);
    }

    #[test]
    fn worker_request_retries_do_not_spend_the_budget_twice() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 0,
            budget: 1,
        };
        let mut state = State::default();
        assert_eq!(
            state.reserve_request(3, 100, &cfg),
            RequestReservation::Reserved
        );
        assert_eq!(
            state.reserve_request(3, 101, &cfg),
            RequestReservation::Already
        );
        assert_eq!(
            state.reserve_request(4, 102, &cfg),
            RequestReservation::Capped
        );
        assert_eq!(state.launches, 1);
    }

    #[test]
    fn direct_requests_share_the_same_cap_and_budget() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 10,
            budget: 2,
        };
        let mut state = State::default();
        assert!(state.reserve(100, &cfg));
        assert!(!state.reserve(109, &cfg));
        assert!(state.reserve(110, &cfg));
        assert!(!state.reserve(1000, &cfg));
    }

    #[test]
    fn configuration_is_user_scoped_and_checked() {
        let defaults = Config::parse("").unwrap();
        assert_eq!(defaults.periodic_ms, 45 * 60 * 1000);
        let cfg = Config::parse("[trainer]\ninterval = \"2m\"\nno_commit = \"1h\"\nmin_interval = \"30s\"\nbudget = 2\n").unwrap();
        assert_eq!(
            (
                cfg.periodic_ms,
                cfg.no_commit_ms,
                cfg.min_interval_ms,
                cfg.budget
            ),
            (120_000, 3_600_000, 30_000, 2)
        );
        assert!(Config::parse("[trainer]\nbudget = -1\n").is_err());
        assert!(Config::parse("[trainer]\ntypo = true\n").is_err());
    }

    #[test]
    fn rescans_do_not_repeat_a_failure_or_question() {
        use super::super::brief::{Observation, Source};
        let mut source = Source::default();
        for _ in 0..3 {
            source.observations.push(Observation::Check {
                name: "cargo test".into(),
                passed: Some(false),
            });
        }
        for _ in 0..2 {
            source
                .observations
                .push(Observation::Question("Which branch?".into()));
        }
        source
            .observations
            .push(Observation::Status("done: partial".into()));
        let mut state = State::default();
        observe_records(&mut state, &source);
        observe_records(&mut state, &source);
        assert_eq!(state.pending(), 3);
    }

    #[test]
    fn concurrent_reservations_cannot_exceed_the_budget() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = crate::horse::Store::open(&tmp.path().join("store"))?;
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 0,
            budget: 1,
        };
        transact(&store, "test-run", |state| {
            state.request(1);
            state.request(2);
        })?;
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    transact(&store, "test-run", |state| state.take(100, &cfg)).unwrap()
                })
            })
            .collect();
        let taken = threads
            .into_iter()
            .filter_map(|thread| thread.join().unwrap())
            .count();
        assert_eq!(taken, 1);
        let state = transact(&store, "test-run", |state| state.clone())?;
        assert_eq!(state.launches, 1);
        assert_eq!(state.pending(), 1);
        Ok(())
    }

    #[test]
    fn scripted_stalls_queue_once_and_caps_preserve_pending_signals() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 45,
            no_commit_ms: 1000,
            min_interval_ms: 10,
            budget: 3,
        };
        let mut state = State::default();
        state.check("cargo test", Some(false));
        state.check("cargo test", Some(false));
        state.check("cargo test", Some(false));
        state.check("cargo test", Some(false));
        state.question("Which branch?");
        state.question("Which branch?");
        state.question("Which branch?");
        state.status(9, "done: partial", false);
        state.status(9, "done: partial", false);
        assert_eq!(state.pending(), 3);
        assert!(matches!(
            state.take(100, &cfg),
            Some(Signal::RepeatedFailure { .. })
        ));
        assert!(state.take(109, &cfg).is_none());
        assert!(matches!(
            state.take(110, &cfg),
            Some(Signal::RepeatedQuestion { .. })
        ));
        assert!(matches!(
            state.take(120, &cfg),
            Some(Signal::PrematureStatus { sequence: 9 })
        ));
        state.request(20);
        assert!(state.take(130, &cfg).is_none());
        assert_eq!(state.pending(), 1);
    }

    #[test]
    fn active_clock_ignores_wall_time_and_resets_failure_and_commit_streaks() {
        let cfg = Config {
            enabled: true,
            periodic_ms: 45,
            no_commit_ms: 60,
            min_interval_ms: 0,
            budget: 20,
        };
        let mut state = State::default();
        state.active(44, &cfg);
        assert!(state.take(1_000_000, &cfg).is_none());
        state.active(1, &cfg);
        assert!(matches!(
            state.take(1_000_001, &cfg),
            Some(Signal::Periodic { interval: 1 })
        ));
        state.active(15, &cfg);
        assert!(matches!(
            state.take(1_000_002, &cfg),
            Some(Signal::NoCommit { .. })
        ));
        state.commit();
        for _ in 0..3 {
            state.check("test", Some(false));
        }
        assert!(matches!(
            state.take(1_000_003, &cfg),
            Some(Signal::RepeatedFailure { .. })
        ));
        state.check("test", Some(true));
        for _ in 0..3 {
            state.check("test", Some(false));
        }
        assert!(matches!(
            state.take(1_000_004, &cfg),
            Some(Signal::RepeatedFailure { .. })
        ));
        let restored: State =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(restored.launches, 4);
    }
}
