use anyhow::{Context as _, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::gauge;
use paddock::speckit::flow::{self, Spot};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum SpecCmd {
    /// The active Spec Kit feature of a project, its tasks with state, owner and branch, what can start, and what waits for review.
    Status {
        /// The project. Default: the current folder.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// A feature folder in specs/. Default: the active feature.
        #[arg(long)]
        feature: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// The appetite of a feature: the time you want to spend on it, from its start (3d, 36h, 1d12h), or off. paddock warns at 50, 80 and 100 % and every 25 % after that, and never stops the work. Without VALUE, print it.
    Appetite {
        feature: String,
        value: Option<String>,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Record the result of a gate run on a task (pass, fail or skip), for `paddock gauge`. A gate that only skipped is no evidence that the task is done.
    Gate {
        /// The task id, for example T012.
        task: String,
        /// The gate name, for example tests or real-sandbox.
        gate: String,
        /// pass, fail or skip.
        result: String,
        #[arg(long)]
        feature: Option<String>,
        #[arg(long)]
        note: Option<String>,
        /// Exclave peer name; requires --snapshot-hash.
        #[arg(long, requires = "snapshot_hash")]
        peer: Option<String>,
        /// Snapshot BLAKE3 hash (64 lowercase hex digits); requires --peer.
        #[arg(long, requires = "peer")]
        snapshot_hash: Option<String>,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Move .specify/ and specs/ of a repository into the loft, after a question. Needs the loft (`paddock loft init`) or [speckit] store = "external".
    Adopt {
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Do not ask.
        #[arg(long)]
        yes: bool,
    },
}

fn project(dir: Option<PathBuf>) -> Result<PathBuf> {
    let d = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    std::fs::canonicalize(&d).with_context(|| format!("no folder {}", d.display()))
}

pub(crate) fn run(env: &HostEnv, cmd: SpecCmd) -> Result<i32> {
    match cmd {
        SpecCmd::Status { dir, feature, json } => {
            let project = project(dir)?;
            let Some(layout) = paddock::speckit::Layout::resolve(env, &project) else {
                anyhow::bail!(
                    "{} is no Spec Kit project (no .specify/ folder), or [speckit] enabled = false",
                    project.display()
                );
            };
            let now = paddock::state::now();
            let feature = feature
                .or_else(|| layout.feature())
                .context("this Spec Kit project has no feature with a tasks.md yet")?;
            let repo = paddock::speckit::repo_of(&project);
            // The newest run of this repository with records of the feature.
            let root = flow::newest_tree(env, &repo, &feature, now);
            let spot = Spot::find(
                env,
                &project,
                root.as_deref().unwrap_or("no-run"),
                Some(&feature),
            )?
            .context("no Spec Kit project")?;
            let has_records =
                paddock::speckit::records::ProjectRecords::path_for(&spot.layout, &feature)
                    .is_file();
            let v = spot.sync(env, false, now)?;
            if json {
                let mut j = flow::to_json(&v, &spot.layout);
                j["api_version"] = "paddock.spec/v1".into();
                j["run"] = root.into();
                j["store_path"] = serde_json::json!(layout.root);
                j["git_tracked"] = if layout.external {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(paddock::speckit::git::tracked(&layout.root))
                };
                println!("{j}");
            } else {
                println!("{}", paddock::speckit::git::location(&layout));
                println!("{}", flow::to_text(&v));
                if !has_records {
                    println!(
                        "(no run has worked on these tasks yet: the states come from tasks.md)"
                    );
                }
            }
            Ok(0)
        }
        SpecCmd::Appetite {
            feature,
            value,
            dir,
        } => appetite(env, &project(dir)?, &feature, value.as_deref()),
        SpecCmd::Gate {
            task,
            gate,
            result,
            feature,
            note,
            peer,
            snapshot_hash,
            dir,
        } => {
            paddock::update::refuse_in_sandbox(env, "paddock spec gate")?;
            let project = project(dir)?;
            let layout = spec_layout(env, &project)?;
            let feature = gauge::collect::feature(&layout, feature.as_deref())?;
            anyhow::ensure!(
                paddock::speckit::tasks::is_id(&task),
                "'{task}' is no task id such as T012"
            );
            let g = gauge::evidence::Gate {
                task: task.clone(),
                gate,
                outcome: gauge::evidence::Outcome::parse(&result)?,
                at: paddock::state::now(),
                note: note.unwrap_or_default(),
                peer,
                snapshot_hash,
            };
            gauge::evidence::record_gate(
                &gauge::collect::Files::of(&layout, &feature).gates(),
                &g,
            )?;
            println!("{feature} {task}: gate {} {}", g.gate, g.outcome.name());
            Ok(0)
        }
        SpecCmd::Adopt { dir, yes } => {
            let project = project(dir)?;
            let mut term = paddock::setup::Terminal::detect();
            let mut ask = |q: &str| {
                if yes {
                    return true;
                }
                match term.as_mut() {
                    Some(t) => paddock::setup::Ask::yes_no(t, q, false),
                    None => {
                        eprintln!("paddock: {q}\nThere is no terminal to answer. Run it again with --yes.");
                        false
                    }
                }
            };
            println!(
                "{}",
                paddock::speckit::adopt::adopt(env, &project, &mut ask)?
            );
            Ok(0)
        }
    }
}

fn spec_layout(env: &HostEnv, project: &std::path::Path) -> Result<paddock::speckit::Layout> {
    paddock::speckit::Layout::resolve(env, project).with_context(|| {
        format!(
            "{} is no Spec Kit project (no .specify/ folder), or [speckit] enabled = false",
            project.display()
        )
    })
}

/// `paddock spec appetite FEATURE [VALUE]`.
fn appetite(
    env: &HostEnv,
    project: &std::path::Path,
    feature: &str,
    value: Option<&str>,
) -> Result<i32> {
    let layout = spec_layout(env, project)?;
    let feature = gauge::collect::feature(&layout, Some(feature))?;
    let path = gauge::collect::Files::of(&layout, &feature).appetite();
    let old = gauge::appetite::load(&path);
    let Some(value) = value else {
        match old {
            Some(a) => println!("{feature}: appetite {}", a.text),
            None => println!("{feature}: no appetite (forecasts only)"),
        }
        return Ok(0);
    };
    paddock::update::refuse_in_sandbox(env, "paddock spec appetite")?;
    match gauge::appetite::parse(value)? {
        None => {
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
            println!("{feature}: no appetite now (forecasts only)");
        }
        Some(secs) => {
            // A new value starts its own warnings; the same value keeps them.
            let warned = old
                .filter(|a| a.secs == secs)
                .map(|a| a.warned)
                .unwrap_or_default();
            let a = gauge::appetite::Appetite {
                secs,
                text: value.trim().to_string(),
                set_at: paddock::state::now(),
                warned,
            };
            gauge::appetite::save(&path, &a)?;
            println!(
                "{feature}: appetite {} ({}). paddock warns at 50, 80 and 100 % and every 25 % after that; it never stops the work. `paddock gauge {feature}` shows the share used.",
                a.text,
                gauge::appetite::human(secs)
            );
        }
    }
    Ok(0)
}
