//! `self/ctl`: one context transform per write (`horse::device::ctl`).
//!
//! A write runs on its own thread, so a command that waits for the
//! approval card does not stop the other files of the sandbox. The write
//! returns when the command is done: its length when it ran, else EACCES
//! (refused), EPERM (denied), ETIMEDOUT, EINVAL or EIO. The answer text
//! of the last command is the content of `self/ctl` for the next open and
//! for a read on the same handle.

use super::super::{DeviceFs, Handle, Kind, MAX_RUNNING};
use super::HorseFs;
use crate::broker::{self, Answer as CardAnswer};
use crate::devices::ledger;
use crate::horse::device::ctl::{self, Failure};
use crate::horse::device::registry::{Answer, Host};
use fuser::{Errno, ReplyWrite};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The longest command line.
const MAX_LINE: usize = 64 * 1024;

/// The approval card and the device ledger of the sandbox.
struct FuseHost<'a>(&'a HorseFs);

impl Host for FuseHost<'_> {
    fn state_dir(&self) -> PathBuf {
        self.0.ctx.env.paddock_state()
    }

    fn ask(&self, summary: &str, detail: &str) -> Answer {
        let ctx = &self.0.ctx;
        let wait = broker::HostPolicy::load(&ctx.env).map_or(120, |p| p.wait);
        let now = crate::state::now();
        let p = broker::Pending {
            id: broker::new_request_id(),
            sandbox: ctx.sandbox.clone(),
            harness: ctx.harness.clone(),
            project: ctx.project.clone(),
            capability: "horse".into(),
            summary: summary.into(),
            detail: detail.into(),
            reason: String::new(),
            created: now,
            expires: now + wait,
            // Each break is its own card: its cost differs.
            session_allowed: false,
            started_by: ctx.started_by.clone(),
            choices: Vec::new(),
            connect: None,
        };
        let outcome = ctx
            .approver
            .ask_outcome(&ctx.env, &p, Duration::from_secs(wait));
        // This interface does not yet have a separate unavailable result.
        let unknown = matches!(
            &outcome,
            broker::ApprovalOutcome::Unknown | broker::ApprovalOutcome::Unavailable
        );
        let verdict = match outcome {
            broker::ApprovalOutcome::Answered(v) => Some(v),
            _ => None,
        };
        let answer = match verdict.map(|v| v.answer) {
            Some(CardAnswer::Once | CardAnswer::Session) => Answer::Approved,
            Some(_) => Answer::Denied,
            None if unknown => Answer::Unknown,
            None => Answer::TimedOut(wait),
        };
        log::info!(
            "sandbox {}: horse ctl card {}: {answer:?}",
            ctx.sandbox,
            p.id
        );
        answer
    }

    fn record(&self, line: &str) {
        let ctx = &self.0.ctx;
        let project = ctx.project.to_string_lossy();
        let payer = ledger::Payer {
            sandbox: &ctx.sandbox,
            root: ledger::root_of(&ctx.sandbox, &ctx.started_by),
            project: &project,
            device: "horse",
        };
        if let Err(e) =
            ledger::Ledger::open(&ledger::path(&ctx.env)).and_then(|l| l.record(&payer, line))
        {
            log::warn!("horse ledger: {e:#}");
        }
    }
}

/// The errno of a command that did not run.
pub(in crate::devices::fuse) fn errno(f: &Failure) -> Errno {
    match f.kind {
        ctl::Kind::Refused => Errno::EACCES,
        ctl::Kind::Invalid => Errno::EINVAL,
        ctl::Kind::Denied => Errno::EPERM,
        ctl::Kind::TimedOut => Errno::ETIMEDOUT,
        ctl::Kind::Failed => Errno::EIO,
    }
}

impl HorseFs {
    /// Run one command line, with the card and the ledger of the device.
    pub(in crate::devices::fuse) fn ctl(&self, data: &[u8]) -> Result<String, Failure> {
        let line = std::str::from_utf8(data).unwrap_or("\u{fffd}");
        let r = ctl::run(&self.setup, &self.ctx.sandbox, line, &FuseHost(self));
        *self.ctl_last.lock().unwrap() = match &r {
            Ok(out) => out.clone().into_bytes(),
            Err(f) => format!("{}\n", f.message).into_bytes(),
        };
        r
    }

    /// The answer to the last command.
    pub(in crate::devices::fuse) fn ctl_last(&self) -> Vec<u8> {
        self.ctl_last.lock().unwrap().clone()
    }
}

impl DeviceFs {
    /// A write of `self/ctl` on handle `fh`.
    pub(in crate::devices::fuse) fn ctl_write(&self, fh: u64, data: &[u8], reply: ReplyWrite) {
        if data.len() > MAX_LINE {
            return reply.error(Errno::EFBIG);
        }
        if self.inner.running.load(Ordering::SeqCst) >= MAX_RUNNING as u64 {
            return reply.error(Errno::EBUSY);
        }
        let inner = self.inner.clone();
        let data = data.to_vec();
        inner.running.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let Some(horse) = &inner.horse else {
                inner.running.fetch_sub(1, Ordering::SeqCst);
                return reply.error(Errno::EIO);
            };
            let r = horse.ctl(&data);
            let answer = horse.ctl_last();
            if let Some(Handle {
                kind: Kind::Horse(_, bytes, _),
                ..
            }) = inner.handles.lock().unwrap().get_mut(&fh)
            {
                *bytes = answer;
            }
            inner.running.fetch_sub(1, Ordering::SeqCst);
            match r {
                Ok(_) => reply.written(data.len() as u32),
                Err(f) => reply.error(errno(&f)),
            }
        });
    }
}
