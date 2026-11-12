//! Approval cards, answers and the approver.

use crate::env::HostEnv;
#[cfg(test)]
use crate::state;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "approval_store.rs"]
mod store;
pub(crate) use store::{
    abandon_owner, claim_connect, claim_connect_exact, claim_connect_question, close_pending,
    confirm_pause_resume, is_store_lock_error, record_connect_outcome_bound,
    record_hook_connect_selection, record_pause_outcome, record_wake_selection,
    retire_connect_outcome, take_answer, update_pending, withdraw_unanswered, UNVERIFIED_RULE,
};
pub use store::{
    approvals_dir, connect_card_retired, pending, pending_checked, pending_report, write_pending,
    AffectedDecision, ApplicationScope, ApprovalUnresolved, ConnectOutcome, LocalApplication,
    PendingReport,
};
use store::{close_owned, lock_approvals, try_write_pending, PublishError};
#[cfg(test)]
use store::{reap_stale, retry_interrupted};
#[cfg(test)]
pub(crate) use store::{record_connect_outcome, CARD_MAIL_HOOK, OUTCOME_TIME, PID_LIVENESS_HOOK};

#[path = "approval_notify.rs"]
mod notifications;
pub use notifications::{approval_note, approval_ui_active, approval_ui_file};
use notifications::{notify_choices, notify_with_actions};

/// A request that waits for the user, as `paddock approvals --json` shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub sandbox: String,
    pub harness: String,
    pub project: PathBuf,
    pub capability: String,
    /// One line for a list.
    pub summary: String,
    /// The exact thing that will happen, for example the full command line.
    pub detail: String,
    pub reason: String,
    pub created: u64,
    /// Unix time when the request stops waiting and times out.
    #[serde(default)]
    pub expires: u64,
    /// Whether "allow for this session" is offered.
    pub session_allowed: bool,
    /// For a request from a sandbox that another sandbox started: the
    /// sandboxes above it, the first one started by the user, the parent last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub started_by: Vec<String>,
    /// For a question (`/dev/paddock/ask`): short answers that the card
    /// offers as buttons. The user can always type another answer. For a
    /// card of paddock itself (`spawn-timeout`): the only answers, besides
    /// deny.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// For a `connect` question of the firewall of the macOS guest route:
    /// the connection that waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<Connect>,
}

/// The capability name of a question of the firewall of paddock's macOS
/// guest: a desktop app wants to connect to a host in network mode `ask`. The
/// answer is one of the five `ConnectAnswer` words. The session helper that
/// asks keeps the question in the store until it is answered or the session
/// ends, so it survives a restart of paddock and of the marker on the host.
pub const CONNECT_CAPABILITY: &str = "connect";

/// The connection behind a `connect` question.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connect {
    /// The bundle identifier of the guest process, or its file name.
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub host: String,
    pub port: u16,
    /// The connections that wait on this answer.
    pub count: u64,
    /// Unix time of the question, or of its last re-ask after a sleep of
    /// the host.
    pub asked_at: u64,
    /// The session helper (`paddock-vm app`) that asks. When it is gone,
    /// the question goes with it.
    pub pid: u32,
    /// The executable in the guest, for the `Why?` line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The app profile, whose rules file an `always` answer writes.
    pub profile: String,
    /// The name of the app in the title of the panel: `ChatGPT`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The open questions of the same session that wait behind this one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: u64,
    /// The notice of a session that asks too much, not a question about
    /// one host: `deny` denies every open question once, `allow-once`
    /// goes on with one question at a time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notice: bool,
    /// The icon file of the app on the host, for the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<std::path::PathBuf>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The five answers of a `connect` question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectAnswer {
    AllowOnce,
    AllowSite,
    AllowEverything,
    AllowSession,
    AllowAlways,
    Deny,
    DenyAlways,
}

impl ConnectAnswer {
    pub const ALL: [ConnectAnswer; 7] = [
        ConnectAnswer::AllowOnce,
        ConnectAnswer::AllowSite,
        ConnectAnswer::AllowEverything,
        ConnectAnswer::AllowSession,
        ConnectAnswer::AllowAlways,
        ConnectAnswer::Deny,
        ConnectAnswer::DenyAlways,
    ];

