//! Grok Build in horse mode (setup.rs): the command and the task of a
//! headless run, the hooks of a TUI run, and the custody of each.
//!
//! A spawned Grok child takes its task as a file (`--prompt-file`, the
//! `[oneshot]` of the profile). In horse mode the one-shot arguments
//! become `agent [--always-approve] --no-leader stdio`, and the task goes
//! into the inbox after the DNA entry: the ACP custody (live/grok.rs)
//! sends it as the first `session/prompt`.
//!
//! A TUI run keeps its command. The sandbox copy of paddock's hooks file
//! gets five entries that call `paddock-host horse safe-point KIND
//! --stdin`: `UserPromptSubmit` (a turn starts), `Stop`, `StopCancelled`
//! and `StopFailure` (a turn ended, however), and the `idle_prompt`
//! `Notification` (the backstop of the Grok user guide for a turn that
//! reports none of the three). Grok passes the event as JSON on stdin,
//! with the session id and the turn. The host copy never changes, and
//! outside horse mode paddock writes no such entry.

use super::RunHorse;
use crate::build::Built;
use crate::horse::grok;
use crate::horse::live::{self, headless::Captured};
use crate::horse::Store;
use crate::spec::HomeSetting;
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// The hook command of a safe point.
pub fn hook_command(kind: &str) -> String {
    format!("paddock-host horse safe-point {kind} --stdin || true")
}

/// The events of the TUI hooks, the safe point that each reports, and the
/// matcher of the event.
pub const HOOKS: [(&str, &str, Option<&str>); 5] = [
    ("UserPromptSubmit", "prompt", None),
    ("Stop", "stop", None),
    ("StopCancelled", "stop", None),
    ("StopFailure", "stop", None),
    ("Notification", "stop", Some("idle_prompt")),
];

/// The task file of a headless Grok or agy child, as paddock sees it: a
/// spawned run whose command has the one-shot arguments with the prompt
/// file.
pub fn task_file(built: &Built) -> Option<PathBuf> {
    let file_task =
        [grok::HARNESS, crate::horse::agy::HARNESS].contains(&built.spec.harness.as_str());
    if !file_task || built.spawn.is_none() || built.stdin.is_some() {
        return None;
    }
    let one = built.profile.oneshot.as_ref()?;
    if one.prompt != crate::profile::PromptVia::File {
        return None;
    }
    let rel = crate::spawn::prompt_rel(&built.spec.id);
    let args = one.render(&built.spec.home.join(&rel));
    let c = &built.spec.command;
    (1..c.len())
        .any(|i| c[i..].starts_with(&args))
        .then(|| built.spec.home_src.join(rel))
}

/// For a headless live run: the permission policy of the plain run and
/// the task, and the command rewritten for the ACP agent. Nothing changes
/// when it fails.
pub fn prepare_live(built: &mut Built) -> Result<(live::grok::Policy, String)> {
    let one = built
        .profile
        .oneshot
        .as_ref()
        .context("the grok profile has no [oneshot] command")?;
    let file = built
        .spec
        .home
        .join(crate::spawn::prompt_rel(&built.spec.id));
    let oneshot = one.render(&file);
    let policy = live::grok::policy(&built.profile)?;
    let c = &built.spec.command;
    let at = (1..c.len())
        .find(|&i| c[i..].starts_with(&oneshot))
        .context("horse mode cannot find the one-shot arguments in the grok command")?;
    if at + oneshot.len() != c.len() {
        bail!("horse mode cannot hold a grok command with arguments after its one-shot arguments: they would follow `agent stdio`");
    }
    let path = task_file(built).context("a headless grok run has no task file")?;
    let task = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read the task of the run {}", path.display()))?;
    if task.trim().is_empty() {
        bail!("the task of the run is empty");
    }
    super::setup::live_command(
        &mut built.spec.command,
        &oneshot,
        &live::grok::args(&policy),
    )?;
    Ok((policy, task.trim_end().to_string()))
}

