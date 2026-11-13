use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::build;
use paddock::env::HostEnv;
use paddock::envset::{self, Mode, Scope};
use paddock::state;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum EnvCmd {
    /// What an agent gets in a project, in four groups: needed by the
    /// agent, your variables, keys, and what stays on this computer.
    /// Names only, unless --values.
    Show {
        path: Option<PathBuf>,
        /// The agent (a harness name). Without it: what every agent gets.
        #[arg(long)]
        agent: Option<String>,
        /// The record of a run: what it got at its start.
        #[arg(long, value_name = "ID")]
        run: Option<String>,
        /// Print JSON (api_version paddock.env/v1, or paddock.env.run/v1
        /// with --run).
        #[arg(long)]
        json: bool,
        /// Add the values of plain variables. Never the value of a key.
        #[arg(long, conflicts_with = "run")]
        values: bool,
    },
    /// Pass a host variable by name (NAME), or a fixed value (NAME=VALUE),
    /// for all agents, one agent, or one project.
    Add {
        #[arg(value_name = "NAME[=VALUE]")]
        var: String,
        #[arg(long, conflicts_with = "project")]
        agent: Option<String>,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Stop passing a variable of `paddock env add`.
    Remove {
        name: String,
        #[arg(long, conflicts_with = "project")]
        agent: Option<String>,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// listed: only the lists pass. inherit: the host environment passes,
    /// without secrets, cloud credentials and sockets.
    Mode {
        #[arg(value_parser = ["listed", "inherit"])]
        mode: String,
    },
    /// The review for current users: the host variables that pass unseen
    /// today. --yes passes only the suggested ones (or the --pass names)
    /// and sets mode listed.
    Migrate {
        /// Write the list and set mode listed.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Pass these instead of the suggested ones (repeatable).
        #[arg(long, value_name = "NAME", requires = "yes")]
        pass: Vec<String>,
        /// Print JSON (api_version paddock.env.review/v1).
        #[arg(long, conflicts_with = "yes")]
        json: bool,
    },
    /// Stop passing these variables in inherit mode (saved in the user configuration).
    Hide { names: Vec<String> },
    /// Pass these variables again.
    Unhide { names: Vec<String> },
}

fn scope(env: &HostEnv, agent: Option<String>, project: Option<PathBuf>) -> Result<Scope> {
    Ok(match (agent, project) {
        (Some(a), _) => Scope::Agent(a),
        (None, Some(p)) => {
            let p = build::resolve_project(Some(&p))?;
            Scope::Project(envset::project_key(env, &p))
        }
        (None, None) => Scope::User,
    })
}

/// A refusal of the never-pass list or of a secret name: its sentence on
/// standard error, and exit code 2.
fn refused(e: anyhow::Error) -> Result<i32> {
    match e.downcast_ref::<envset::Refusal>() {
        Some(r) => {
            eprintln!("paddock: {}", r.sentence);
            Ok(envset::REFUSED_EXIT)
        }
        None => Err(e),
    }
}

pub(crate) fn run(env: &HostEnv, cmd: EnvCmd) -> Result<i32> {
    match cmd {
        EnvCmd::Hide { names } | EnvCmd::Unhide { names } if names.is_empty() => {
            bail!("name at least one variable")
        }
        EnvCmd::Hide { names } => {
            let all = state::edit_env_hide(env, &names, &[])?;
            println!("Hidden from sandboxes: {}", all.join(", "));
        }
        EnvCmd::Unhide { names } => {
            let all = state::edit_env_hide(env, &[], &names)?;
            println!(
                "Hidden from sandboxes: {}",
                if all.is_empty() {
                    "(none)".into()
                } else {
                    all.join(", ")
                }
            );
        }
        EnvCmd::Add {
            var,
            agent,
            project,
        } => {
            let scope = scope(env, agent, project)?;
            let (name, value) = match var.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (var.clone(), None),
            };
            if let Err(e) = envset::add(env, &scope, &name, value.as_deref()) {
                return refused(e);
            }
            match value {
                None if env.var(&name).is_none() => println!(
                    "{name} passes from this computer for {}, but it is not set here now. {}",
                    scope.describe(),
                    envset::NEXT_START
                ),
                None => println!(
                    "{name} passes from this computer for {}. {}",
                    scope.describe(),
                    envset::NEXT_START
                ),
                Some(_) => println!(
                    "{name} has a fixed value for {}. {}",
                    scope.describe(),
                    envset::NEXT_START
                ),
            }
        }
        EnvCmd::Remove {
            name,
            agent,
            project,
        } => {
            let scope = scope(env, agent, project)?;
            if !envset::remove(env, &scope, &name)? {
                bail!("{name} is not in the list of {}", scope.describe());
            }
            println!(
                "{name} no longer passes for {}. {}",
                scope.describe(),
                envset::NEXT_START
            );
        }
        EnvCmd::Mode { mode } => {
            let m = Mode::parse(&mode).expect("clap checks the value");
            envset::set_mode(env, m)?;
            println!(
                "{}. {}",
                match m {
                    Mode::Listed => "Only the lists pass: paddock env show names them",
                    Mode::Inherit => "The host environment passes, without secrets, cloud credentials and sockets",
                },
                envset::NEXT_START
            );
        }
        EnvCmd::Migrate { yes, pass, json } => {
            let project = build::resolve_project(None)?;
            let r = envset::review(env, &project)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
                return Ok(0);
            }
            if yes {
                let list: Vec<String> = if pass.is_empty() {
                    r.items
                        .iter()
                        .filter(|i| i.suggested)
                        .map(|i| i.name.clone())
                        .collect()
                } else {
                    pass
                };
                if let Err(e) = envset::finish_review(env, &list) {
                    return refused(e);
                }
                println!(
                    "Only the lists pass now. Added to [env] pass: {}. {} To go back: paddock env mode inherit",
                    if list.is_empty() {
                        "(none)".to_string()
                    } else {
                        list.join(", ")
                    },
                    envset::NEXT_START
                );
                return Ok(0);
            }
            println!("{}", r.summary);
            if let Some(f) = &r.forced_from {
                println!("From paddock {f}, only the lists pass.");
            }
            for i in &r.items {
                println!(
                    "  {} {}{}",
                    if i.suggested { "[x]" } else { "[ ]" },
                    envset::cut(&i.name, 40),
                    if i.listed { "  (in [env] pass)" } else { "" }
                );
            }
            println!("\nPass only the ticked ones: paddock env migrate --yes");
            println!("Choose others: paddock env migrate --yes --pass NAME --pass NAME");
            println!("Keep passing everything: paddock env mode inherit");
        }
        EnvCmd::Show {
            path,
            agent,
            run,
            json,
            values,
        } => {
            if let Some(id) = run {
                let r = envset::load_record(env, &id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&r)?);
                } else {
                    for l in envset::record_lines(&r) {
                        println!("{l}");
                    }
                }
                return Ok(0);
            }
            let project = build::resolve_project(path.as_deref())?;
            let factory = paddock::profile::HarnessFactory::new(env)?;
            let profile = match &agent {
                Some(a) => Some(factory.profile(a).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "there is no agent {a}; the agents are: {}",
                        factory.names().join(", ")
                    )
                })?),
                None => None,
            };
            let v = envset::view(&envset::Input {
                env,
                project: &project,
                profile: profile.as_ref(),
                cli: None,
                values,
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for l in envset::plain_lines(&v) {
                    println!("{l}");
                }
            }
        }
    }
    Ok(0)
}
