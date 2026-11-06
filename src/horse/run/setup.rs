//! How a run in horse mode holds its harness, and the command it needs.
//!
//! - dsh: live, always (`dsh --profile sdk`).
//! - Claude Code, headless (a spawned child whose task comes on stdin, as
//!   `claude -p` takes it): live over stream-json. paddock picks the
//!   session id and binds it to the run's session before the sandbox
//!   starts.
//! - Codex, headless (as `codex exec -` takes its task): live over
//!   `codex app-server` on stdio.
//! - Claude Code, interactive (the TUI), on a version with a checked
//!   channel path (`claude::channel_pin`): the channel custody
//!   (live/channel.rs). The TUI keeps its terminal; its hooks give the
//!   safe points, and `paddock-host channel` pushes the entries.
//! - Codex, interactive (the TUI), with a `[horse.pty]` table of its
//!   profile for the pinned version: the pty custody (live/pty.rs). The
//!   TUI keeps its terminal through paddock's relay; Codex's `notify`
//!   program gives the safe points, and paddock types the entries in.
//! - Claude Code, interactive, on a version with no channel path but a
//!   `[horse.pty]` table of its profile: the pty custody
//!   (live/pty/claude.rs). Its `Stop` and `UserPromptSubmit` hooks give
//!   the safe points, and paddock types the entries in.
//! - Every other run, and a TUI without its safe-point path: final-only.
//!   paddock reads the native files when the run ends. A TUI says so in
//!   one line.
//!
//! A headless live run gets its task as the first user entry after the DNA
//! entry. Its custody ends when the main session is at a safe point and
//! its inbox is empty ([`Ends::WhenDone`](live::harness::Ends::WhenDone)). What the custodian
//! says becomes the output of the run.

use super::{RunHorse, Store};
use crate::build::Built;
use crate::env::HostEnv;
use crate::horse::live::{self, claude::Start, harness::Harness, headless::Captured};
use crate::horse::{claude, codex};
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// How paddock holds the harness of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Custody {
    /// paddock reads the native files when the sandbox ended.
    FinalOnly,
    /// paddock holds the stdio of the harness, delivers at safe points and
    /// ingests after every turn.
    Live,
    /// An interactive Claude Code: its hooks give the safe points, the
    /// channel server delivers, and paddock ingests after every turn.
    Channel,
    /// An interactive Codex, or a Claude Code with no channel path: its
    /// `notify` program or its hooks give the safe points, paddock types
    /// the entries into the pty of the relay, and ingests after every turn.
    Pty,
}

impl Custody {
    /// The `custody` field of the `run.end` event.
    pub fn label(self) -> &'static str {
        match self {
            Custody::FinalOnly => "final-only",
            Custody::Live => "live",
            Custody::Channel => "channel",
            Custody::Pty => "pty",
        }
    }
}

/// The run got its task up front and has no terminal: a spawned child
/// whose prompt comes on stdin.
pub fn headless(built: &Built) -> bool {
    built.spawn.is_some() && built.stdin.is_some()
}

/// The custody of a run of `harness` at `version`. An interactive run
/// keeps its terminal: a Claude Code TUI gets the channel when its
/// version has one and the pty relay else, an interactive Codex run the
/// pty relay.
pub fn custody(harness: &str, headless: bool, version: &str) -> Custody {
    match Harness::of(harness) {
        Some(Harness::Dsh) => Custody::Live,
        Some(Harness::Claude | Harness::Codex) if headless => Custody::Live,
        Some(Harness::Claude) if claude::channel_pin(version).is_some() => Custody::Channel,
        Some(Harness::Claude | Harness::Codex) => Custody::Pty,
        None => Custody::FinalOnly,
    }
}

/// The custody of the run of `built` at `version`, and the line for the
/// user when a TUI gets no safe-point delivery.
pub fn choose(built: &Built, version: &str) -> (Custody, Option<String>) {
    use std::io::IsTerminal as _;
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    choose_with(built, version, terminal)
}