    /// The word of the answer: `allow-once`, ..., `deny-always`.
    pub fn word(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow-once",
            Self::AllowSite => "allow-site",
            Self::AllowEverything => "allow-everything",
            Self::AllowSession => "allow-session",
            Self::AllowAlways => "allow-always",
            Self::Deny => "deny",
            Self::DenyAlways => "deny-always",
        }
    }

    pub fn allows(self) -> bool {
        matches!(
            self,
            Self::AllowOnce
                | Self::AllowSite
                | Self::AllowEverything
                | Self::AllowSession
                | Self::AllowAlways
        )
    }

    /// The answer of a plain card answer: `once` allows once, `session`
    /// allows for the session, `deny` denies once. `deny-session` has no
    /// counterpart: a session-long deny is a rule of the CLI.
    pub fn from_answer(a: Answer) -> Result<Self> {
        Ok(match a {
            Answer::Once => Self::AllowOnce,
            Answer::Session => Self::AllowSession,
            Answer::Deny => Self::Deny,
            Answer::DenySession => bail!("{}", crate::macvm::text::connect_answers()),
        })
    }

    /// The answer in an answer file: `{"reply":"allow-always"}` from the
    /// panel or `paddock approvals answer`, or a plain card answer.
    pub fn parse_file(text: &str) -> Option<Self> {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(r) = v.get("reply").and_then(|r| r.as_str()) {
                return r.parse().ok();
            }
        }
        AnswerFile::parse(text).and_then(|(a, _)| Self::from_answer(a).ok())
    }
}

impl std::str::FromStr for ConnectAnswer {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|a| a.word() == s)
            .ok_or_else(|| anyhow::anyhow!("{}", crate::macvm::text::connect_answers()))
    }
}

/// The capability name of a question from `/dev/paddock/ask` on the
/// approval card. It is not a host capability: the answer is text for the
/// agent, and nothing runs on the host.
pub const ASK_CAPABILITY: &str = "ask";
/// The capability name of the card for a spawned child at its time limit
/// (`[spawn] on_timeout = "ask"`). The answer is one of its choices (more
/// time), or deny (stop).
pub const TIMEOUT_CAPABILITY: &str = "spawn-timeout";
/// The capability name of the first-use card of a paid device. The answer
/// is one of its choices (the budget of the run), or deny.
pub const BUDGET_CAPABILITY: &str = "device-budget";
/// The longest typed answer to a question, in characters.
pub const MAX_REPLY: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Once,
    Session,
    Deny,
    /// Deny this request and every same request of this sandbox for the
    /// rest of its session, without a new card.
    #[serde(rename = "deny-session")]
    DenySession,
}

impl Answer {
    pub fn denies(self) -> bool {
        matches!(self, Self::Deny | Self::DenySession)
    }
}

/// The longest note for the agent on a denial, in characters.
pub const MAX_NOTE: usize = 300;

/// The note of a denial, made safe: one line, no control or invisible
/// characters, at most `MAX_NOTE` characters. None when nothing is left.
pub fn clean_note(s: &str) -> Option<String> {
    let line: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let t: String = clean_text(&line).trim().chars().take(MAX_NOTE).collect();
    let t = t.trim_end().to_string();
    (!t.is_empty()).then_some(t)
}

/// The message of a denial for the agent: the user's note when there is
/// one, else `plain`.
pub fn denied_message(plain: &str, note: Option<&str>) -> String {
    match note {
        Some(n) => format!("the user denied this request: \"{n}\""),
        None => plain.to_string(),
    }
}

/// An answer with the user's note on a denial, and who gave it ("user" or
/// "agent").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub answer: Answer,
    pub note: Option<String>,
    pub who: &'static str,
}

/// The answer file: a plain answer (`"deny"`), or an answer with a note.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(in crate::broker) enum AnswerFile {
    Plain(Answer),
    Noted {
        answer: Answer,
        #[serde(default)]
        note: String,
    },
}

