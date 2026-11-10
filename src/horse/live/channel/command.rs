//! The claude command of a TUI run in horse mode: the session id, the
//! `Stop` and `UserPromptSubmit` hooks, and (for the channel custody) the
//! channel server.

use crate::horse::claude::ChannelPin;
use anyhow::{bail, Result};

/// The name of the MCP server of the channel.
pub const SERVER: &str = "paddock";
/// The hook commands. A hook that cannot reach the host must not fail
/// the turn (exit code 2 of a `Stop` hook would keep the model going).
pub const STOP_HOOK: &str = "paddock-host horse safe-point stop || true";
pub const PROMPT_HOOK: &str = "paddock-host horse safe-point prompt || true";

/// The flags of the claude command that paddock sets itself in a TUI run:
/// the session flags, then (from [`CHANNEL_OWNED`] on) the channel flags.
const OWNED: [&str; 11] = [
    "--session-id",
    "--resume",
    "-r",
    "--continue",
    "-c",
    "--fork-session",
    "-p",
    "--print",
    "--channels",
    "--dangerously-load-development-channels",
    "--mcp-config",
];
/// Where the channel flags start in [`OWNED`]. A run that gets its entries
/// through the pty (pty/claude.rs) owns only the session flags.
const CHANNEL_OWNED: usize = 8;

/// What a TUI run in horse mode adds to the claude command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The session id that paddock picked.
    pub native: String,
    /// The arguments after the executable.
    pub args: Vec<String>,
    /// The hooks, `(event, command)`, beside the hooks of the command.
    pub hooks: Vec<(&'static str, String)>,
}

/// The arguments and hooks of a TUI run on session `native`, for the
/// Claude Code version of `pin`. The `=` forms keep a variadic option
/// (`--channels <servers...>`) from taking the arguments after it.
pub fn prepare(native: &str, pin: &ChannelPin) -> Setup {
    let mut s = prepare_hooks(native);
    let mcp = serde_json::json!({"mcpServers": {SERVER: {
        "type": "stdio", "command": "paddock-host", "args": ["channel"],
    }}});
    s.args.push(format!("--mcp-config={mcp}"));
    s.args.push(format!("--channels=server:{SERVER}"));
    s.args
        .extend(pin.flags.iter().map(|f| f.replace("{}", SERVER)));
    s
}

/// The session id and the two hooks, with no channel: a TUI run whose
/// entries paddock types into the pty (pty/claude.rs).
pub fn prepare_hooks(native: &str) -> Setup {
    Setup {
        native: native.into(),
        args: vec![format!("--session-id={native}")],
        hooks: vec![
            ("Stop", STOP_HOOK.to_string()),
            ("UserPromptSubmit", PROMPT_HOOK.to_string()),
        ],
    }
}

/// Why the TUI command cannot get the channel: it sets a flag that
/// paddock owns in a TUI run. `None` when it can.
pub fn conflict(command: &[String]) -> Option<String> {
    conflict_in(command, &OWNED)
}

/// [`conflict`] for a run with no channel: only the session flags count.
pub fn conflict_session(command: &[String]) -> Option<String> {
    conflict_in(command, &OWNED[..CHANNEL_OWNED])
}

fn conflict_in(command: &[String], owned: &[&str]) -> Option<String> {
    let arg = command.iter().skip(1).find(|a| {
        owned
            .iter()
            .any(|f| *a == f || (f.starts_with("--") && a.starts_with(&format!("{f}="))))
    })?;
    let flag = arg.split('=').next().unwrap_or(arg);
    Some(format!(
        "paddock picks the session and the channel of the run itself, and the claude command sets {flag}"
    ))
}

/// Put `setup` into the claude command: the hooks into its `--settings`,
/// the arguments after the executable.
pub fn apply(command: &mut Vec<String>, setup: &Setup) -> Result<()> {
    let channel = setup.args.iter().any(|a| a.starts_with("--channels"));
    let why = if channel {
        conflict(command)
    } else {
        conflict_session(command)
    };
    if let Some(why) = why {
        bail!("horse mode: {why}");
    }
    anyhow::ensure!(!command.is_empty(), "the claude command is empty");
    crate::hostcli::status::add_hooks(command, &setup.hooks)?;
    command.splice(1..1, setup.args.iter().cloned());
    Ok(())
}
