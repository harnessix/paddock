//! `paddock-vm`: the helper that owns the Linux VM of a paddock sandbox on
//! macOS. It needs the `com.apple.security.virtualization` entitlement
//! (packaging/macos/paddock-vm.entitlements).
//!
//! Exit codes: 0 when the guest stops, 1 on an error, 2 on a usage error,
//! 3 after a hard stop.

use clap::Parser;
use paddock::vm::helper::{self, Cli, Cmd};

fn main() {
    // A launcher bundle of `paddock app install` is started by Finder with no
    // arguments of its own. The bundle's target file says what to do.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match std::env::current_exe()
        .ok()
        .and_then(|exe| paddock::macvm::shim::argv_from_bundle(&exe, &argv))
    {
        Some(from_bundle) => {
            let mut whole = vec!["paddock-vm".to_string()];
            whole.extend(from_bundle);
            Cli::parse_from(whole)
        }
        None => Cli::parse(),
    };
    paddock::logging::init(0);
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("paddock-vm: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<i32> {
    match cli.cmd {
        Cmd::Start(args) => {
            let env = paddock::env::HostEnv::detect()?;
            let plan = helper::plan(&args, &env.home)?;
            boot(&plan)
        }
        Cmd::MacvmSetup(args) => {
            let env = paddock::env::HostEnv::detect()?;
            macvm_setup(&env, &args)
        }
        Cmd::App(args) => {
            let plan = helper::plan_app(&args)?;
            app(&plan)
        }
        Cmd::Install(args) => {
            let env = paddock::env::HostEnv::detect()?;
            install(&env, &args)
        }
    }
}

#[cfg(target_os = "macos")]
fn install(env: &paddock::env::HostEnv, args: &helper::InstallArgs) -> anyhow::Result<i32> {
    paddock::macvm::launch::install_main(env, args)
}

#[cfg(not(target_os = "macos"))]
fn install(_env: &paddock::env::HostEnv, _args: &helper::InstallArgs) -> anyhow::Result<i32> {
    anyhow::bail!("the launchers of paddock's macOS guest run only on macOS")
}

#[cfg(target_os = "macos")]
fn macvm_setup(env: &paddock::env::HostEnv, args: &helper::MacvmSetupArgs) -> anyhow::Result<i32> {
    paddock::macvm::vz_mac::setup_main(env, args)
}

#[cfg(not(target_os = "macos"))]
fn macvm_setup(
    _env: &paddock::env::HostEnv,
    _args: &helper::MacvmSetupArgs,
) -> anyhow::Result<i32> {
    anyhow::bail!("the macOS guest of paddock runs only on macOS")
}

#[cfg(target_os = "macos")]
fn app(plan: &helper::AppPlan) -> anyhow::Result<i32> {
    let env = paddock::env::HostEnv::detect()?;
    paddock::macvm::run::run(&env, plan)
}

#[cfg(not(target_os = "macos"))]
fn app(_plan: &helper::AppPlan) -> anyhow::Result<i32> {
    anyhow::bail!("the macOS guest of paddock runs only on macOS")
}

#[cfg(target_os = "macos")]
fn boot(plan: &helper::Plan) -> anyhow::Result<i32> {
    paddock::vm::vz::run(plan)
}

#[cfg(not(target_os = "macos"))]
fn boot(_plan: &helper::Plan) -> anyhow::Result<i32> {
    anyhow::bail!("the paddock VM runs only on macOS")
}