/// The settings that put paddock's hooks into the sandbox copy of the
/// hooks file `into` (the `[hooks] into` of the grok profile).
pub fn hook_settings(into: &str) -> Vec<HomeSetting> {
    HOOKS
        .iter()
        .map(|(event, kind, matcher)| {
            let mut group = json!({ "hooks": [{
                "type": "command",
                "command": hook_command(kind),
                "timeout": 10,
            }] });
            if let Some(m) = matcher {
                group["matcher"] = json!(m);
            }
            HomeSetting {
                file: into.into(),
                format: "json".into(),
                path: vec!["hooks".into(), (*event).into()],
                value: json!([group]),
                origin: "horse mode: the safe points of the Grok Build TUI".into(),
                append: false,
                codex_trust: None,
                no_pty: false,
                chain: false,
            }
        })
        .collect()
}

/// For a TUI run: the idle test of the profile, and paddock's hooks in
/// the sandbox copy of the hooks file.
pub fn prepare_tui(built: &mut Built, version: &str) -> Result<live::pty::PtyIdle> {
    let idle = live::pty::PtyIdle::of(&built.profile, &built.spec.harness, version)?;
    let into = built
        .profile
        .hooks
        .as_ref()
        .map(|h| h.into.clone())
        .context("the grok profile has no [hooks] file for paddock's hooks")?;
    built.spec.home_settings.extend(hook_settings(&into));
    Ok(idle)
}

/// Queue the task and arm the ACP custody of the next sandbox.
pub fn arm_live(
    store: &Store,
    rh: &RunHorse,
    built: &Built,
    policy: live::grok::Policy,
    task: &str,
    out: &Captured,
) -> Result<()> {
    anyhow::ensure!(rh.harness == grok::HARNESS, "expected a grok run");
    store.queue_task(&rh.session, task)?;
    live::arm_grok(
        live::grok::CustodyCtx {
            store: store.clone(),
            horse: rh.horse.clone(),
            session: rh.session.clone(),
            cwd: built.spec.workdir.display().to_string(),
            grok_home: built.spec.home_src.join(".grok"),
            run: built.spec.id.clone(),
            tick: Duration::from_millis(200),
            reply_timeout: Duration::from_secs(120),
            close_timeout: Duration::from_secs(10),
            out: Box::new(out.clone()),
        },
        policy,
    );
    Ok(())
}

/// Arm the pty custody of a TUI run.
pub fn arm_tui(
    env: &crate::env::HostEnv,
    store: &Store,
    rh: &RunHorse,
    built: &Built,
    idle: live::pty::PtyIdle,
    ledger: Box<dyn Fn(&str) + Send>,
) -> Result<()> {
    anyhow::ensure!(rh.harness == grok::HARNESS, "expected a grok run");
    let (close, control) = std::sync::mpsc::channel();
    live::arm_pty_grok(
        live::pty::CustodyCtx {
            store: store.clone(),
            horse: rh.horse.clone(),
            session: rh.session.clone(),
            sessions_root: built.spec.home_src.join(".grok"),
            version: grok::VERSION.into(),
            run: built.spec.id.clone(),
            runs: live::tui_state::runs(),
            ledger,
            control,
            timing: live::pty::Timing::default(),
            idle,
            tap: live::pty::PtyTap::with_mail(env, &built.spec.id),
        },
        close,
    );
    Ok(())
}

/// The fields of the `run.end` event of a Grok TUI run.
pub fn tui_report(r: &live::pty::Report) -> Value {
    json!({
        "session_id": r.thread,
        "stops": r.stops,
        "other_stops": r.other_stops,
        "missed_stops": r.missed_stops,
        "deliveries": r.typed,
        "landed": r.landed,
        "retried": r.retried,
        "interrupts": r.interrupts,
        "not_landed": r.not_landed,
        "ingests": r.ingests,
        "ingest_errors": r.ingest_errors,
    })
}