/// [`choose`] for a run whose caller has a `terminal` or not.
pub fn choose_with(built: &Built, version: &str, terminal: bool) -> (Custody, Option<String>) {
    let headless = headless(built);
    let harness = built.spec.harness.as_str();
    let tui = matches!(Harness::of(harness), Some(Harness::Claude | Harness::Codex)) && !headless;
    let line = |why: &str| {
        format!("horse: no safe-point delivery for {harness} {version} in the TUI{why}; entries stay queued")
    };
    let fallback = |why: String| (Custody::FinalOnly, Some(line(&format!(" ({why})"))));
    match custody(harness, headless, version) {
        Custody::Channel => match live::channel::conflict(&built.spec.command) {
            Some(why) => fallback(why),
            None => (Custody::Channel, None),
        },
        Custody::Pty => {
            if let Err(e) = live::pty::PtyIdle::of(&built.profile, harness, version) {
                fallback(format!("{e:#}"))
            } else if let Some(why) = match Harness::of(harness) {
                Some(Harness::Claude) => live::channel::conflict_session(&built.spec.command),
                _ => codex::notify::conflict(&built.spec.command),
            } {
                fallback(why)
            } else if !terminal {
                fallback("the run has no terminal for paddock's relay".into())
            } else {
                (Custody::Pty, None)
            }
        }
        Custody::FinalOnly if tui => (Custody::FinalOnly, Some(line(""))),
        c => (c, None),
    }
}

/// Replace the first run of `from` after the executable with `to`. False
/// when `from` is empty or not in `command`; the command then stays.
pub(crate) fn replace_args(command: &mut Vec<String>, from: &[String], to: &[String]) -> bool {
    let Some(at) = (1..command.len())
        .find(|&i| command[i..].starts_with(from))
        .filter(|_| !from.is_empty())
    else {
        return false;
    };
    command.splice(at..at + from.len(), to.iter().cloned());
    true
}

/// Replace the one-shot arguments `oneshot` in `command` with `live`.
/// The arguments before them (the executable, global options) and after
/// them (model and effort options) stay.
pub fn live_command(command: &mut Vec<String>, oneshot: &[String], live: &[String]) -> Result<()> {
    if !replace_args(command, oneshot, live) {
        bail!("horse mode cannot find the one-shot arguments in the command");
    }
    Ok(())
}

/// The flags of a headless `claude` command that paddock sets itself.
const OWNED: [&str; 6] = [
    "--input-format",
    "--output-format",
    "--session-id",
    "--resume",
    "--continue",
    "--include-partial-messages",
];

/// Replace `-p` (or `--print`) in the `claude` command with the custody
/// arguments of `start`. The model flags of the command stay as they are.
pub fn claude_command(command: &mut Vec<String>, start: &Start) -> Result<()> {
    for f in OWNED {
        let eq = format!("{f}=");
        if command.iter().skip(1).any(|a| a == f || a.starts_with(&eq)) {
            bail!("horse mode holds the claude session itself, so the claude command must not set {f}; remove it from the [oneshot] args");
        }
    }
    let print = command
        .iter()
        .skip(1)
        .find(|a| *a == "-p" || *a == "--print")
        .cloned()
        .context("horse mode cannot find -p in the claude command; the [oneshot] args of the claude profile must have it")?;
    live_command(command, &[print], &live::claude::args(start, None))
}

/// The custody of a run that paddock prepares, by harness.
enum Wire {
    Claude(Start),
    Codex(live::codex::Policy),
    /// A Claude Code TUI on this session id: through the channel server,
    /// or with the idle test of its pty through the relay.
    ClaudeTui(String, Option<live::pty::PtyIdle>),
    /// A Codex TUI through the relay.
    Pty(live::pty::PtyIdle),
}

/// A live or channel run before its horse exists: the custody and the
/// task (none for a TUI run).
pub struct Prepared {
    wire: Wire,
    task: Option<String>,
}

