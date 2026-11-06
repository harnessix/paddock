use anyhow::Result;
use clap::Subcommand;
use paddock::env::HostEnv;

#[derive(Subcommand)]
pub(crate) enum GatewayCmd {
    /// Let the system start the executor on the first request: systemd user
    /// units on Linux, a LaunchAgent of launchd on macOS.
    Install,
    /// Turn that off and remove the files that paddock wrote.
    Remove,
    /// How the executor starts.
    Status,
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
    }
    Ok(0)
}

pub(crate) fn gatewayd(env: &HostEnv, idle: u64) -> Result<i32> {
    paddock::gatewayd::main(env, std::time::Duration::from_secs(idle))
}
