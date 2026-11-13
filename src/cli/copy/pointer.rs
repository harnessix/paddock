//! `paddock pointer`: the pointer folder of a project and its store.

use super::{confirm, out, project_of};
use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::copy::pointer::{self, State};
use paddock::copy::{json, Config};
use paddock::env::HostEnv;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum PointerCmd {
    /// Show whether the pointer and the store agree, and whether the disk
    /// of the store is encrypted.
    Status {
        /// The project folder. Without it: the current one.
        project: Option<PathBuf>,
        /// Print JSON (api_version paddock.pointer/v1).
        #[arg(long)]
        json: bool,
    },
    /// Make the pointer and an empty store for a folder that has none.
    Create { project: Option<PathBuf> },
    /// Make a new, empty store for this folder: for a copy of a project
    /// folder, or when the store is not on this machine.
    New {
        project: Option<PathBuf>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Give the store to this folder: after a restore from a backup, or a
    /// move to another disk.
    Link {
        project: Option<PathBuf>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

pub(super) fn pointer_cmd(env: &HostEnv, cfg: &Config, cmd: PointerCmd) -> Result<i32> {
    match cmd {
        PointerCmd::Status {
            project,
            json: as_json,
        } => {
            let project = project_of(project.as_deref())?;
            let doc = json::pointer_doc(env, cfg, &project)?;
            if as_json {
                out(&doc);
                return Ok(0);
            }
            let ok = doc["ok"] == true;
            println!(
                "{} {:<10} {}",
                if ok { '●' } else { '▲' },
                doc["state"].as_str().unwrap_or(""),
                doc["text"].as_str().unwrap_or("")
            );
            if let Some(s) = doc["store"].as_object() {
                println!(
                    "Store      {}  ·  {} copies  ·  {} backups",
                    s["dir"].as_str().unwrap_or(""),
                    s["copies"],
                    s["backups"]
                );
                println!(
                    "Size       {} of its own",
                    paddock::storage::human(s["size"]["bytes"].as_u64().unwrap_or(0))
                );
                if !ok {
                    println!("Belongs to {}", s["original"].as_str().unwrap_or(""));
                }
            }
            let enc: paddock::copy::disk::Encryption = match (
                doc["encryption"]["state"].as_str(),
                doc["encryption"]["kind"].as_str(),
            ) {
                (Some("encrypted"), Some(k)) => {
                    paddock::copy::disk::Encryption::Encrypted(k.into())
                }
                (Some("not_encrypted"), _) => paddock::copy::disk::Encryption::NotEncrypted,
                _ => paddock::copy::disk::Encryption::Unknown,
            };
            println!("Disk       {}", enc.word());
            match doc["action"].as_str() {
                Some(a) => println!("Run `{a}` in the folder."),
                None => println!("A copy of this folder carries only the pointer, not the files."),
            }
            Ok(if ok { 0 } else { 1 })
        }
        PointerCmd::Create { project } => {
            let project = project_of(project.as_deref())?;
            let st = pointer::status(env, cfg, &project, true)?;
            if st.state != State::NoPointer {
                bail!("the folder has a pointer already: {}", st.state.text());
            }
            let (store, _) = pointer::new_store(env, cfg, &project)?;
            println!(
                "Made {}/.paddock/pointer.json and the store {}.",
                project.display(),
                store.dir.display()
            );
            Ok(0)
        }
        PointerCmd::New { project, yes } => {
            let project = project_of(project.as_deref())?;
            let st = pointer::status(env, cfg, &project, true)?;
            if st.state.ok() {
                bail!("the folder has its store already. A new one would leave its copies and backups behind");
            }
            let q = format!(
                "{} Make a new, empty store for {}?",
                st.state.text(),
                project.display()
            );
            if !confirm(&q, yes)? {
                return Ok(1);
            }
            let (store, _) = pointer::new_store(env, cfg, &project)?;
            println!("Made the store {}.", store.dir.display());
            Ok(0)
        }
        PointerCmd::Link { project, yes } => {
            let project = project_of(project.as_deref())?;
            let st = pointer::status(env, cfg, &project, true)?;
            let (State::Restored, Some(mut store)) = (st.state, st.store) else {
                bail!(
                    "{} `paddock pointer link` is for a folder that came back from a backup or moved to another disk",
                    st.state.text()
                );
            };
            let q = format!(
                "The store belonged to {}, which is gone. Link it with its backups to {}?",
                store.meta.path.display(),
                project.display()
            );
            if !confirm(&q, yes)? {
                return Ok(1);
            }
            pointer::link(&mut store, &project)?;
            println!("Linked the store {}.", store.dir.display());
            Ok(0)
        }
    }
}
