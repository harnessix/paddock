//! `paddock freeze` of a session of the macOS guest (`backend = "macvm"`):
//! the freeze sends `pause` on the control socket of the shim, which pauses
//! the VM with `VZVirtualMachine.pause`, and the thaw sends `resume`. The
//! shim's process keeps running, so its Dock icon, its windows and its marks
//! stay. The audit lines and the kv state `sandbox:<id>` are the ones of
//! every freeze (`freeze.rs`).
//!
//! A quit of a frozen session is a thaw, then the quit: the app can save
//! its work, and the shim can save the state of the VM.

use crate::env::HostEnv;
use crate::state::Running;
use anyhow::{Context as _, Result};
use std::path::PathBuf;

/// What a user asks of a guest session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Freeze,
    Thaw,
    Quit,
}

/// The control lines of one op, given whether the session is frozen, and
/// whether it is frozen after them. A second freeze and a thaw of a
/// running session send nothing.
pub fn lines(frozen: bool, op: Op) -> (Vec<&'static str>, bool) {
    match (op, frozen) {
        (Op::Freeze, false) => (vec!["pause"], true),
        (Op::Freeze, true) => (vec![], true),
        (Op::Thaw, true) => (vec!["resume"], false),
        (Op::Thaw, false) => (vec![], false),
        (Op::Quit, true) => (vec!["resume", "stop"], false),
        (Op::Quit, false) => (vec!["stop"], false),
    }
}

/// A run of the macOS guest.
pub fn is_macvm(r: &Running) -> bool {
    r.backend == crate::macvm::host::BACKEND
}

/// The control socket of the session whose shim is `r.pid`, from the
/// session records of the guest.
pub fn control_of(env: &HostEnv, r: &Running) -> Result<PathBuf> {
    let layout = crate::macvm::layout::Layout::new(env);
    crate::macvm::host::sessions(&layout.sessions)
        .into_iter()
        .map(|(_, s)| s)
        .find(|s| s.pid == r.pid)
        .map(|s| s.control)
        .with_context(|| {
            format!(
                "{} has no session record of the macOS guest with pid {}, so paddock cannot reach its VM",
                r.id, r.pid
            )
        })
}

/// Send one line to the shim of the session. The shim answers once the
/// framework is done.
pub fn send(env: &HostEnv, r: &Running, line: &str) -> Result<()> {
    let sock = control_of(env, r)?;
    crate::vm::host::control(&sock, line)
        .map(drop)
        .with_context(|| format!("the macOS guest of {} did not take `{line}`", r.id))
}

/// Freeze (`pause`) or thaw (`resume`) a guest session, for `freeze::set`.
/// The caller already knows the state, so this sends one line.
pub fn set(env: &HostEnv, r: &Running, freeze: bool) -> Result<()> {
    let (lines, _) = lines(!freeze, if freeze { Op::Freeze } else { Op::Thaw });
    for line in lines {
        send(env, r, line)?;
    }
    Ok(())
}

/// Before the quit of a guest session (`paddock app stop`): a frozen
/// session is thawed through `freeze::thaw`, so the audit log, the kv
/// state and the timers see the thaw. True when it was frozen.
pub fn thaw_before_quit(env: &HostEnv, id: &str) -> Result<bool> {
    if super::frozen(env, id).is_none() {
        return Ok(false);
    }
    super::thaw(env, id, false)?;
    Ok(true)
}