impl AnswerFile {
    pub(in crate::broker) fn parse(text: &str) -> Option<(Answer, Option<String>)> {
        match serde_json::from_str::<AnswerFile>(text).ok()? {
            AnswerFile::Plain(a) => Some((a, None)),
            AnswerFile::Noted { answer, note } => {
                Some((answer, clean_note(&note).filter(|_| answer.denies())))
            }
        }
    }
}

impl std::str::FromStr for Answer {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "once" => Ok(Self::Once),
            "session" => Ok(Self::Session),
            "deny" => Ok(Self::Deny),
            "deny-session" => Ok(Self::DenySession),
            _ => bail!("answer must be once, session, deny or deny-session"),
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Answer a `connect` question of the firewall. The panel, the CLI and
/// the notification all come here, so they never disagree. Only the host
/// can call this, like `answer`.
pub fn answer_connect(env: &HostEnv, id: &str, a: ConnectAnswer) -> Result<()> {
    answer_connect_origin(env, id, a, false)
}

/// Answer a connection question from the approvals command.
pub fn answer_connect_cli(env: &HostEnv, id: &str, a: ConnectAnswer) -> Result<()> {
    answer_connect_origin(env, id, a, true)
}

fn answer_connect_origin(env: &HostEnv, id: &str, a: ConnectAnswer, cli: bool) -> Result<()> {
    if !valid_id(id) {
        bail!("invalid request id");
    }
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let p = read_pending_locked(&dir, id)?;
    answer_connect_locked(&dir, id, &p, a, cli)
}

fn answer_connect_locked(
    dir: &Path,
    id: &str,
    p: &Pending,
    a: ConnectAnswer,
    cli: bool,
) -> Result<()> {
    if p.capability != CONNECT_CAPABILITY {
        bail!("{id} is not a connect question: answer it with once, session or deny");
    }
    if p.connect.as_ref().is_some_and(|c| c.notice)
        && !matches!(a, ConnectAnswer::AllowOnce | ConnectAnswer::Deny)
    {
        bail!("this notice takes allow-once or deny");
    }
    let mut answer =
        serde_json::json!({ "reply": a.word(), "source": if cli { "cli" } else { "panel" } });
    // Bind every new selection to the card read under this lock.
    answer["request"] = serde_json::to_value(p)?;
    write_answer(dir, id, answer.to_string())
}

/// Record the user's answer. Only the host can call this: the approvals
/// directory is not mounted in any sandbox.
pub fn answer(env: &HostEnv, id: &str, a: Answer) -> Result<()> {
    answer_with_note(env, id, a, None)
}

/// Record the user's answer with a note for the agent. Only a denial
/// carries a note.
pub fn answer_with_note(env: &HostEnv, id: &str, a: Answer, note: Option<&str>) -> Result<()> {
    answer_with_note_origin(env, id, a, note, false)
}

pub fn answer_with_note_cli(env: &HostEnv, id: &str, a: Answer, note: Option<&str>) -> Result<()> {
    answer_with_note_origin(env, id, a, note, true)
}

fn answer_with_note_origin(
    env: &HostEnv,
    id: &str,
    a: Answer,
    note: Option<&str>,
    cli: bool,
) -> Result<()> {
    if !valid_id(id) {
        bail!("invalid request id");
    }
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let p = read_pending_locked(&dir, id)?;
    if p.capability == CONNECT_CAPABILITY {
        if note.is_some() {
            bail!("a connect question takes no note");
        }
        return answer_connect_locked(&dir, id, &p, ConnectAnswer::from_answer(a)?, cli);
    }
    if p.capability == ASK_CAPABILITY && a != Answer::Deny {
        bail!("{id} is a question: answer it with --text TEXT, or dismiss it with deny");
    }
    if p.capability == ASK_CAPABILITY && a == Answer::DenySession {
        bail!("{id} is a question: dismiss it with deny");
    }
    if p.capability != ASK_CAPABILITY && !p.choices.is_empty() && a != Answer::Deny {
        bail!(
            "{id} offers choices: answer it with --text {}, or deny",
            p.choices.join("|")
        );
    }
    if a == Answer::Session && !p.session_allowed {
        bail!("this request can only be allowed once");
    }
    if a == Answer::DenySession && !p.session_allowed {
        bail!("this request has no session answer: deny it with deny");
    }
    if note.is_some() && !a.denies() {
        bail!("only a denial carries a note");
    }
    let file = match note.and_then(clean_note) {
        Some(note) => AnswerFile::Noted { answer: a, note },
        None => AnswerFile::Plain(a),
    };
    write_answer(&dir, id, serde_json::to_string(&file)?)?;
    Ok(())
}

fn read_pending_locked(dir: &Path, id: &str) -> Result<Pending> {
    let pending = dir.join(format!("{id}.json"));
    let p: Pending = serde_json::from_str(&store::read_evidence_text(pending)?)?;
    if p.id != id {
        bail!("invalid pending request {id}");
    }
    store::require_live_owner(dir, &p)?;
    Ok(p)
}

/// Write the answer file of `id` whole, at once, and only the first time:
/// a poller never reads half an answer, and a second answer does not
/// replace the first.
fn write_answer(dir: &Path, id: &str, text: String) -> Result<()> {
    #[cfg(test)]
    ANSWER_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook();
        }
    });
    match crate::privfs::write_new(&dir.join(format!("{id}.answer")), text) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("{id} was already answered")
        }
        r => Ok(r?),
    }
}

