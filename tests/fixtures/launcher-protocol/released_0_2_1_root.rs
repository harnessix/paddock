//! Reduced root-parser fixture from tag v0.2.1, commit
//! 5e47e8da2edc20e50c1728fc09a31116cf2866e8, src/cli/mod.rs. The root
//! flags and external-subcommand declaration are copied; built-in
//! subcommands are omitted because they do not affect an unknown root
//! option. Cargo.lock resolves the same clap 4.6.7 as that tag. This is
//! not the released executable or its complete command tree.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "paddock",
    version,
    about = "Run a coding-agent harness with full auto-approval inside an isolated sandbox",
    long_about = None,
    after_help = "Examples:\n  paddock claude              start Claude Code in the current project\n  paddock codex ~/src/app     start Codex in another project\n  paddock explain claude      show every mount, variable and rule, and why\n  paddock doctor --escape     try to escape the sandbox and report the result"
)]
pub struct Cli {
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub quiet: u8,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Model only the old parser-to-external dispatch. The callback is a fake;
/// this fixture cannot start a harness.
pub fn dispatch(
    argv: &[&str],
    mut fake_harness: impl FnMut(&[String]),
) -> Result<(), clap::error::ErrorKind> {
    let cli = Cli::try_parse_from(argv.iter().copied()).map_err(|e| e.kind())?;
    let Cmd::External(args) = cli.cmd;
    fake_harness(&args);
    Ok(())
}
