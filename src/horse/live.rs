//! Horse mode for dsh runs (experimental, off by default).
//!
//! `[horse] enabled = true` in the user configuration, or `paddock run dsh
//! --horse`, starts dsh as `dsh --profile sdk` instead of `dsh web`, and
//! paddock holds its stdio: the SDK channel (rpc.rs) is the stable,
//! documented channel of dsh 0.2.1-rc.5. The web API (`session.follow`)
//! is an internal Typert protocol over a WebSocket mux with a launch
//! token, and dsh gives it no stability promise. The custodian
//! (custody.rs) runs in a thread of the paddock process, outside the
//! sandbox, so the store is never in the sandbox.
//!
//! Runs without horse mode do not change: nothing here runs for them.

pub mod channel;
pub mod claude;
pub mod codex;
pub mod custody;
pub mod harness;
pub mod headless;
pub mod pty;
pub mod rpc;
pub mod tui_state;

use super::Store;
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::process::{ChildStdin, ChildStdout};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

pub use custody::{Report, Setup};
pub use harness::{Harness, LiveSession, Record};

/// `[horse]` of the user configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    /// The provider route for the SDK handshake.
    pub dsh_provider: String,
    pub dsh_model: String,
    pub dsh_effort: Option<String>,
}

/// The route of the SDK handshake when the user sets none: the route that
/// the SDK server mounts by itself (`packages/sdk/server/README.md`), and
/// the default model of dsh's catalog.
pub const PROVIDER: &str = "deepseek-official";
pub const MODEL: &str = "deepseek-flash";

/// Read `[horse]` from the text of the user configuration.
pub fn settings_in(text: &str) -> Result<Settings> {
    let v: toml::Table = toml::from_str(text).context("the user configuration is not TOML")?;
    let h = v.get("horse").and_then(toml::Value::as_table);
    let s = |k: &str| -> Result<Option<String>> {
        match h.and_then(|t| t.get(k)) {
            None => Ok(None),
            Some(toml::Value::String(x)) if !x.is_empty() => Ok(Some(x.clone())),
            Some(_) => bail!("[horse] {k} must be a text"),
        }
    };
    if let Some(t) = h {
        for k in t.keys() {
            if !["enabled", "dsh_provider", "dsh_model", "dsh_effort"].contains(&k.as_str()) {
                bail!("[horse] {k} is not known");
            }
        }
    }
    let enabled = match h.and_then(|t| t.get("enabled")) {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => bail!("[horse] enabled must be true or false"),
    };
    Ok(Settings {
        enabled,
        dsh_provider: s("dsh_provider")?.unwrap_or_else(|| PROVIDER.into()),
        dsh_model: s("dsh_model")?.unwrap_or_else(|| MODEL.into()),
        dsh_effort: s("dsh_effort")?,
    })
}

pub fn settings(env: &HostEnv) -> Result<Settings> {
    settings_in(
        &std::fs::read_to_string(env.paddock_config().join("config.toml")).unwrap_or_default(),
    )
}

/// Replace the launch arguments of the dsh profile (`web`) in the command
/// with `--profile sdk`.
pub fn sdk_command(command: &mut Vec<String>, launch_args: &[String]) -> Result<()> {
    let sdk = ["--profile".to_string(), "sdk".to_string()];
    if !super::run::setup::replace_args(command, launch_args, &sdk) {
        bail!("horse mode cannot find the dsh launch arguments in the command");
    }
    Ok(())
}

type Hook = Box<dyn FnOnce(ChildStdin, ChildStdout) + Send>;

static DEVICE_SETUP: Mutex<Option<Setup>> = Mutex::new(None);

static HOOK: Mutex<Option<Hook>> = Mutex::new(None);
static JOIN: Mutex<Option<JoinHandle<Result<Done>>>> = Mutex::new(None);
/// The control channel of a custody that holds no stdio (a TUI run).
static CLOSE: Mutex<Option<std::sync::mpsc::Sender<harness::Control>>> = Mutex::new(None);

/// What the custody of a run did, by harness.
#[derive(Debug, PartialEq, Eq)]
pub enum Done {
    Dsh(Report),
    Claude(claude::Report),
    Codex(codex::Report),
    /// An interactive Claude Code run through the channel server.
    Channel(channel::Report),
    /// An interactive Codex run through the pty relay.
    Pty(pty::Report),
    /// An interactive Claude Code run through the pty relay.
    PtyClaude(pty::claude::Report),
}

/// Arm `hook` for the next sandbox that this process starts: the sandbox
/// gets pipes as stdin and stdout, and a thread of this process holds
/// them with `custody`. [`finish`] waits for that thread.
fn arm_with<F>(custody: F)
where
    F: FnOnce(ChildStdin, ChildStdout) -> Result<Done> + Send + 'static,
{
    let hook: Hook = Box::new(move |stdin, stdout| {
        let j = std::thread::spawn(move || custody(stdin, stdout));
        *JOIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(j);
    });
    *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
}

