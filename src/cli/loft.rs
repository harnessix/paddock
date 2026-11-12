use anyhow::{Context as _, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::loft::{self, host, Loft};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum LoftCmd {
    /// Make the loft of the current repository and turn it on. `.paddock` in the project becomes a link to its notes.
    Init {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// The state of the loft, the private files that wait in the project, and the changes since the last commit.
    Status {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// The history of the loft.
    Log {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(short = 'n', long, default_value_t = 20)]
        lines: usize,
    },
    /// The changes since the last commit of the loft.
    Diff {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Sync the real `.paddock` folder of this work tree with the loft now (`[loft] mode = "sync"`).
    Sync {
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Keep syncing each time the folder or the loft changes, until Ctrl-C.
        #[arg(long)]
        watch: bool,
    },
    /// Show the loft mode, or set `[loft] mode` in the user configuration and switch this work tree: `link` or `sync`.
    Mode {
        #[arg(long)]
        dir: Option<PathBuf>,
        /// `link`: `.paddock` is a link to the loft's notes. `sync`: a real folder that paddock syncs.
        mode: Option<String>,
    },
    /// Commit the changes of the loft now. In sync mode, it syncs this work tree first.
    Commit {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(short = 'm', long, default_value = "Commit by hand")]
        message: String,
    },
    /// List the loft trash: the notes that a sync deleted, with the time of the delete.
    Trash {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Bring a note (or a folder of notes) back from the loft trash to the loft and to the work trees.
    Restore {
        #[arg(long)]
        dir: Option<PathBuf>,
        /// The path in `.paddock`, for example `plan.md` or `notes/`. If the loft has a newer file there, the restored one becomes a conflict copy.
        path: String,
    },
    /// Take the `.paddock` link away and turn the loft off. Its files and history stay.
    Remove {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Move the private files of the repository (instruction files, design files, Spec Kit folders) into the loft, after a question.
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

fn made(env: &HostEnv, dir: Option<PathBuf>) -> Result<Loft> {
    let p = project(dir)?;
    let l = Loft::of(env, &p);
    anyhow::ensure!(
        l.initialized(),
        "{} has no loft: `paddock loft init` makes it",
        p.display()
    );
    Ok(l)
}

pub(crate) fn run(env: &HostEnv, cmd: LoftCmd) -> Result<i32> {
    paddock::update::refuse_in_sandbox(env, "paddock loft")?;
    match cmd {
        LoftCmd::Init { dir } => println!("{}", host::init(env, &project(dir)?)?),
        LoftCmd::Status { dir } => print!("{}", host::status(env, &project(dir)?)?),
        LoftCmd::Log { dir, lines } => print!("{}", loft::git::log(&made(env, dir)?, lines)?),
        LoftCmd::Diff { dir } => print!("{}", loft::git::diff(&made(env, dir)?)?),
        LoftCmd::Sync { dir, watch } => {
            let wt = loft::sync::worktree_of(&project(dir)?);
            let l = Loft::of(env, &wt);
            let who = loft::sync::host_name();
            if !loft::sync::synced(&l, &wt) {
                match loft::sync::apply_mode(env, &wt, &who)? {
                    Some(msg) => println!("{msg}"),
                    None => anyhow::bail!(
                        "{}/.paddock is not in sync mode: `paddock loft mode sync` switches it",
                        wt.display()
                    ),
                }
            }
            let r = loft::sync::sync(&l, &wt, &who)?;
            if r.lines().is_empty() {
                println!("The folder and the loft are the same.");
            }
            for line in r.lines() {
                println!("{line}");
            }
            if watch {
                println!(
                    "Watching {}/.paddock and the loft. Ctrl-C stops.",
                    wt.display()
                );
                let stop = std::sync::atomic::AtomicBool::new(false);
                loft::sync::watch_loop(&l, &wt, &who, &stop, loft::sync::WATCH_EVERY, true);
            }
        }
        LoftCmd::Mode { dir, mode } => {
            let wt = loft::sync::worktree_of(&project(dir)?);
            if let Some(m) = mode {
                let m = match m.as_str() {
                    "link" => loft::sync::Mode::Link,
                    "sync" => loft::sync::Mode::Sync,
                    _ => anyhow::bail!("the loft mode is `link` or `sync`"),
                };
                host::set_mode(env, m)?;
                if let Some(p) = loft::sync::project_mode(&wt).filter(|p| *p != m) {
                    println!(
                        "[loft] mode = \"{}\" in {}/paddock.toml wins over the user configuration.",
                        p.name(),
                        wt.display()
                    );
                }
                match loft::sync::apply_mode(env, &wt, &loft::sync::host_name())? {
                    Some(msg) => println!("{msg}"),
                    None => println!(
                        "{}/.paddock is already in {} mode.",
                        wt.display(),
                        loft::sync::mode(env, &wt).name()
                    ),
                }
            } else {
                let l = Loft::of(env, &wt);
                let now = if loft::sync::synced(&l, &wt) {
                    "a real folder, synced with the loft"
                } else if host::linked(env, &wt) {
                    "a link to the loft's notes"
                } else {
                    "not connected to the loft yet"
                };
                println!(
                    "mode {} ({}): {}/.paddock is {now}",
                    loft::sync::mode(env, &wt).name(),
                    if loft::sync::project_mode(&wt).is_some() {
                        "from paddock.toml"
                    } else {
                        "from the user configuration"
                    },
                    wt.display()
                );
            }
        }
        LoftCmd::Commit { dir, message } => {
            let wt = loft::sync::worktree_of(&project(dir.clone())?);
            loft::sync::sync_if_synced(env, &wt, &loft::sync::host_name());
            if loft::git::commit(&made(env, dir)?, &message)? {
                println!("Committed the loft.");
            } else {
                println!("Nothing changed since the last commit.");
            }
        }
        LoftCmd::Trash { dir } => {
            let wt = loft::sync::worktree_of(&project(dir)?);
            print!("{}", loft::trash::render(env, &Loft::of(env, &wt)));
        }
        LoftCmd::Restore { dir, path } => {
            let wt = loft::sync::worktree_of(&project(dir)?);
            let l = Loft::of(env, &wt);
            for line in loft::trash::restore(&l, Some(&wt), &path, &loft::sync::host_name())? {
                println!("{line}");
            }
        }
        LoftCmd::Remove { dir } => println!("{}", host::remove(env, &project(dir)?)?),
        LoftCmd::Adopt { dir, yes } => {
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
            println!("{}", host::adopt(env, &project, &mut ask)?);
        }
    }
    Ok(0)
}
