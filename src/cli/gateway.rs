use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use paddock::env::HostEnv;
use paddock::spec::Network;

#[derive(Subcommand)]
pub(crate) enum GatewayCmd {
    /// Let the system start the executor on the first request: systemd user
    /// units on Linux, a LaunchAgent of launchd on macOS.
    Install,
    /// Turn that off and remove the files that paddock wrote.
    Remove,
    /// How the executor starts.
    Status,
    /// The sandboxed gateway of a gateway agent: `paddock gateway openclaw
    /// start|stop|status|logs`.
    #[command(external_subcommand)]
    Agent(Vec<String>),
}

/// `paddock gateway NAME ACTION`.
#[derive(Parser)]
#[command(name = "paddock gateway NAME")]
struct AgentCli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Run the gateway in a sandbox of its own, as a service of the user:
    /// a systemd user unit on Linux, a LaunchAgent on macOS. It binds to
    /// 127.0.0.1 with a token that paddock makes and keeps.
    Start {
        /// egress (default) or host.
        #[arg(long, short)]
        network: Option<Network>,
        /// auto, or a backend name.
        #[arg(long, short)]
        backend: Option<String>,
        /// The port on 127.0.0.1. Default: the port of the last start, else
        /// the port of the profile, else a free one.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stop the gateway and its unit.
    Stop,
    /// The state, the port, the uptime and the clients.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// The output of the gateway and of its supervisor.
    Logs {
        /// The number of lines.
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: usize,
        /// Print new lines as they come. Ctrl-C ends it.
        #[arg(long, short)]
        follow: bool,
    },
}

pub(crate) fn run(env: &HostEnv, cmd: GatewayCmd) -> Result<i32> {
    use paddock::gatewayd;
    match cmd {
        GatewayCmd::Install => {
            for p in gatewayd::install(env)? {
                println!("wrote {}", p.display());
            }
            let manager = if cfg!(target_os = "macos") {
                "launchd"
            } else {
                "systemd"
            };
            println!("{manager} now starts the gateway executor on the first request. Undo with: paddock gateway remove");
        }
        GatewayCmd::Remove => {
            for p in gatewayd::remove(env)? {
                println!("removed {}", p.display());
            }
            println!("paddock starts the gateway executor itself when a request needs it.");
        }
        GatewayCmd::Status => println!("{}", gatewayd::status_line(env)),
        GatewayCmd::Agent(argv) => return agent(env, argv),
    }
    Ok(0)
}

fn agent(env: &HostEnv, argv: Vec<String>) -> Result<i32> {
    use paddock::agentgw::{control, report, state};
    let Some((name, rest)) = argv.split_first() else {
        bail!("name a gateway agent: paddock gateway openclaw start");
    };
    let known = report::profiles(env);
    let Some((_, host_port)) = known.iter().find(|(n, _)| n == name) else {
        let names: Vec<&str> = known.iter().map(|(n, _)| n.as_str()).collect();
        bail!(
            "unknown gateway subcommand or agent '{name}': use install, remove, status, or an agent with a gateway ({})",
            if names.is_empty() { "none".to_string() } else { names.join(", ") }
        );
    };
    let mut full = vec![format!("paddock gateway {name}")];
    full.extend(rest.iter().cloned());
    let a = AgentCli::try_parse_from(full).unwrap_or_else(|e| e.exit());
    match a.action {
        Action::Start {
            network,
            backend,
            port,
        } => {
            let opts = control::StartOpts {
                network,
                backend,
                port,
            };
            let st = control::start(env, name, &opts)?;
            println!("{}", report::line(&report::of(env, name, *host_port)));
            if let Some(r) = &st.reason {
                println!("{r}");
            }
            println!("Attach with: paddock {name} tui. Stop with: paddock gateway {name} stop");
        }
        Action::Stop => {
            if control::stop(env, name)? {
                println!("stopped the {name} gateway");
            } else {
                println!("no {name} gateway ran");
            }
        }
        Action::Status { json } => {
            let g = report::of(env, name, *host_port);
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "api_version": "paddock.gateway/v1", "gateway": g })
                );
            } else {
                println!("{}", report::line(&g));
                if let Some(r) = &g.reason {
                    println!("  {r}");
                }
                if let Some(e) = &g.last_exit {
                    println!("  last end: {e}");
                }
                if let Some(p) = g.host_gateway {
                    println!("  a gateway that paddock did not start answers on 127.0.0.1:{p}");
                }
                println!("  home: {}", state::home(env, name).display());
            }
        }
        Action::Logs { lines, follow } => {
            let file = state::log_file(env, name);
            print!("{}", control::log_tail(env, name, lines));
            println!();
            if follow {
                follow_file(&file)?;
            }
        }
    }
    Ok(0)
}

/// Print what is added to `file` until Ctrl-C.
fn follow_file(file: &std::path::Path) -> Result<()> {
    use std::io::{Read as _, Seek as _, Write as _};
    let mut pos = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    loop {
        if let Ok(mut f) = std::fs::File::open(file) {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < pos {
                pos = 0;
            }
            if len > pos {
                f.seek(std::io::SeekFrom::Start(pos))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                pos += buf.len() as u64;
                let mut out = std::io::stdout().lock();
                if out.write_all(&buf).and_then(|()| out.flush()).is_err() {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

pub(crate) fn gatewayd(env: &HostEnv, idle: u64) -> Result<i32> {
    paddock::gatewayd::main(env, std::time::Duration::from_secs(idle))
}
