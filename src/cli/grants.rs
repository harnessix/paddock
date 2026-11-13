//! `paddock app share`, `paddock app network` and `paddock app clipboard`:
//! the live grants of an app of paddock's macOS guest
//! (`paddock::macvm::grants_cli`). They are subcommands of `paddock app`.

use anyhow::Result;
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::macvm::grants_cli;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum GrantsCmd {
    /// The folders that an app of paddock's macOS guest sees: add, remove,
    /// change the mode, list. paddock saves each change in the profile for
    /// the next start. While the app runs, the change also applies at once.
    /// A change that paddock cannot save does not apply. If the save fails
    /// after the change applied, paddock undoes it. If the undo fails too,
    /// the error says so, and the app keeps the change until it stops.
    Share {
        name: String,
        #[command(subcommand)]
        cmd: ShareCmd,
    },
    /// The network mode of an app of paddock's macOS guest: ask (each new
    /// host asks you), allowlist (only the hosts of the profile and
    /// [network] allow) or none. Saved for the next start, and live while
    /// the app runs. none closes the open connections. A save that fails
    /// is handled as for share.
    Network {
        name: String,
        #[arg(value_parser = ["ask", "allowlist", "none"])]
        mode: String,
        #[arg(long)]
        json: bool,
    },
    /// The clipboard of an app of paddock's macOS guest, per direction.
    /// Saved for the next start, and live while the app runs. A save that
    /// fails is handled as for share. Without a flag: print the two
    /// directions.
    Clipboard {
        name: String,
        /// Paste from this Mac into the app.
        #[arg(long, value_parser = ["on", "off"])]
        to_app: Option<String>,
        /// Copy from the app to this Mac.
        #[arg(long, value_parser = ["on", "off"])]
        from_app: Option<String>,
        /// The running session only: without a direction, print its
        /// directions next to the saved ones; with one, change the
        /// session and keep the profile as it is.
        #[arg(long)]
        session: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ShareCmd {
    /// Share a folder, read-only unless --rw.
    Add {
        path: PathBuf,
        /// Read and write.
        #[arg(long)]
        rw: bool,
        /// Share it even when it holds a .env or key file, which the app
        /// can then read.
        #[arg(long)]
        unprotected: bool,
        #[arg(long)]
        json: bool,
    },
    /// Stop sharing a folder. Refused while the app holds files of it open,
    /// unless --force.
    Remove {
        path: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Change the mode of a shared folder: ro or rw.
    Mode {
        path: PathBuf,
        #[arg(value_parser = ["ro", "rw"])]
        mode: String,
        #[arg(long)]
        json: bool,
    },
    /// Keep a folder that only the running session shares (one of `paddock
    /// app run NAME PATH`) for the next starts, with its mode now.
    Keep {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List the folders with their mode, status, open files and last use.
    List {
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(env: &HostEnv, cmd: GrantsCmd) -> Result<i32> {
    match cmd {
        GrantsCmd::Share { name, cmd } => match cmd {
            ShareCmd::Add {
                path,
                rw,
                unprotected,
                json,
            } => grants_cli::share_add(env, &name, &path, rw, unprotected, json),
            ShareCmd::Remove { path, force, json } => {
                grants_cli::share_remove(env, &name, &path, force, json)
            }
            ShareCmd::Mode { path, mode, json } => {
                grants_cli::share_mode(env, &name, &path, mode == "ro", json)
            }
            ShareCmd::List { json } => grants_cli::share_list(env, &name, json),
            ShareCmd::Keep { path, json } => grants_cli::share_keep(env, &name, &path, json),
        },
        GrantsCmd::Network { name, mode, json } => grants_cli::network(env, &name, &mode, json),
        GrantsCmd::Clipboard {
            name,
            to_app,
            from_app,
            session,
            json,
        } if session => match (to_app, from_app) {
            (None, None) => grants_cli::clipboard_show(env, &name, json),
            (to_app, from_app) => grants_cli::clipboard_session(
                env,
                &name,
                to_app.map(|w| w == "on"),
                from_app.map(|w| w == "on"),
                json,
            ),
        },
        GrantsCmd::Clipboard {
            name,
            to_app,
            from_app,
            json,
            ..
        } => grants_cli::clipboard(
            env,
            &name,
            to_app.map(|w| w == "on"),
            from_app.map(|w| w == "on"),
            json,
        ),
    }
}
