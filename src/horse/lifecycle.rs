//! Run ownership, native transcript attribution and final reconciliation.
use super::{claude, codex, dsh, live, run, Store};
use crate::{build::Built, env::HostEnv};
use anyhow::{bail, Context, Result};
use run::setup::Custody;
use serde_json::json;
use std::{path::PathBuf, time::Duration};

pub struct Lifecycle {
    env: HostEnv,
    run: run::RunHorse,
    sandbox: String,
    home: PathBuf,
    before: run::snapshot::Snapshot,
    /// The exit code of the harness, when the run got that far.
    exit: Option<i32>,
    custody: Custody,
    /// What a live custody of a headless run said: the output of the run.
    /// None for a TUI run.
    output: Option<live::headless::Captured>,
}

/// The end of a run with its exit code: the final ingest and the adapt
/// rules of the DNA run now. `out`: the captured stdout of the sandbox.
/// When the custody held stdout, the output of the custody replaces it.
pub fn end(l: Option<Lifecycle>, code: i32, out: Option<String>) -> Option<String> {
    // Outside horse mode the captured output passes through untouched.
    let Some(mut l) = l else {
        return out;
    };
    l.exit = Some(code);
    let said = l.output.clone();
    drop(l);
    out.or_else(|| said.map(|c| c.text()))
}

/// Capture the private home after shadow setup and before launching the sandbox.
pub fn start(env: &HostEnv, built: &mut Built) -> Result<Option<Lifecycle>> {
    if !built.horse {
        return Ok(None);
    }
    if built.detach {
        bail!("horse mode does not support detached runs yet");
    }
    let (expected, check): (_, fn(&str) -> Result<()>) = match built.spec.harness.as_str() {
        "dsh" => (dsh::VERSION, dsh::check_version),
        "claude" => (claude::VERSION, claude::check_version),
        "codex" => (codex::VERSION, codex::check_version),
        other => bail!("no pinned horse codec for {other}"),
    };
    let output = std::process::Command::new(&built.harness_exe)
        .arg("--version")
        .output()
        .context("cannot determine harness version for horse mode")?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !(output.status.success() && text.split_whitespace().any(|v| v == expected)) {
        // The codec says why, for the version that the harness printed.
        let found = text
            .split_whitespace()
            .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
            .unwrap_or("");
        check(found).context("horse mode")?;
        bail!(
            "horse mode requires {} version {expected}; got {}",
            built.spec.harness,
            text.trim()
        );
    }
    let (custody, line) = run::setup::choose(built, expected);
    if let Some(line) = line {
        eprintln!("{line}");
    }
    // The command and the task of a headless live run of Claude Code or
    // Codex, or the channel of a Claude Code TUI, before the store gets
    // anything.
    let headless = run::setup::prepare(built, custody, expected)?;
    if custody == Custody::Pty {
        eprintln!(
            "horse: {} gets queued entries between its turns: paddock types them into the TUI when it is idle",
            built.spec.harness
        );
    }
    if custody == Custody::Channel {
        eprintln!("horse: claude gets queued entries between its turns through the paddock channel; confirm the development channel when Claude Code asks");
    }
    let home = built.spec.home_src.clone();
    let before = run::snapshot::Snapshot::capture(&home, &built.spec.harness)?;
    if built.spec.harness == dsh::HARNESS {
        live::sdk_command(&mut built.spec.command, &built.profile.launch.args)?;
    }
    let store = Store::open(&super::root(env))?;
    // A hitched child: the parent's paddock made its session already.
    let hitched = built.spawn.as_ref().and_then(|c| c.hitch.clone());
    let rh = match &hitched {
        Some(h) => {
            let rh = h.run();
            let session = store.session(&rh.session)?;
            anyhow::ensure!(
                session.horse == rh.horse && rh.harness == built.spec.harness,
                "the hitched session does not match this run"
            );
            rh
        }
        None => {
            // A plain spawn from a run in horse mode: a child horse.
            let parent = match &built.spawn {
                Some(c) => run::by_sandbox::find(&store, &c.parent)?.map(|p| p.horse),
                None => None,
            };
            run::begin(
                &store,
                &built.spec.harness,
                expected,
                parent.as_deref(),
                None,
            )?
        }
    };
    run::by_sandbox::record(&store, &built.spec.id, &rh)?;
    run::record_event(&store, &rh, "run.begin", &built.spec.id, json!({}))?;
    if rh.harness == dsh::HARNESS {
        let cfg = live::settings(env)?;
        let main = format!("session-{}", super::uuid_v4()?);
        store.lock()?.bind_native(&rh.session, &main)?;
        live::arm(live::Setup {
            store: store.clone(),
            horse: rh.horse.clone(),
            main,
            resume: false,
            sessions_root: Some(home.join(".dsh/sessions")),
            cwd: built.spec.workdir.display().to_string(),
            provider: cfg.dsh_provider,
            model: cfg.dsh_model,
            effort: cfg.dsh_effort,
            run: built.spec.id.clone(),
            tick: Duration::from_millis(200),
        });
    }
    let output = match headless {
        Some(p) => run::setup::arm(env, &store, &rh, built, p)?,
        None => None,
    };
    Ok(Some(Lifecycle {
        env: env.clone(),
        run: rh,
        sandbox: built.spec.id.clone(),
        home,
        before,
        exit: None,
        custody,
        output,
    }))
}

impl Lifecycle {
    pub(crate) fn access(&self) -> Result<super::device::Access> {
        Ok(super::device::Access {
            store: Store::open(&super::root(&self.env))?,
            horse: self.run.horse.clone(),
            session: self.run.session.clone(),
        })
    }
    fn finish(&self) -> Result<()> {
        let (custody, report) = match self.custody {
            Custody::Live | Custody::Channel | Custody::Pty => {
                let (r, report) = run::setup::finish();
                (r, Some((self.custody, report)))
            }
            Custody::FinalOnly => (Ok(()), None),
        };
        let store = Store::open(&super::root(&self.env))?;
        let ok = self.exit == Some(0);
        run::final_only::end(
            &store,
            &self.run,
            &self.sandbox,
            &self.home,
            &self.before,
            ok,
            report,
        )?;
        custody
    }
}
impl Drop for Lifecycle {
    fn drop(&mut self) {
        if let Err(e) = self.finish() {
            eprintln!("paddock: horse final ingest: {e:#}");
        }
    }
}

#[cfg(test)]
mod channel_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod pty_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod end_tests {
    #[test]
    fn a_run_outside_horse_mode_keeps_its_output() {
        assert_eq!(
            super::end(None, 0, Some("FINAL ANSWER: x".into())),
            Some("FINAL ANSWER: x".into())
        );
        assert_eq!(super::end(None, 3, None), None);
    }
}
