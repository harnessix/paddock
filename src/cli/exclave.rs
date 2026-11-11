use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::env::HostEnv;

#[derive(Subcommand)]
pub(crate) enum ExclaveCmd {
    /// Serve a peer over standard input and output.
    Serve,
    /// Inspect or remove peer-local project caches.
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Run a command on a peer.
    Run {
        #[arg(long)]
        on: Option<String>,
        /// Select a project root explicitly.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
        #[arg(long, default_value = "registries")]
        network: String,
        #[arg(long)]
        gpu: bool,
        #[arg(long)]
        output: Vec<String>,
        #[arg(long)]
        include: Vec<String>,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum CacheCmd {
    /// List project cache sizes.
    Ls,
    /// Remove a project cache by its listed ID.
    Rm { id: String },
}

pub(crate) fn run(env: &HostEnv, cmd: ExclaveCmd) -> Result<i32> {
    match cmd {
        ExclaveCmd::Cache { cmd } => {
            let config = paddock::exclave::config::ServeConfig::load(env)?;
            let remove = match &cmd {
                CacheCmd::Ls => None,
                CacheCmd::Rm { id } => Some(id.as_str()),
            };
            for (id, bytes) in paddock::exclave::cache::manage(&config.cache_root, remove)? {
                println!("{id} {bytes}");
            }
            Ok(0)
        }
        ExclaveCmd::Serve => {
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            if let Err(error) = paddock::exclave::serve::serve(env, &mut stdin.lock(), &mut stdout)
            {
                paddock::exclave::protocol::write(
                    &mut stdout,
                    &paddock::exclave::protocol::Response::Error {
                        message: format!("{error:#}"),
                    },
                )?;
            }
            Ok(0)
        }
        ExclaveCmd::Run {
            on,
            root,
            network,
            gpu,
            output,
            include,
            command,
        } => {
            if !matches!(network.as_str(), "none" | "registries" | "egress") {
                bail!("invalid exclave network mode");
            }
            let cwd = std::env::current_dir()?.canonicalize()?;
            let project = if let Some(root) = root {
                let root = root.canonicalize()?;
                if !root.is_dir() {
                    bail!("exclave root must be a directory");
                }
                eprintln!("exclave: project root {} (explicit --root)", root.display());
                root
            } else {
                match paddock::hostgit::Layout::discover(&cwd)? {
                    Some(layout) => {
                        let root = layout.work_tree.canonicalize()?;
                        eprintln!("exclave: project root {} (Git worktree)", root.display());
                        root
                    }
                    None => {
                        eprintln!(
                            "exclave: project root {} (current folder; no Git worktree)",
                            cwd.display()
                        );
                        cwd.clone()
                    }
                }
            };
            let relative = cwd
                .strip_prefix(&project)?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid working directory"))?;
            let peers = paddock::exclave::Peers::load(env)?;
            let name = peers.select(on.as_deref())?;
            let config = paddock::manifest::Manifest::load(env, &project)?;
            let hidden: Vec<_> = config
                .merged
                .secrets
                .hide
                .iter()
                .filter_map(|h| {
                    env.expand(h, &project)
                        .strip_prefix(&project)
                        .ok()
                        .map(std::path::Path::to_path_buf)
                })
                .collect();
            let snapshot = paddock::exclave::snapshot::capture(&project, &include, &hidden)?;
            let identity = paddock::exclave::identity::project(env, name, &project)?;
            let mut session =
                match paddock::exclave::client::Session::connect(&peers, name, identity) {
                    Ok(v) => v,
                    Err(e) if e.is::<paddock::exclave::client::Unreachable>() => {
                        if gpu {
                            bail!(
                                "peer {name} is unreachable: {e}; GPU request cannot run locally"
                            );
                        }
                        eprintln!("exclave: peer {name} is unreachable: {e}; running locally");
                        return local(&command);
                    }
                    Err(e) => return Err(e),
                };
            let hash = session.upload(&snapshot)?;
            match session.reserve() {
                Ok(true) => {}
                Ok(false) => bail!("peer {name} is busy"),
                Err(e) => return Err(e.context("peer reservation failed")),
            }
            let command = paddock::exclave::command_in(relative, command)?;
            session.run_with_outputs(
                hash,
                network,
                gpu,
                command,
                output,
                &project,
                &hidden,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            )
        }
    }
}

fn local(command: &[String]) -> Result<i32> {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::null())
        .status()?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}
