//! agy in horse mode (setup.rs): the command and the task of a headless
//! run, the hooks of a TUI run, and the custody of each.
//!
//! A spawned agy child takes its task as a file (`--print "Read the file
//! ..."`, the `[oneshot]` of the profile). In horse mode `--print` and its
//! text become `--input-format stream-json --output-format stream-json`,
//! and the other one-shot flags (`--dangerously-skip-permissions`) stay,
//! so the custody run asks for the same permissions as the plain run. The
//! task goes into the inbox after the DNA entry: the stream-json custody
//! (live/agy.rs) sends it as the first user line. A session with rows
//! continues its conversation with `--conversation <id>`.
//!
//! A TUI run keeps its command. The sandbox copy of
//! `.gemini/config/hooks.json` gets the entry `paddock-horse`: a
//! `PreInvocation` hook and a `Stop` hook that call `paddock-host horse
//! safe-point KIND --stdin`. agy passes its hook input as JSON on stdin.
//! agy loads this file without a trust step. The host copy never changes,
//! and outside horse mode paddock writes no such entry.

use super::RunHorse;
use crate::build::Built;
use crate::horse::agy;
use crate::horse::live::{self, agy::Start, headless::Captured};
use crate::horse::Store;
use crate::spec::HomeSetting;
use anyhow::{Context as _, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// The entry of paddock in the hooks file.
pub const HOOK_ENTRY: &str = "paddock-horse";

/// The events of the TUI hooks and the safe point that each reports.
pub const HOOKS: [(&str, &str); 2] = [("PreInvocation", "prompt"), ("Stop", "stop")];

/// The agy home of the sandbox, relative to its home.
pub const HOME: &str = ".gemini/antigravity-cli";

/// The setting that puts paddock's hooks into the sandbox copy of the
/// hooks file `into` (the `[hooks] into` of the agy profile).
pub fn hook_setting(into: &str) -> HomeSetting {
    let mut value = json!({});
    for (event, kind) in HOOKS {
        value[event] = json!([{
            "type": "command",
            "command": super::grok_setup::hook_command(kind),
            "timeout": 10,
        }]);
    }
    HomeSetting {
        file: into.into(),
        format: "json".into(),
        path: vec![HOOK_ENTRY.into()],
        value,
        origin: "horse mode: the safe points of the agy TUI".into(),
        append: false,
        codex_trust: None,
        no_pty: false,
        chain: false,
    }
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
        .context("the agy profile has no [hooks] file for paddock's hooks")?;
    built.spec.home_settings.push(hook_setting(&into));
    Ok(idle)
}

/// For a headless live run: the task, and the command rewritten for
/// stream-json. Nothing changes when it fails.
pub fn prepare_live(built: &mut Built) -> Result<String> {
    let one = built
        .profile
        .oneshot
        .as_ref()
        .context("the agy profile has no [oneshot] command")?;
    let file = built
        .spec
        .home
        .join(crate::spawn::prompt_rel(&built.spec.id));
    let oneshot = one.render(&file);
    let at = oneshot
        .iter()
        .position(|a| a == "--print" || a == "-p")
        .filter(|i| i + 1 < oneshot.len())
        .context("the [oneshot] args of the agy profile have no --print with its text")?;
    let mut keep: Vec<String> = oneshot[..at].to_vec();
    keep.extend(oneshot[at + 2..].iter().cloned());
    for owned in [
        "--input-format",
        "--output-format",
        "--conversation",
        "-c",
        "--continue",
    ] {
        if built.spec.command.iter().skip(1).any(|a| a == owned) {
            anyhow::bail!("horse mode holds the agy conversation itself, so the agy command must not set {owned}");
        }
    }
    let path =
        super::grok_setup::task_file(built).context("a headless agy run has no task file")?;
    let task = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read the task of the run {}", path.display()))?;
    if task.trim().is_empty() {
        anyhow::bail!("the task of the run is empty");
    }
    keep.extend(live::agy::args(&Start::default()));
    super::setup::live_command(&mut built.spec.command, &oneshot, &keep)?;
    Ok(task.trim_end().to_string())
}

/// Add `--conversation <id>` after the stream-json flags of the command.
fn conversation_flag(command: &mut Vec<String>, id: &str) -> Result<()> {
    let at = command
        .windows(2)
        .position(|w| w == ["--output-format", "stream-json"])
        .context("the agy command has no stream-json output")?;
    command.splice(
        at + 2..at + 2,
        ["--conversation".to_string(), id.to_string()],
    );
    Ok(())
}

/// Queue the task, pick the conversation, and arm the stream-json custody
/// of the next sandbox.
pub fn arm_live(
    store: &Store,
    rh: &RunHorse,
    built: &mut Built,
    task: &str,
    out: &Captured,
) -> Result<()> {
    anyhow::ensure!(rh.harness == agy::HARNESS, "expected an agy run");
    let home = built.spec.home_src.join(HOME);
    let start = Start::of(store, &rh.session, &home)?;
    if let Some(id) = &start.conversation {
        conversation_flag(&mut built.spec.command, id)?;
    }
    store.queue_task(&rh.session, task)?;
    live::arm_agy(live::agy::CustodyCtx {
        store: store.clone(),
        horse: rh.horse.clone(),
        session: rh.session.clone(),
        start,
        agy_home: home,
        run: built.spec.id.clone(),
        tick: Duration::from_millis(200),
        close_timeout: Duration::from_secs(10),
        out: Box::new(out.clone()),
        interrupt: None,
    });
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
    anyhow::ensure!(rh.harness == agy::HARNESS, "expected an agy run");
    let (close, control) = std::sync::mpsc::channel();
    live::arm_pty_agy(
        live::pty::CustodyCtx {
            store: store.clone(),
            horse: rh.horse.clone(),
            session: rh.session.clone(),
            sessions_root: built.spec.home_src.join(HOME),
            version: agy::VERSION.into(),
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

/// The fields of the `run.end` event of a headless agy run.
pub fn report(r: &live::agy::Report) -> Value {
    json!({
        "session_id": r.native_id,
        "turns": r.turns,
        "interrupted": r.interrupted,
        "deliveries": r.deliveries,
        "landed": r.landed,
        "interrupts": r.interrupts,
        "unknown": r.unknown,
        "bad": r.bad,
        "ingest_errors": r.ingest_errors,
        "exited": r.exited,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_conversation_goes_after_the_stream_json_flags() {
        let mut c: Vec<String> = [
            "agy",
            "--dangerously-skip-permissions",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--model",
            "m",
        ]
        .map(String::from)
        .to_vec();
        super::conversation_flag(&mut c, "id").unwrap();
        assert_eq!(c[6..8], ["--conversation", "id"]);
        assert_eq!(c[8..], ["--model", "m"]);
    }
}