/// Answer a question from `/dev/paddock/ask` with text. The text goes to
/// the agent that asked; nothing runs on the host. Only the host can call
/// this, like `answer`.
pub fn answer_reply(env: &HostEnv, id: &str, text: &str) -> Result<()> {
    answer_reply_origin(env, id, text, false)
}

pub fn answer_reply_cli(env: &HostEnv, id: &str, text: &str) -> Result<()> {
    answer_reply_origin(env, id, text, true)
}

fn answer_reply_origin(env: &HostEnv, id: &str, text: &str, cli: bool) -> Result<()> {
    if !valid_id(id) {
        bail!("invalid request id");
    }
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let p = read_pending_locked(&dir, id)?;
    if p.capability == CONNECT_CAPABILITY {
        return answer_connect_locked(&dir, id, &p, text.trim().parse()?, cli);
    }
    // A card of paddock itself with fixed choices: only those.
    if p.capability != ASK_CAPABILITY && !p.choices.is_empty() {
        if !p.choices.iter().any(|c| c == text.trim()) {
            bail!(
                "{id} takes one of these answers: {}, or deny",
                p.choices.join(", ")
            );
        }
        write_answer(
            &dir,
            id,
            serde_json::json!({ "reply": text.trim() }).to_string(),
        )?;
        return Ok(());
    }
    if p.capability != ASK_CAPABILITY {
        bail!("{id} is a host request, not a question: answer it with once, session or deny");
    }
    if text.trim().is_empty() {
        bail!("the answer is empty");
    }
    if text.chars().count() > MAX_REPLY {
        bail!("the answer is longer than {MAX_REPLY} characters");
    }
    write_answer(&dir, id, serde_json::json!({ "reply": text }).to_string())?;
    Ok(())
}

#[cfg(test)]
type RemoveHook = Box<dyn Fn(&Path) -> io::Result<()>>;
#[cfg(test)]
type AnswerReadHook = Box<dyn Fn() -> io::Result<()>>;

#[cfg(test)]
thread_local! {
    static ANSWER_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static REMOVE_HOOK: std::cell::RefCell<Option<RemoveHook>> = std::cell::RefCell::new(None);
    pub(crate) static OWNER_PUBLISHED_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    static ANSWER_READ_HOOK: std::cell::RefCell<Option<AnswerReadHook>> = std::cell::RefCell::new(None);
    static PUBLISH_CONTENDED_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
}

// Only contention acquiring the store lock proves that this attempt did not
// publish anything. Other failures can leave an uncertain store state.
fn publish_while_waiting(
    env: &HostEnv,
    p: &Pending,
    start: Instant,
    wait: Duration,
    cancelled: &dyn Fn() -> bool,
) -> std::io::Result<bool> {
    let mut first = true;
    loop {
        if cancelled() || (!first && start.elapsed() >= wait) {
            return Ok(false);
        }
        first = false;
        match try_write_pending(env, p) {
            Ok(_) => return Ok(true),
            Err(PublishError::Contended) => {
                #[cfg(test)]
                PUBLISH_CONTENDED_HOOK.with(|hook| {
                    if let Some(hook) = hook.borrow().as_ref() {
                        hook();
                    }
                });
                if cancelled() || start.elapsed() >= wait {
                    return Ok(false);
                }
                std::thread::sleep(
                    Duration::from_millis(20).min(wait.saturating_sub(start.elapsed())),
                );
            }
            Err(PublishError::Failed(e)) => return Err(e),
        }
    }
}

