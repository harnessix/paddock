use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::{env::HostEnv, turnout};

#[derive(Subcommand)]
pub(crate) enum TurnoutCmd {
    /// List configured jobs.
    Ls,
    /// Install a job timer.
    Install { name: String },
    /// Remove a job timer.
    Remove { name: String },
    /// Run one wake.
    Run { name: String },
    /// Stop scheduled work, for one job or all jobs.
    Pause { name: Option<String> },
    /// Resume scheduled work, for one job or all jobs.
    Resume { name: Option<String> },
    /// Show the health records of one job.
    Log { name: String },
    /// Summarize the recorded wakes of each job.
    Health,
    #[command(hide = true)]
    Retry { name: String },
    #[command(hide = true)]
    Worker {
        name: String,
        #[arg(long)]
        checkout: Option<std::path::PathBuf>,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        output: Option<std::path::PathBuf>,
        #[arg(long)]
        summary: Option<std::path::PathBuf>,
    },
    /// Show a configured job.
    Show { name: String },
}

pub(crate) fn run(env: &HostEnv, cmd: TurnoutCmd) -> Result<i32> {
    if let TurnoutCmd::Pause { name } = &cmd {
        let state = env.paddock_state().join("turnout");
        let names = if let Some(name) = name {
            if !turnout::valid_name(name) {
                bail!("invalid turnout job name");
            }
            vec![name.clone()]
        } else {
            let mut names = turnout::pause::known_jobs(&state);
            if let Ok(jobs) = turnout::load(env) {
                names.extend(jobs.into_keys());
            }
            names.sort();
            names.dedup();
            names
        };
        turnout::pause::pause(
            &state,
            &names,
            name.is_none(),
            std::path::Path::new("systemctl"),
        )?;
        return Ok(0);
    }
    let cmd = match cmd {
        TurnoutCmd::Retry { name } => {
            if !turnout::valid_name(&name) {
                bail!("invalid turnout job name");
            }
            if !turnout::wake::pending_path(&env.paddock_state().join("turnout"), &name).exists() {
                return Ok(0);
            }
            TurnoutCmd::Run { name }
        }
        other => other,
    };
    let project_file = std::env::current_dir()?.join(paddock::manifest::PROJECT_FILE);
    if project_file.is_file() {
        paddock::manifest::Manifest::read(&project_file)?;
    }
    let jobs = turnout::load(env)?;
    match cmd {
        TurnoutCmd::Ls => {
            for name in jobs.keys() {
                println!("{name}");
            }
        }
        TurnoutCmd::Install { name } => {
            let job = jobs
                .get(&name)
                .ok_or_else(|| anyhow::anyhow!("no turnout job named {name}"))?;
            turnout::units::install(
                &env.config_dir.join("systemd/user"),
                std::path::Path::new("systemctl"),
                &std::env::current_exe()?,
                &name,
                job,
            )?;
        }
        TurnoutCmd::Remove { name } => {
            if !turnout::valid_name(&name) {
                bail!("invalid turnout job name");
            }
            turnout::units::remove(
                &env.config_dir.join("systemd/user"),
                std::path::Path::new("systemctl"),
                &name,
            )?;
            match std::fs::remove_file(turnout::wake::pending_path(
                &env.paddock_state().join("turnout"),
                &name,
            )) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        TurnoutCmd::Run { name } => {
            let job = jobs
                .get(&name)
                .ok_or_else(|| anyhow::anyhow!("no turnout job named {name}"))?;
            if let Some(project) = &job.project {
                let project_file =
                    std::path::Path::new(project).join(paddock::manifest::PROJECT_FILE);
                if project_file.is_file() {
                    paddock::manifest::Manifest::read(&project_file)?;
                }
            }
            turnout::report::wake_with_retry(
                env,
                &name,
                job,
                (
                    paddock::state::now(),
                    if job.max_load.is_some() {
                        turnout::limits::host_load().unwrap_or(f64::INFINITY)
                    } else {
                        0.0
                    },
                ),
                || {
                    if job.trigger != "timer" {
                        return Ok(None);
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let load = if job.max_load.is_some() {
                        turnout::limits::host_load().unwrap_or(f64::INFINITY)
                    } else {
                        0.0
                    };
                    Ok(Some((paddock::state::now(), load)))
                },
                |head, generation| {
                    let state = env.paddock_state().join("turnout");
                    let cancelled = || turnout::pause::cancelled(&state, &name, generation);
                    if name == "host-checks" {
                        return turnout::host_checks::perform(
                            env,
                            job,
                            head.ok_or_else(|| {
                                anyhow::anyhow!("host-checks needs a branch commit")
                            })?,
                            &cancelled,
                        );
                    }
                    turnout::budget::run(
                        std::path::Path::new("systemd-run"),
                        std::path::Path::new("systemctl"),
                        &cancelled,
                        &std::env::current_exe()?,
                        &name,
                        job,
                        &[],
                    )?;
                    Ok("completed".to_owned())
                },
            )?;
        }
        TurnoutCmd::Pause { .. } | TurnoutCmd::Retry { .. } => unreachable!(),
        TurnoutCmd::Resume { name } => {
            if name.as_deref().is_some_and(|n| !turnout::valid_name(n)) {
                bail!("invalid turnout job name");
            }
            turnout::pause::resume(&env.paddock_state().join("turnout"), name.as_deref())?;
        }
        TurnoutCmd::Worker {
            name,
            checkout,
            commit,
            output,
            summary,
        } => {
            let job = jobs
                .get(&name)
                .ok_or_else(|| anyhow::anyhow!("no turnout job named {name}"))?;
            if job.harness != "none" {
                bail!("agent jobs are not available yet");
            }
            if name == "host-checks" {
                let checkout =
                    checkout.ok_or_else(|| anyhow::anyhow!("missing checks checkout"))?;
                let commit = commit.ok_or_else(|| anyhow::anyhow!("missing checks commit"))?;
                let output = output.ok_or_else(|| anyhow::anyhow!("missing checks output"))?;
                let summary = summary.ok_or_else(|| anyhow::anyhow!("missing checks summary"))?;
                let tree_root = env.paddock_cache().join("turnout/worktrees");
                if checkout.parent() != Some(tree_root.as_path())
                    || checkout.exists()
                    || !checkout
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("checkout-") && turnout::valid_name(n))
                    || output.parent() != Some(env.paddock_state().join("turnout").as_path())
                    || summary.parent() != Some(env.paddock_state().join("turnout").as_path())
                {
                    bail!("invalid checks paths");
                }
                if let Err(error) = turnout::host_checks::Worktree::add(
                    std::path::Path::new(
                        job.project
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("missing checks project"))?,
                    ),
                    &checkout,
                    &commit,
                ) {
                    paddock::privfs::write_new(&output, format!("checkout failed: {error:#}\n"))?;
                    return Err(error);
                }
                let seconds = job
                    .suite_timeout
                    .as_deref()
                    .map(paddock::timer::parse_duration)
                    .transpose()?
                    .unwrap_or(900);
                let text = turnout::host_checks::suites(
                    std::path::Path::new("cargo"),
                    &checkout,
                    &job.suites,
                    std::time::Duration::from_secs(seconds),
                    &output,
                )?;
                paddock::privfs::write_atomic(&summary, text)?;
            }
        }
        TurnoutCmd::Log { name } => {
            if !turnout::valid_name(&name) {
                bail!("invalid turnout job name");
            }
            for row in turnout::report::read(&env.paddock_state().join("turnout"), &name)? {
                println!("{}", serde_json::to_string(&row)?);
            }
        }
        TurnoutCmd::Health => {
            for name in jobs.keys() {
                let rows = turnout::report::read(&env.paddock_state().join("turnout"), name)?;
                let failures = rows.iter().filter(|row| row.error.is_some()).count();
                println!("{name}: {} wakes, {failures} failures", rows.len());
            }
        }
        TurnoutCmd::Show { name } => match jobs.get(&name) {
            Some(job) => print!("{}", toml::to_string_pretty(job)?),
            None => bail!("no turnout job named {name}"),
        },
    }
    Ok(0)
}
