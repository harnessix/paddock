use anyhow::Result;
use clap::Subcommand;
use paddock::{env::HostEnv, horse::keys::Keyring};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum KeyCmd {
    /// Save a private backup without replacing an existing file.
    Export { path: PathBuf },
    /// Restore a matching key to an empty keyring.
    Import { path: PathBuf },
}

pub(super) fn run(env: &HostEnv, cmd: KeyCmd) -> Result<i32> {
    let keys = Keyring::for_host(env);
    match cmd {
        KeyCmd::Export { path } => keys.export(&path)?,
        KeyCmd::Import { path } => keys.import(&path)?,
    }
    Ok(0)
}
