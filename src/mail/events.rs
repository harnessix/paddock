//! The events that paddock writes into a run's `up` box. A manager watches
//! them with `paddock mail watch` instead of reading the screen:
//!
//! - `started`: the run's paddock process started (run.rs).
//! - `ended`: the sandbox ended, with its exit code (run.rs, detach-run).
//! - `gone`: the run's paddock process is gone, but it wrote no end: it was
//!   killed. Its pane can still look alive. Whoever reads the mailbox finds
//!   it (`watch`, `ls`), so it does not need the dead process.
//! - `idle`: a detached run shows no output for `[mail] idle_after` and has
//!   unread mail (detach.rs polls its pane).
//! - `card`: an approval card was opened, with what it asked.
//! - `child-started`, `child-ended`: in the parent's box, for a child of
//!   `paddock-host spawn`.
//!
//! `ended` and `gone` are one end event: the first one wins (`append_end`).

use super::{Dir, Message, Starter};
use crate::env::HostEnv;
use std::path::Path;

/// Quiet time with unread mail before the `idle` event. `[mail] idle_after`.
pub const IDLE_AFTER: u64 = 2 * 60;

/// `[mail]` of the user configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cfg {
    pub idle_after: u64,
}

impl Cfg {
    pub fn load(env: &HostEnv) -> Cfg {
        let text = std::fs::read_to_string(env.paddock_config().join("config.toml"));
        match Cfg::parse(&text.unwrap_or_default()) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("{e:#}");
                Cfg {
                    idle_after: IDLE_AFTER,
                }
            }
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Cfg> {
        use anyhow::Context as _;
        let v: toml::Value = crate::manifest::user_config_doc(text)?;
        let idle_after = match v.get("mail").and_then(|m| m.get("idle_after")) {
            None => IDLE_AFTER,
            Some(d) => d
                .as_str()
                .and_then(|s| crate::timer::parse_duration(s).ok())
                .context("[mail] idle_after is a duration such as 2m or 90s")?,
        };
        Ok(Cfg { idle_after })
    }
}

/// The state letter and the start time (clock ticks after boot) of a
/// process, from `/proc/PID/stat`. None where `/proc` does not have it.
fn proc_stat(pid: u32) -> Option<(String, Option<u64>)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let state = fields.next()?.to_string();
    Some((state, fields.nth(18).and_then(|s| s.parse().ok())))
}

/// The start time of a process, in clock ticks after boot. None where
/// `/proc` does not have it.
pub fn proc_start(pid: u32) -> Option<u64> {
    proc_stat(pid)?.1
}

/// The process runs, and it is the same process (not a reused pid). A
/// zombie (killed, not yet reaped by its parent) does not run: signal 0
/// finds it, but it never writes an end, and its parent can wait for
/// whoever waits for that end.
pub fn alive(pid: u32, start: Option<u64>) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as i32, 0) };
    let exists = r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    if !exists {
        return false;
    }
    match proc_stat(pid) {
        None => true,
        Some((state, _)) if matches!(state.as_str(), "Z" | "X") => false,
        Some((_, now)) => start.is_none() || now.is_none() || now == start,
    }
}

/// A child's event for its parent's box.
fn to_parent(
    env: &HostEnv,
    parent: &str,
    child: &str,
    name: &str,
    text: String,
    code: Option<i32>,
) {
    let mut m = Message::event(name, text);
    m.child = Some(child.to_string());
    m.code = code;
    if let Err(e) = super::append(env, parent, Dir::Up, m) {
        log::debug!("no {name} event for {parent}: {e}");
    }
}

/// The mailbox of a running sandbox. `finish` writes its end.
pub struct RunMail {
    env: HostEnv,
    id: String,
    parent: Option<String>,
}

/// What `start` needs to know about a run.
pub struct Start<'a> {
    pub id: &'a str,
    pub harness: &'a str,
    pub project: &'a Path,
    pub detached: bool,
    /// `hooks` when the harness runs paddock's mail hooks (deliver.rs).
    pub delivery: Option<&'a str>,
    /// For a child of `paddock-host spawn`: its parent and spawn id.
    pub parent: Option<(&'a str, &'a str)>,
}

/// At the start of a run: make its mailbox (a detached run has one from
/// `paddock run --detach` already), record this paddock process and write
/// `started`. A problem only costs the mailbox, never the run.
pub fn start(env: &HostEnv, s: &Start) -> Option<RunMail> {
    let starter = match s.parent {
        Some((p, _)) => Starter::sandbox(p),
        None => Starter::host(),
    };
    let pid = std::process::id();
    let made = super::create(env, s.id, &starter).and_then(|()| {
        super::set_run_info(
            env,
            &super::RunInfo {
                id: s.id.to_string(),
                harness: s.harness.to_string(),
                project: s.project.to_path_buf(),
                pid,
                pid_start: proc_start(pid),
                started: crate::state::now(),
                detached: s.detached,
                delivery: s.delivery.map(str::to_string),
                horse_session: None,
            },
        )
    });
    if let Err(e) = made {
        log::warn!("sandbox {}: no mailbox: {e:#}", s.id);
        return None;
    }
    let text = format!(
        "the sandbox started: {} in {}",
        s.harness,
        s.project.display()
    );
    let _ = super::append(env, s.id, Dir::Up, Message::event("started", text));
    if let Some((parent, spawn_id)) = s.parent {
        let text = format!(
            "child sandbox {} started (spawn id {spawn_id}, {})",
            s.id, s.harness
        );
        to_parent(env, parent, s.id, "child-started", text, None);
    }
    Some(RunMail {
        env: env.clone(),
        id: s.id.to_string(),
        parent: s.parent.map(|(p, _)| p.to_string()),
    })
}

