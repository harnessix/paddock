//! Custody of an interactive Claude Code run through the pty relay, for a
//! Claude Code version with no checked channel path (`claude::channel_pin`).
//!
//! The command gets the session id and the `Stop` and `UserPromptSubmit`
//! hooks, as for the channel custody, but no channel server
//! (`channel::prepare_hooks`). The turns are followed as in the channel
//! custody (live/channel.rs): the hooks through the TUI state, the
//! transcript tail for a hook that did not come, the ingest after every
//! `Stop` and every 5 s of a long turn, the landing of each entry by its
//! user row, and one retry of an entry that does not arrive (`pty.retry`
//! in the ledger).
//!
//! What this custody adds: it types the oldest entry into the pty when
//! the TUI is idle by all three parts of pty.rs (no hook and no row says
//! that a turn runs, the screen shows the empty input row and no busy row,
//! and the screen did not change for `quiet_ms`). For an urgent entry it
//! sends the interrupt key once while a turn runs, then waits for idle by
//! the same rule.
//!
//! The TUI state starts busy: the first entry goes out after the first
//! `Stop`, as in the channel custody. paddock never guesses that no turn
//! runs.

use super::{is_idle, type_entry, urgent_entry, PtyIdle, PtyTap};
use crate::horse::claude as codec;
use crate::horse::live::channel;
use crate::horse::live::harness::{Driver, End, Input, Live, LiveSession, Pump};
use crate::horse::live::tui_state::{run_key, SignalRx};
use crate::horse::store::Store;
use anyhow::{bail, Result};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// What the loop needs: the context of the turns, as for the channel
/// custody, and the pty.
pub struct CustodyCtx {
    pub tui: channel::CustodyCtx,
    pub idle: PtyIdle,
    pub tap: Arc<PtyTap>,
}

/// What a custody did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The turns, the landings and the retries. `released` counts the
    /// entries typed into the TUI.
    pub tui: channel::Report,
    /// Interrupt keys sent for urgent entries.
    pub interrupts: usize,
    /// The relay went away while the run went on: nothing more was typed.
    pub broken: bool,
}

/// One Claude Code TUI run in custody.
pub struct Custodian {
    turns: channel::Custodian,
    store: Store,
    session: String,
    idle: PtyIdle,
    tap: Arc<PtyTap>,
    /// The interrupt key went out, and the TUI was not idle since.
    interrupted: bool,
    /// Urgent entries that got their interrupt key.
    interrupted_for: HashSet<String>,
    /// The relay is gone: nothing more is typed.
    broken: bool,
    interrupts: usize,
}

/// Hold the TUI run until the run ends (two `Close`s on the control
/// channel), then ingest a last time.
pub fn run(ctx: CustodyCtx, signals: SignalRx) -> Result<Report> {
    let CustodyCtx { mut tui, idle, tap } = ctx;
    codec::check_version(&tui.version)?;
    let (_keep, dummy) = std::sync::mpsc::channel();
    let control = std::mem::replace(&mut tui.control, dummy);
    let (runs, key) = (tui.runs, run_key(&tui.horse, &tui.session));
    let mut d = Driver::new(tui.store.clone(), &tui.horse, &tui.run, tui.timing.tick);
    let mut c = Custodian::new(tui, idle, tap, signals)?;
    let r = d.drive(&mut c, &control);
    runs.forget(&key);
    r?;
    Ok(c.report())
}

impl Custodian {
    pub fn new(
        tui: channel::CustodyCtx,
        idle: PtyIdle,
        tap: Arc<PtyTap>,
        signals: SignalRx,
    ) -> Result<Self> {
        let (store, session) = (tui.store.clone(), tui.session.clone());
        // No channel server of the sandbox gets an entry of this run.
        tui.runs.typed(&run_key(&tui.horse, &tui.session));
        let mut turns = channel::Custodian::new(tui, signals)?;
        turns.typed_by_pty();
        Ok(Custodian {
            turns,
            store,
            session,
            idle,
            tap,
            interrupted: false,
            interrupted_for: HashSet::new(),
            broken: false,
            interrupts: 0,
        })
    }

    pub fn report(&self) -> Report {
        Report {
            tui: self.turns.report().clone(),
            interrupts: self.interrupts,
            broken: self.broken,
        }
    }

    /// The screen part of the idle test.
    fn screen_idle(&self) -> bool {
        self.tap
            .screen()
            .is_some_and(|(s, unchanged)| is_idle(&s, &self.idle, unchanged))
    }

    /// Idle by all three parts.
    fn idle(&self) -> bool {
        !self.broken && self.turns.quiet() && self.screen_idle()
    }

    /// An urgent entry waits while a turn runs: send the interrupt key
    /// once for it. The TUI ends the turn and writes its rows; the entry
    /// goes out when the TUI is idle again.
    fn urgent(&mut self) -> Result<()> {
        if self.turns.quiet() {
            self.interrupted = false;
            return Ok(());
        }
        if self.interrupted || self.broken {
            return Ok(());
        }
        let Some(e) = urgent_entry(&self.store, &self.session, &self.interrupted_for)? else {
            return Ok(());
        };
        self.interrupted_for.insert(e.id.clone());
        match self.tap.interrupt(&self.idle) {
            Ok(()) => {
                self.interrupted = true;
                self.interrupts += 1;
                self.turns.note(&format!("pty.interrupt {}", e.id));
            }
            Err(err) => {
                self.broken = true;
                log::warn!("horse pty: cannot send the interrupt key: {err}");
            }
        }
        Ok(())
    }
}

impl Live for Custodian {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        let p = self.turns.pump(timeout)?;
        // The TUI goes on after an error of the store: try again in the
        // next pass.
        if let Err(e) = self.urgent() {
            log::warn!("horse pty: {e:#}");
        }
        Ok(p)
    }

    fn main_session(&self) -> Option<String> {
        self.turns.main_session()
    }

    fn sessions(&self) -> Vec<LiveSession> {
        self.turns.sessions()
    }

    fn safe(&self, session: Option<&str>) -> bool {
        session.is_none_or(|s| s == self.session) && self.idle()
    }

    fn deliver(&mut self, input: Input) -> Result<()> {
        let e = match input {
            Input::Entry(e) => e,
            Input::Typed(_) => bail!("the user types into the TUI directly"),
        };
        match type_entry(&self.store, &self.tap, &self.idle, &e)? {
            Some((text, head)) => {
                self.turns.track(&e.id, &text, head);
                self.turns.note(&format!("pty.deliver {}", e.id));
            }
            None => self.broken = true,
        }
        Ok(())
    }

    fn interrupt(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn close_input(&mut self) -> Result<()> {
        self.turns.close_input()
    }

    fn finish(&mut self, end: End) -> Result<()> {
        self.turns.finish(end)
    }
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