/// The custody of the next sandbox that this process starts, for dsh:
/// the user types into the run.
pub fn arm(set: Setup) {
    *DEVICE_SETUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(set.clone());
    arm_with(move |stdin, stdout| {
        let (tx, rx) = std::sync::mpsc::channel();
        rpc::read_runtime(stdout, tx);
        let (ctl, control) = std::sync::mpsc::channel();
        harness::read_user(std::io::stdin(), ctl);
        custody::run(set, stdin, rx, &control, std::io::stdout()).map(Done::Dsh)
    });
}

/// The custody of the next sandbox that this process starts, for a
/// headless Claude Code run (`claude -p` over stream-json). No user types
/// into the run: the custody delivers what waits in the inbox (the task
/// first), and ends when the session is idle and the inbox is empty.
pub fn arm_claude(ctx: claude::CustodyCtx) {
    arm_with(move |stdin, stdout| {
        claude::run(headless::io(stdout, stdin), ctx, harness::Ends::WhenDone).map(Done::Claude)
    });
}

/// The custody of the next sandbox that this process starts, for a
/// headless Codex run (`codex app-server` on stdio), as for
/// [`arm_claude`].
pub fn arm_codex(ctx: codex::CustodyCtx, policy: codex::Policy) {
    arm_with(move |stdin, stdout| {
        codex::run(
            headless::io(stdout, stdin),
            ctx,
            policy,
            harness::Ends::WhenDone,
        )
        .map(Done::Codex)
    });
}

/// The custody of an interactive Claude Code run: the TUI keeps its
/// terminal, and a thread of this process follows its safe points from
/// now until [`finish`]. `close` ends it: the run sends two `Close`s.
pub fn arm_channel(ctx: channel::CustodyCtx, close: std::sync::mpsc::Sender<harness::Control>) {
    // Subscribe before the sandbox starts, so no hook goes unheard.
    let signals = ctx
        .runs
        .subscribe(&tui_state::run_key(&ctx.horse, &ctx.session));
    let j = std::thread::spawn(move || channel::run(ctx, signals).map(Done::Channel));
    *JOIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(j);
    *CLOSE.lock().unwrap_or_else(|e| e.into_inner()) = Some(close);
}

/// The relay tap of an interactive run whose custody types into its pty.
static PTY_TAP: Mutex<Option<std::sync::Arc<pty::PtyTap>>> = Mutex::new(None);

/// The custody of an interactive Codex run: the TUI keeps its terminal
/// through paddock's relay, and a thread of this process types the
/// entries into it at the safe points from now until [`finish`]. The
/// relay of the next sandbox gets `ctx.tap` ([`take_pty_tap`]).
pub fn arm_pty(ctx: pty::CustodyCtx, close: std::sync::mpsc::Sender<harness::Control>) {
    let key = tui_state::run_key(&ctx.horse, &ctx.session);
    let (runs, tap) = (ctx.runs, ctx.tap.clone());
    arm_tui(runs, &key, tap, close, move |signals| {
        pty::run(ctx, signals).map(Done::Pty)
    });
}

/// [`arm_pty`] for an interactive Claude Code run with no channel path.
pub fn arm_pty_claude(
    ctx: pty::claude::CustodyCtx,
    close: std::sync::mpsc::Sender<harness::Control>,
) {
    let key = tui_state::run_key(&ctx.tui.horse, &ctx.tui.session);
    let (runs, tap) = (ctx.tui.runs, ctx.tap.clone());
    arm_tui(runs, &key, tap, close, move |signals| {
        pty::claude::run(ctx, signals).map(Done::PtyClaude)
    });
}

/// Start the custody `f` of a TUI run whose entries paddock types into
/// its pty. It subscribes before the sandbox starts, so no safe point
/// goes unheard, and no channel server of the sandbox gets an entry of
/// this run.
fn arm_tui<F>(
    runs: &'static tui_state::Runs,
    key: &str,
    tap: std::sync::Arc<pty::PtyTap>,
    close: std::sync::mpsc::Sender<harness::Control>,
    f: F,
) where
    F: FnOnce(tui_state::SignalRx) -> Result<Done> + Send + 'static,
{
    runs.typed(key);
    let signals = runs.subscribe(key);
    *PTY_TAP.lock().unwrap_or_else(|e| e.into_inner()) = Some(tap);
    let j = std::thread::spawn(move || f(signals));
    *JOIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(j);
    *CLOSE.lock().unwrap_or_else(|e| e.into_inner()) = Some(close);
}