/// For a headless live run of Claude Code or Codex: rewrite the command
/// and take the task off the stdin of the sandbox. For a Claude Code TUI
/// run: add the session id, the hooks and (on a version with a channel
/// path) the channel server to the command (live/channel.rs). Nothing goes into the store, so a bad
/// profile or task fails before the store. `None` for every other run.
pub fn prepare(built: &mut Built, custody: Custody, version: &str) -> Result<Option<Prepared>> {
    if custody == Custody::Channel {
        let pin = claude::channel_pin(version)
            .with_context(|| format!("Claude Code {version} has no channel path"))?;
        let native = crate::horse::uuid_v4()?;
        let s = live::channel::prepare(&native, &pin);
        live::channel::apply(&mut built.spec.command, &s)?;
        return Ok(Some(Prepared {
            wire: Wire::ClaudeTui(native, None),
            task: None,
        }));
    }
    if custody == Custody::Pty && built.spec.harness == claude::HARNESS {
        let idle = live::pty::PtyIdle::of(&built.profile, &built.spec.harness, version)?;
        let native = crate::horse::uuid_v4()?;
        let s = live::channel::prepare_hooks(&native);
        live::channel::apply(&mut built.spec.command, &s)?;
        return Ok(Some(Prepared {
            wire: Wire::ClaudeTui(native, Some(idle)),
            task: None,
        }));
    }
    if custody == Custody::Pty {
        let idle = live::pty::PtyIdle::of(&built.profile, &built.spec.harness, version)?;
        // The command line stays: `notify` in the sandbox copy of
        // config.toml is the only change.
        built.spec.home_settings.push(codex::notify::setting());
        return Ok(Some(Prepared {
            wire: Wire::Pty(idle),
            task: None,
        }));
    }
    if custody != Custody::Live {
        return Ok(None);
    }
    let wire = match Harness::of(&built.spec.harness) {
        Some(Harness::Claude) => {
            let start = Start::fresh()?;
            claude_command(&mut built.spec.command, &start)?;
            Wire::Claude(start)
        }
        Some(Harness::Codex) => {
            let one = built
                .profile
                .oneshot
                .as_ref()
                .context("the codex profile has no [oneshot] command")?;
            let prompt_file = built
                .spec
                .home
                .join(crate::spawn::prompt_rel(&built.spec.id));
            let oneshot = one.render(&prompt_file);
            let policy = live::codex::policy(&built.profile)?;
            let args = live::codex::args(&built.profile)?;
            live_command(&mut built.spec.command, &oneshot, &args)?;
            Wire::Codex(policy)
        }
        Some(Harness::Dsh) | None => return Ok(None),
    };
    let bytes = built.stdin.take().context("a headless run has no task")?;
    let task = String::from_utf8(bytes).context("the task of the run is not UTF-8 text")?;
    if task.trim().is_empty() {
        bail!("the task of the run is empty");
    }
    Ok(Some(Prepared {
        wire,
        task: Some(task.trim_end().to_string()),
    }))
}

/// One line in the device ledger of the run of `built`, as the broker
/// writes it for the channel server.
fn ledger(env: &HostEnv, built: &Built) -> Box<dyn Fn(&str) + Send> {
    use crate::devices::ledger;
    let path = ledger::path(env);
    let sandbox = built.spec.id.clone();
    let chain = built
        .spawn
        .as_ref()
        .map(|c| c.chain.clone())
        .unwrap_or_default();
    let project = built.spec.project.to_string_lossy().into_owned();
    Box::new(move |line: &str| {
        let payer = ledger::Payer {
            sandbox: &sandbox,
            root: ledger::root_of(&sandbox, &chain),
            project: &project,
            device: "horse",
        };
        if let Err(e) = ledger::Ledger::open(&path).and_then(|l| l.record(&payer, line)) {
            log::warn!("horse ledger: {e:#}");
        }
    })
}

