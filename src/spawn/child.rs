//! The host process of one child: it builds and runs the child sandbox.
use super::{
    child_network, remove_prompt, tail, write_prompt, ChildInfo, ChildPlan, ChildResult, Job,
    MAX_OUTPUT, MAX_STAT,
};
use crate::env::HostEnv;
use crate::spec::GuiMode;
use crate::state;
use anyhow::{bail, Context as _, Result};
use std::path::Path;

/// The host process of one child (`paddock spawn-child JOB`). The parent's
/// paddock starts it; it builds and runs the child sandbox and writes
/// `result.json` next to the job.
pub fn child_main(env: &HostEnv, job_file: &Path) -> Result<i32> {
    let dir = job_file.parent().context("bad job file")?.to_path_buf();
    if !dir.starts_with(env.paddock_run()) {
        bail!("the job file must be in {}", env.paddock_run().display());
    }
    let job: Job = serde_json::from_slice(
        &std::fs::read(job_file).with_context(|| format!("cannot read {}", job_file.display()))?,
    )?;
    let prompt = std::fs::read(dir.join("prompt.md"))?;
    let _ = std::fs::remove_file(dir.join("prompt.md"));
    let mut result = ChildResult {
        branch: job.branch.clone(),
        started: state::now(),
        ..Default::default()
    };
    let r = run_child(env, &job, prompt, &mut result);
    if let Err(e) = &r {
        result.error = Some(format!("{e:#}"));
    }
    result.ended = state::now();
    crate::privfs::write(&dir.join("result.json"), serde_json::to_vec(&result)?)?;
    Ok(if r.is_ok() { 0 } else { 1 })
}

fn run_child(env: &HostEnv, job: &Job, prompt: Vec<u8>, result: &mut ChildResult) -> Result<()> {
    let c = &job.ceiling;
    let factory = crate::profile::HarnessFactory::new(env)?;
    let harness = factory.create(&job.harness)?;
    let oneshot = harness
        .profile
        .oneshot
        .clone()
        .with_context(|| format!("the {} profile has no [oneshot] command", job.harness))?;
    crate::storage::guard::before_run(env, Some(&c.repo), true)?;
    let opts = crate::build::Options {
        harness: job.harness.clone(),
        project: Some(c.repo.clone()),
        backend: Some(c.backend.clone()),
        network: Some(child_network(job.role, c.network)),
        gui: Some(GuiMode::Off),
        // Both roles get a clean environment: only the allowlist and the
        // harness's own variables pass.
        inherit_env: Some(false),
        auth: Some(c.auth.clone()),
        worktree: true,
        child: Some(ChildPlan {
            id: job.sandbox_id.clone(),
            manifest: c.manifest.clone(),
            branch: job.branch.clone(),
            base_from: c.head_from.clone(),
            base: job.base.clone(),
            role: job.role,
        }),
        args: job.pick.args(&oneshot),
        ..Default::default()
    };
    let mut built = crate::build::build_with(env, &opts, harness)?;
    built.horse = c.horse;
    let wt = built.worktree.clone();
    if let Some(w) = &wt {
        result.base = w.base.clone();
    }
    let outcome = (|| -> Result<crate::run::Outcome> {
        let backend = crate::run::select_backend(env, &built)?;
        c.check(&built, backend.name())?;
        crate::run::probe_args(
            env,
            &built.harness_exe,
            &[oneshot.args.clone(), job.pick.args(&oneshot)].concat(),
            &oneshot.probe_args,
            "[oneshot] args",
        )?;
        // The child's broker narrows these again with the configuration as
        // it is when the child starts.
        let modes = c.modes.clone();
        built.spawn = Some(ChildInfo {
            spawn_id: job.spawn_id.clone(),
            parent: c.parent.clone(),
            chain: c.chain.clone(),
            depth: c.depth + 1,
            modes,
            role: job.role,
            hitch: job.hitch.clone(),
        });
        // paddock kv and the devices: the identity comes from the job, which
        // the parent's paddock wrote in a folder that no sandbox sees.
        built.parent = Some(c.parent.clone());
        built.device_ceiling = Some(c.devices.clone());
        let home = built.spec.home_src.clone();
        let prompt_id = built.spec.id.clone();
        // The file also when the prompt comes on stdin: the note after a
        // context compaction (compaction.rs) points the agent at it.
        if oneshot.prompt == crate::profile::PromptVia::Stdin {
            built.stdin = Some(prompt.clone());
        }
        write_prompt(&home, &prompt_id, &prompt, 0o600)?;
        let out = crate::run::run(env, built, backend.as_ref(), true, true);
        remove_prompt(&home, &prompt_id);
        out
    })();
    if let Some(w) = &wt {
        let (commits, uncommitted) = crate::build::worktree_counts(env, w);
        result.commits = commits;
        result.uncommitted = uncommitted;
        let stat = crate::hostgit::Layout::discover(&w.repo)
            .ok()
            .flatten()
            .and_then(|l| {
                l.try_query(&[
                    "diff",
                    "--stat",
                    "--no-textconv",
                    "--no-ext-diff",
                    "--no-color",
                    &format!("{}...{}", w.base, w.branch),
                ])
            })
            .unwrap_or_default();
        result.stat = tail(&stat, MAX_STAT).0;
        // Nothing to review: the worktree and the branch go at once.
        if commits == 0 && uncommitted == Some(0) {
            if let Err(e) = crate::worktree::reclaim_if_empty(env, w) {
                log::warn!(
                    "cannot remove the empty worktree {}: {e:#}",
                    w.path.display()
                );
            }
        }
    }
    let o = outcome?;
    result.exit_code = Some(o.code);
    let (text, cut) = tail(&o.stdout.unwrap_or_default(), MAX_OUTPUT);
    result.output = text;
    result.output_truncated = cut;
    Ok(())
}

impl super::Spawner {
    /// The end of a hitched child: its result goes to the parent's session,
    /// unless the child's own `paddock-host done` already sent it.
    pub(super) fn hitch_end(
        &self,
        hitched: &crate::horse::hitch::Hitched,
        s: &super::Status,
        r: &ChildResult,
    ) {
        let (status, summary) = super::result_parts(s, r);
        let mut text = format!("{status}: {summary}");
        if !r.output.trim().is_empty() {
            text.push('\n');
            text.push_str(r.output.trim_end());
        }
        let sent = crate::horse::Store::open(&crate::horse::root(&self.env))
            .and_then(|st| crate::horse::hitch::wire::finish_once(&st, hitched, &text));
        match sent {
            Ok(Some(_)) => log::info!("spawn {}: the result goes to the parent's horse", s.id),
            Ok(None) => {}
            Err(e) => log::warn!(
                "spawn {}: cannot queue the result to the parent: {e:#}",
                s.id
            ),
        }
    }
}