impl RunMail {
    /// The run ended. `code` is None when it ended before the sandbox did.
    pub fn finish(self, code: Option<i32>) {
        if ended(&self.env, &self.id, code).is_some() {
            if let Some(p) = &self.parent {
                let text = format!("child sandbox {} ended with {}", self.id, code_text(code));
                to_parent(&self.env, p, &self.id, "child-ended", text, code);
            }
        }
    }
}

fn code_text(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("exit code {c}"),
        None => "no exit code (paddock stopped before the sandbox ended)".into(),
    }
}

/// Write `ended`, unless the run has an end event already.
pub fn ended(env: &HostEnv, id: &str, code: Option<i32>) -> Option<Message> {
    let mut m = Message::event(
        "ended",
        format!("the sandbox ended with {}", code_text(code)),
    );
    m.code = code;
    let written = super::append_end(env, id, m)?;
    crate::run_end_hook::start(env, id, code);
    Some(written)
}

/// Look at a run that has no end event: when its paddock process is gone,
/// write `gone` (and `child-ended` for its parent). Returns the event when
/// this call wrote it. Readers of the mailbox call this as they poll.
pub fn check_gone(env: &HostEnv, id: &str) -> Option<Message> {
    if !matches!(super::inspect_end(env, id), super::EndState::Absent) {
        return None;
    }
    let info = super::run_info(env, id)?;
    if alive(info.pid, info.pid_start) {
        return None;
    }
    let pane = info.detached && crate::view::Tmux::find(env).is_some_and(|t| t.has(id));
    let short = crate::ids::short(id);
    let text = format!(
        "the agent process is gone: paddock (pid {}) of this run ended without recording an end, so it was killed.{} Nothing reads the mailbox or the keyboard now. Start the task again; `paddock view {short}` shows the last screen",
        info.pid,
        if pane { " Its pane in paddock's tmux server still shows the last screen, but nothing runs behind it: do not type into it." } else { "" },
    );
    let m = super::append_end(env, id, Message::event("gone", text))?;
    crate::run_end_hook::start(env, id, None);
    if let Some(p) = super::starter(env, id).and_then(|s| s.parent) {
        let text = format!("child sandbox {id} is gone: its paddock process was killed");
        to_parent(env, &p, id, "child-ended", text, None);
    }
    Some(m)
}

/// Describe an actual card publication. A later reader must list current cards.
pub(crate) fn card_opened(p: &crate::broker::Pending) -> String {
    let mut what = format!("{}: {}", p.capability, p.summary);
    if !p.reason.trim().is_empty() {
        what.push_str(&format!(" (reason: {})", p.reason.trim()));
    }
    let what: String = super::text::clean(&what).chars().take(1000).collect();
    format!(
        "approval card {} opened: {what}. It may have been answered or closed; run `paddock approvals` for the current list",
        p.id
    )
}

pub(crate) fn try_card(
    env: &HostEnv,
    sandbox: &str,
    text: String,
) -> Result<bool, super::SendError> {
    super::try_append(env, sandbox, Dir::Up, Message::event("card", text))
}

/// A detached run's pane showed no output for `quiet` seconds. With unread
/// mail after `[mail] idle_after`, write `idle` once for that mail.
pub fn idle(env: &HostEnv, id: &str, quiet: u64) {
    let Ok(d) = super::dir(env, id) else { return };
    let marker = d.join("idle");
    if quiet < Cfg::load(env).idle_after {
        let _ = std::fs::remove_file(&marker);
        return;
    }
    let unread = super::unread_down(env, id);
    let Some(last) = unread.last().map(|m| m.seq) else {
        return;
    };
    let told: Option<u64> = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|t| t.trim().parse().ok());
    if told == Some(last) || crate::privfs::write(&marker, format!("{last}\n")).is_err() {
        return;
    }
    let seqs: Vec<String> = unread.iter().map(|m| format!("#{}", m.seq)).collect();
    let short = crate::ids::short(id);
    let text = format!(
        "the agent is idle at its prompt for {} minutes with {} unread message(s) ({}). It does not read its mail: attach with `paddock view {short} --write`, or stop it with `paddock stop {short}`",
        quiet / 60,
        unread.len(),
        seqs.join(", ")
    );
    let _ = super::append(env, id, Dir::Up, Message::event("idle", text));
}