/// Queue the task (after the DNA entry of `run::begin`), bind the session
/// id of a Claude Code run, and arm the custody of the sandbox that this
/// process starts next. The returned buffer gets what the custodian says:
/// the output of a headless run. A TUI run has none: its terminal shows
/// what claude says.
pub fn arm(
    env: &HostEnv,
    store: &Store,
    rh: &RunHorse,
    built: &Built,
    p: Prepared,
) -> Result<Option<Captured>> {
    let out = Captured::default();
    // Claude Code names its transcript folder after the real path of its
    // cwd. Seatbelt runs it on the host's paths, where /tmp and /var are
    // links on macOS. Under bwrap the workdir is a mount point: as is.
    let cwd = if cfg!(target_os = "macos") {
        std::fs::canonicalize(&built.spec.workdir).unwrap_or_else(|_| built.spec.workdir.clone())
    } else {
        built.spec.workdir.clone()
    }
    .display()
    .to_string();
    let tick = Duration::from_millis(200);
    let task = p.task.unwrap_or_default();
    match p.wire {
        Wire::ClaudeTui(native, pty) => {
            anyhow::ensure!(rh.harness == claude::HARNESS, "expected a claude run");
            store.lock()?.bind_native(&rh.session, &native)?;
            let (close, control) = std::sync::mpsc::channel();
            let tui = live::channel::CustodyCtx {
                store: store.clone(),
                horse: rh.horse.clone(),
                session: rh.session.clone(),
                native,
                config_dir: built.spec.home_src.join(".claude"),
                cwd,
                version: claude::VERSION.into(),
                run: built.spec.id.clone(),
                runs: live::tui_state::runs(),
                ledger: ledger(env, built),
                control,
                timing: live::channel::Timing::default(),
            };
            match pty {
                Some(idle) => live::arm_pty_claude(
                    live::pty::claude::CustodyCtx {
                        tui,
                        idle,
                        tap: live::pty::PtyTap::new(),
                    },
                    close,
                ),
                None => live::arm_channel(tui, close),
            }
            return Ok(None);
        }
        Wire::Pty(idle) => {
            anyhow::ensure!(rh.harness == codex::HARNESS, "expected a codex run");
            let (close, control) = std::sync::mpsc::channel();
            live::arm_pty(
                live::pty::CustodyCtx {
                    store: store.clone(),
                    horse: rh.horse.clone(),
                    session: rh.session.clone(),
                    sessions_root: built.spec.home_src.join(".codex/sessions"),
                    version: codex::VERSION.into(),
                    run: built.spec.id.clone(),
                    runs: live::tui_state::runs(),
                    ledger: ledger(env, built),
                    control,
                    timing: live::pty::Timing::default(),
                    idle,
                    tap: live::pty::PtyTap::new(),
                },
                close,
            );
            return Ok(None);
        }
        Wire::Claude(start) => {
            anyhow::ensure!(rh.harness == claude::HARNESS, "expected a claude run");
            store.lock()?.bind_native(&rh.session, &start.native)?;
            store.queue_append(&rh.session, "user", &task)?;
            live::arm_claude(live::claude::CustodyCtx {
                store: store.clone(),
                horse: rh.horse.clone(),
                start: Start {
                    session: Some(rh.session.clone()),
                    ..start
                },
                config_dir: built.spec.home_src.join(".claude"),
                cwd,
                version: claude::VERSION.into(),
                run: built.spec.id.clone(),
                tick,
                out: Box::new(out.clone()),
                wait: None,
            });
        }
        Wire::Codex(policy) => {
            anyhow::ensure!(rh.harness == codex::HARNESS, "expected a codex run");
            store.queue_append(&rh.session, "user", &task)?;
            live::arm_codex(
                live::codex::CustodyCtx {
                    store: store.clone(),
                    horse: rh.horse.clone(),
                    session: rh.session.clone(),
                    cwd,
                    sessions_root: built.spec.home_src.join(".codex/sessions"),
                    run: built.spec.id.clone(),
                    model: None,
                    effort: None,
                    tick,
                    reply_timeout: Duration::from_secs(120),
                    close_timeout: Duration::from_secs(10),
                    out: Box::new(out.clone()),
                },
                policy,
            );
        }
    }
    Ok(Some(out))
}

/// Wait for the custodian of a live run. The error of the custody, and
/// the report for the `run.end` event: what the custody did, or why it
/// failed. "the custody did not start" when the sandbox did not start.
pub fn finish() -> (Result<()>, Value) {
    let _ = live::take_hooks();
    let _ = live::take_pty_tap();
    match live::finish() {
        None => (Ok(()), json!({"error": "the custody did not start"})),
        Some(Ok(done)) => (Ok(()), report(&done)),
        Some(Err(e)) => {
            let r = json!({"error": format!("{e:#}")});
            (Err(e.context("horse custody")), r)
        }
    }
}

/// The fields of the `run.end` event that tell what a custody did.
pub fn report(done: &live::Done) -> Value {
    match done {
        live::Done::Dsh(r) => json!({
            "events": r.events,
            "deliveries": r.delivered,
            "reconciled": r.reconciled,
        }),
        live::Done::Claude(r) => json!({
            "turns": r.turns,
            "deliveries": r.deliveries,
            "landed": r.landed,
            "interrupts": r.interrupts,
            "unknown": r.unknown,
            "bad": r.bad,
        }),
        live::Done::Codex(r) => json!({
            "turns": r.turns,
            "deliveries": r.deliveries,
            "landed": r.landed,
            "interrupts": r.interrupts,
            "answers": r.answers,
            "unknown": r.unknown,
            "bad": r.malformed,
            "ingest_errors": r.ingest_errors,
            "exited": r.exited,
        }),
        live::Done::Channel(r) => json!({
            "stops": r.stops,
            "prompts": r.prompts,
            "missed_hooks": r.missed_hooks,
            "deliveries": r.released,
            "landed": r.landed,
            "retried": r.retried,
            "not_landed": r.not_landed,
            "ingests": r.ingests,
            "ingest_errors": r.ingest_errors,
        }),
        live::Done::PtyClaude(r) => json!({
            "stops": r.tui.stops,
            "prompts": r.tui.prompts,
            "missed_hooks": r.tui.missed_hooks,
            "deliveries": r.tui.released,
            "landed": r.tui.landed,
            "retried": r.tui.retried,
            "interrupts": r.interrupts,
            "not_landed": r.tui.not_landed,
            "ingests": r.tui.ingests,
            "ingest_errors": r.tui.ingest_errors,
            "relay_lost": r.broken,
        }),
        live::Done::Pty(r) => json!({
            "thread": r.thread,
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
        }),
    }
}