/// Whether the owner's answer was established before it stopped waiting.
pub enum ApprovalOutcome {
    Answered(Verdict),
    NoAnswer,
    /// No card was published, so the user had no opportunity to answer.
    Unavailable,
    Unknown,
}

impl ApprovalOutcome {
    pub(crate) fn into_verdict(self) -> Option<Verdict> {
        match self {
            Self::Answered(v) => Some(v),
            Self::NoAnswer | Self::Unavailable | Self::Unknown => None,
        }
    }
}

/// The answer source. The real one waits for `paddock approvals answer` or a
/// notification action. Tests use a fixed answer.
pub trait Approver: Send + Sync {
    fn ask(&self, env: &HostEnv, p: &Pending, wait: Duration) -> Option<Answer>;

    /// The answer and who gave it ("user" or "agent"), for the audit log.
    fn ask_with_source(
        &self,
        env: &HostEnv,
        p: &Pending,
        wait: Duration,
    ) -> Option<(Answer, &'static str)> {
        self.ask(env, p, wait).map(|a| (a, "user"))
    }

    /// The answer with the user's note on a denial. Callers that tell the
    /// agent about a denial use this one.
    /// A card with fixed choices (`p.choices`): the chosen one, or None
    /// for deny or no answer.
    fn choose(&self, env: &HostEnv, p: &Pending, wait: Duration) -> Option<String> {
        match ask_choice(env, p, wait, &|| false) {
            crate::spawn::Choice::More(c) => Some(c),
            _ => None,
        }
    }

    fn ask_verdict(&self, env: &HostEnv, p: &Pending, wait: Duration) -> Option<Verdict> {
        self.ask_with_source(env, p, wait)
            .map(|(answer, who)| Verdict {
                answer,
                note: None,
                who,
            })
    }

    fn ask_outcome(&self, env: &HostEnv, p: &Pending, wait: Duration) -> ApprovalOutcome {
        match self.ask_verdict(env, p, wait) {
            Some(v) => ApprovalOutcome::Answered(v),
            None => ApprovalOutcome::NoAnswer,
        }
    }
}

/// Answers every request with deny. `paddock doctor --escape` uses it.
pub struct DenyAll;

impl Approver for DenyAll {
    fn ask(&self, _: &HostEnv, _: &Pending, _: Duration) -> Option<Answer> {
        Some(Answer::Deny)
    }
}

pub struct HostApprover;

/// Write a pending request for the panel and the CLI: 0600 in a 0700 folder.
/// Text from a sandbox, made safe to show: no control characters except
/// newline and tab, and no invisible format characters. A terminal would run
/// escape sequences (clear the screen, rewrite a line), and bidi overrides
/// can reverse text on a card or in a notification.
pub fn clean_text(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            if c == '\n' || c == '\t' {
                return true;
            }
            !c.is_control()
                && !matches!(c,
                    '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
        })
        .collect()
}

impl Pending {
    /// The same request with every text that a sandbox can choose cleaned.
    pub fn cleaned(&self) -> Self {
        let mut p = self.clone();
        p.summary = clean_text(&p.summary);
        p.detail = clean_text(&p.detail);
        p.reason = clean_text(&p.reason);
        p.harness = clean_text(&p.harness);
        p.choices = p.choices.iter().map(|c| clean_text(c)).collect();
        p.started_by = p.started_by.iter().map(|c| clean_text(c)).collect();
        p
    }
}

impl Approver for HostApprover {
    fn ask(&self, env: &HostEnv, p: &Pending, wait: Duration) -> Option<Answer> {
        self.ask_verdict(env, p, wait).map(|v| v.answer)
    }