/// run.rs: the relay tap of a pty custody, for the next sandbox. Its
/// presence makes the run relay its terminal.
pub fn take_pty_tap() -> Option<std::sync::Arc<pty::PtyTap>> {
    PTY_TAP.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// run.rs: the custody hook, when this run is in horse mode. The sandbox
/// then gets pipes as stdin and stdout, and no terminal.
pub fn take_hook() -> Option<Hook> {
    HOOK.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Take the horse identity for the run that consumes the custody hook.
pub fn take_device_setup() -> Option<Setup> {
    DEVICE_SETUP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Transfer the pipes only when custody was armed for this run.
pub(crate) fn handoff(hook: Option<Hook>, child: &mut std::process::Child) {
    if let Some(hook) = hook {
        if let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) {
            hook(stdin, stdout);
        }
    }
}

/// Wait for the custodian after the sandbox ended.
pub fn finish() -> Option<Result<Done>> {
    // A custody without stdio ends when the run says so: the sandbox is
    // gone, so the second Close ends it at once.
    if let Some(tx) = CLOSE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = tx.send(harness::Control::Close);
        let _ = tx.send(harness::Control::Close);
    }
    let j = JOIN.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    Some(
        j.join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the horse custodian stopped"))),
    )
}

/// Prepare a dsh run for horse mode: the command, the horse, the main
/// session, and the custody hook. `horse_id` continues a horse: its newest
/// dsh session is resumed. The returned line tells the user.
pub fn prepare(
    env: &HostEnv,
    built: &mut crate::build::Built,
    horse_id: Option<&str>,
) -> Result<String> {
    match Harness::of(&built.spec.harness) {
        Some(Harness::Dsh) => {}
        // The custodies exist (claude.rs, codex.rs); `paddock run` does not
        // start them yet.
        Some(Harness::Claude) => bail!("horse mode does not work with claude runs yet"),
        Some(Harness::Codex) => bail!("horse mode does not work with codex runs yet"),
        None => bail!(
            "horse mode works with dsh only, not with {}",
            built.spec.harness
        ),
    }
    if built.detach {
        bail!("horse mode does not work with --detach yet");
    }
    let cfg = settings(env)?;
    sdk_command(&mut built.spec.command, &built.profile.launch.args)?;
    let store = Store::open(&super::root(env))?;
    let sessions_root = built.spec.home_src.join(".dsh/sessions");
    let (horse, main, resume) = match horse_id {
        None => {
            let h = store.new_default_horse()?;
            let native = format!("session-{}", super::uuid_v4()?);
            (h.id, native, false)
        }
        Some(id) => {
            let h = store.horse(id)?;
            let newest = store
                .sessions()?
                .into_iter()
                .filter(|s| s.horse == h.id && s.binding.harness == super::dsh::HARNESS)
                .filter(|s| s.head.is_some())
                .max_by_key(|s| s.created_ms);
            match newest {
                None => (h.id, format!("session-{}", super::uuid_v4()?), false),
                Some(s) => {
                    let native = match &s.binding.native_id {
                        Some(n) if custody::find_session_dir(&sessions_root, n).is_some() => {
                            n.clone()
                        }
                        _ => {
                            super::dsh::project(&store, &s.id, &sessions_root, super::now_ms())?
                                .native_id
                        }
                    };
                    (h.id, native, true)
                }
            }
        }
    };
    let line = format!(
        "horse {horse}, dsh session {main}{} (experimental)",
        if resume { ", resumed" } else { "" }
    );
    arm(Setup {
        store,
        horse,
        main,
        resume,
        sessions_root: Some(sessions_root),
        cwd: built.spec.workdir.display().to_string(),
        provider: cfg.dsh_provider,
        model: cfg.dsh_model,
        effort: cfg.dsh_effort,
        run: built.spec.id.clone(),
        tick: Duration::from_millis(200),
    });
    Ok(line)
}

/// The live records of runs whose paddock process still runs. A record of
/// a process that is gone is removed.
pub fn records(store: &Store) -> Result<Vec<Record>> {
    if !store.root().join("live").is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for n in store.list_refs("live")? {
        let Some(b) = store.read_ref(&format!("live/{n}"))? else {
            continue;
        };
        let Ok(r) = serde_json::from_slice::<Record>(&b) else {
            continue;
        };
        // SAFETY: kill with signal 0 only checks that the process exists.
        let alive = unsafe { libc::kill(r.pid as libc::pid_t, 0) } == 0;
        if alive {
            out.push(r);
        } else {
            store.remove_ref(&format!("live/{n}"))?;
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;

pub(crate) fn take_hooks() -> (Option<Hook>, Option<Setup>) {
    (take_hook(), take_device_setup())
}
