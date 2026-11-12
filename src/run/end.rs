//! The end steps of a run, in a guard, so they run even when `run`
//! returns early.

use crate::build::Built;
use crate::env::HostEnv;
use std::path::PathBuf;

/// Cleans the run folder, the borrow copies, kv, devices, the pasture
/// member and the session record, even when `run` returns early. Last, it
/// writes the end of the run into its mailbox.
pub(super) struct EndOfRun<'a> {
    pub env: &'a HostEnv,
    pub id: String,
    pub project: PathBuf,
    pub snapshot: bool,
    pub quiet: bool,
    pub kv: Option<crate::kv::Served>,
    pub devices: Option<crate::devices::Devices>,
    pub bridge: Option<PathBuf>,
    pub harness: String,
    /// This run copied output to a view log.
    pub view: bool,
    /// The run's mailbox, and the exit code of the sandbox once it ended.
    pub mail: Option<crate::mail::events::RunMail>,
    pub trainer: Option<crate::trainer::monitor::Monitor>,
    pub code: Option<i32>,
}

impl<'a> EndOfRun<'a> {
    /// At the start of a run: also makes its mailbox and writes `started`.
    pub fn new(env: &'a HostEnv, built: &Built, quiet: bool) -> Self {
        if built.horse {
            let save = || -> anyhow::Result<()> {
                let store = crate::horse::Store::open_host(env)?;
                if crate::trainer::anchor::load(&store, &built.spec.id)?.is_none() {
                    let base = built.worktree.as_ref().map(|w| w.base.clone()).or_else(|| {
                        crate::hostgit::Layout::discover(&built.spec.project)
                            .ok()
                            .flatten()
                            .and_then(|git| git.try_query(&["rev-parse", "HEAD"]))
                            .map(|s| s.trim().to_string())
                    });
                    let anchor = crate::trainer::anchor::Anchor {
                        goal: built
                            .stdin
                            .as_deref()
                            .map(String::from_utf8_lossy)
                            .unwrap_or_default()
                            .into_owned(),
                        base,
                        hidden: crate::trainer::anchor::hidden_for(env, built),
                        ceiling: None,
                    };
                    crate::trainer::anchor::save(&store, &built.spec.id, &anchor)?;
                }
                Ok(())
            };
            if let Err(error) = save() {
                log::warn!(
                    "cannot record trainer anchor for {}: {error:#}",
                    built.spec.id
                );
            }
        }
        let mail = crate::mail::events::start(
            env,
            &crate::mail::events::Start {
                id: &built.spec.id,
                harness: &built.spec.harness,
                project: &built.spec.project,
                detached: built.detach,
                delivery: crate::mail::deliver::of_spec(&built.spec),
                parent: built
                    .spawn
                    .as_ref()
                    .map(|c| (c.parent.as_str(), c.spawn_id.as_str())),
            },
        );
        EndOfRun {
            env,
            id: built.spec.id.clone(),
            project: built.spec.project.clone(),
            snapshot: false,
            quiet,
            kv: None,
            devices: None,
            bridge: None,
            harness: built.spec.harness.clone(),
            view: false,
            mail,
            trainer: built
                .horse
                .then(|| crate::trainer::monitor::start(env, &built.spec.id))
                .flatten(),
            code: None,
        }
    }

    /// After the sandbox home is written: record again whether the harness
    /// runs the mail hooks. A Codex hook runs only with the trust that the
    /// write put into the home (mail/deliver.rs `of_spec`).
    pub fn home_written(&self, spec: &crate::spec::SandboxSpec) {
        if self.mail.is_none() {
            return;
        }
        let hooks = crate::mail::deliver::of_spec(spec).is_some();
        if let Err(e) = crate::mail::deliver::record_hooks(self.env, &self.id, hooks) {
            log::warn!(
                "sandbox {}: cannot record the mail delivery: {e:#}",
                self.id
            );
        }
    }
}

impl Drop for EndOfRun<'_> {
    fn drop(&mut self) {
        self.trainer.take();
        // Unmount first: the removal must not walk into the device mount.
        self.devices.take();
        if let Some(k) = self.kv.take() {
            k.finish();
        }
        if let Some(base) = &self.bridge {
            crate::pasture::bridge::forget_member(base, &self.id);
        }
        // Before the run folder goes: quota peers keep the end of this run.
        crate::timer::end_runs(self.env, &self.id, crate::state::now());
        crate::sockpath::end_run(&self.env.paddock_run(), self.env.uid, &self.id);
        crate::borrow::end(self.env, &self.id);
        crate::recover::Session::end(self.env, &self.id);
        if self.view {
            crate::detach::record_view_end(self.env, &self.id, &self.harness, &self.project);
        }
        if self.snapshot {
            crate::snapshot::after_end(self.env, &self.project, &self.id, self.quiet);
        }
        crate::loft::end_of_run(self.env, &self.project, &self.id, &self.harness);
        // Last: a manager that reads `ended` finds the run cleaned up.
        if let Some(m) = self.mail.take() {
            m.finish(self.code);
        }
    }
}

/// The run folder of a shadow run: removed when the run ends or fails.
pub(super) struct ShadowRun(pub(super) crate::shadow::Plan);

impl Drop for ShadowRun {
    fn drop(&mut self) {
        if let Err(e) = self.0.remove() {
            log::warn!("{e:#}");
        }
    }
}