    fn ask_with_source(
        &self,
        env: &HostEnv,
        p: &Pending,
        wait: Duration,
    ) -> Option<(Answer, &'static str)> {
        self.ask_verdict(env, p, wait).map(|v| (v.answer, v.who))
    }

    fn ask_verdict(&self, env: &HostEnv, p: &Pending, wait: Duration) -> Option<Verdict> {
        match self.ask_outcome(env, p, wait) {
            ApprovalOutcome::Answered(v) => Some(v),
            ApprovalOutcome::NoAnswer | ApprovalOutcome::Unavailable | ApprovalOutcome::Unknown => {
                None
            }
        }
    }

    fn ask_outcome(&self, env: &HostEnv, p: &Pending, wait: Duration) -> ApprovalOutcome {
        let p = &p.cleaned();
        let answer_file = approvals_dir(env).join(format!("{}.answer", p.id));
        let start = Instant::now();
        match publish_while_waiting(env, p, start, wait, &|| false) {
            Ok(true) => {}
            Ok(false) => {
                log::warn!("approval {} was not published before the deadline", p.id);
                return ApprovalOutcome::Unavailable;
            }
            Err(e) => {
                log::warn!("approval {} publication failed: {e}", p.id);
                return ApprovalOutcome::Unknown;
            }
        }
        #[cfg(test)]
        OWNER_PUBLISHED_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        let shown = notify_with_actions(env, p);
        while start.elapsed() < wait {
            if answer_file.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // Answered elsewhere or timed out: the buttons mean nothing now.
        if let Some(h) = shown {
            h.close();
        }
        match close_owned(env, &p.id, &|| start.elapsed() < wait) {
            Ok(Some(t)) => match AnswerFile::parse(&t) {
                Some((answer, note)) => ApprovalOutcome::Answered(Verdict {
                    answer,
                    note,
                    who: "user",
                }),
                None => ApprovalOutcome::Unknown,
            },
            Ok(None) => ApprovalOutcome::NoAnswer,
            Err(e) => {
                log::warn!("approval {} outcome is unknown: {e}", p.id);
                ApprovalOutcome::Unknown
            }
        }
    }
}

/// Put a card with fixed choices up (`TIMEOUT_CAPABILITY`) and wait for
/// the answer: one of `p.choices`, or deny. Stop waiting after `wait` or
/// when `cancelled` turns true.
pub fn ask_choice(
    env: &HostEnv,
    p: &Pending,
    wait: Duration,
    cancelled: &dyn Fn() -> bool,
) -> crate::spawn::Choice {
    use crate::spawn::Choice;
    let p = &p.cleaned();
    let answer_file = approvals_dir(env).join(format!("{}.answer", p.id));
    let start = Instant::now();
    match publish_while_waiting(env, p, start, wait, cancelled) {
        Ok(true) => {}
        Ok(false) => {
            log::warn!(
                "approval {} was not published before cancellation or deadline",
                p.id
            );
            return Choice::Unavailable;
        }
        Err(e) => {
            log::warn!("approval {} publication failed: {e}", p.id);
            return Choice::Unknown;
        }
    }
    #[cfg(test)]
    OWNER_PUBLISHED_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook();
        }
    });
    let shown = notify_choices(env, p);
    while start.elapsed() < wait && !cancelled() {
        if answer_file.is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if let Some(h) = shown {
        h.close();
    }
    match close_owned(env, &p.id, &|| start.elapsed() < wait && !cancelled()) {
        Ok(Some(t)) => {
            let reply = serde_json::from_str::<serde_json::Value>(&t)
                .ok()
                .and_then(|v| v["reply"].as_str().map(str::to_string));
            match reply {
                Some(r) if p.choices.contains(&r) => Choice::More(r),
                _ if AnswerFile::parse(&t).is_some_and(|(a, _)| a == Answer::Deny) => Choice::Stop,
                _ => Choice::Unknown,
            }
        }
        Ok(None) => Choice::NoAnswer,
        Err(e) => {
            log::warn!("approval {} outcome is unknown: {e}", p.id);
            Choice::Unknown
        }
    }
}

#[cfg(test)]
#[path = "approval_race_tests.rs"]
mod race_tests;
