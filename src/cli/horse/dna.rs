//! `paddock horse dna`: show, set and list the DNA of a horse.
use anyhow::{Context as _, Result};
use clap::Subcommand;
use paddock::horse::{dna, Hash, Store};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub(crate) enum DnaCmd {
    /// Print the DNA of a horse as TOML.
    Show {
        horse: String,
        /// An earlier DNA of the horse, by its hash (see `history`).
        #[arg(long, value_name = "HASH")]
        at: Option<String>,
    },
    /// Set the DNA of a horse from a TOML file, or restore an earlier DNA
    /// by its hash. Each running session gets the new DNA at its next safe
    /// point.
    Set {
        horse: String,
        /// A DNA TOML file, or the hash of an earlier DNA.
        source: String,
    },
    /// List the DNA events of a horse, root first, with their hashes.
    History { horse: String },
    /// Set the DNA of a horse back to an earlier hash (see `history`).
    /// The history stays, and running sessions get the DNA as one entry.
    Revert { horse: String, hash: String },
    /// Graft published DNA (registry://org/name@version) into a horse. The
    /// trust policy and the compatibility check come first.
    Use {
        horse: String,
        reference: String,
        /// Search only this index.
        #[arg(long)]
        index: Option<String>,
    },
}

pub(crate) fn run(state: &Path, st: &Store, cmd: DnaCmd) -> Result<i32> {
    match cmd {
        DnaCmd::Use {
            horse,
            reference,
            index,
        } => {
            let r: paddock::horse::registry::Reference = reference.parse()?;
            let got = dna::registry::resolve_on_host(st, state, &r, index.as_deref())?;
            let before = st.horse(&horse)?.dna;
            let to = dna::registry::use_in(st, &horse, &got)?;
            if to == before {
                println!(
                    "{} adds nothing to the DNA of {horse}",
                    dna::registry::pinned(&got)
                );
            } else {
                println!("{} grafted into {horse}: {to}", dna::registry::pinned(&got));
            }
        }
        DnaCmd::Show { horse, at } => {
            let h = st.horse(&horse)?;
            let hash = match at {
                Some(a) => Hash::parse(&a)?,
                None => h.dna,
            };
            print!("{}", dna::get(st, &hash)?.encode());
        }
        DnaCmd::Set { horse, source } => {
            let to = match Hash::parse(&source) {
                Ok(hash) if !PathBuf::from(&source).exists() => {
                    dna::restore(st, &horse, &hash)?;
                    Some(hash)
                }
                _ => {
                    let text = std::fs::read_to_string(&source)
                        .with_context(|| format!("cannot read {source}"))?;
                    dna::hot_load(st, &horse, &text)?
                }
            };
            match to {
                Some(h) => println!("the DNA of {horse} is now {h}"),
                None => println!("the DNA of {horse} did not change"),
            }
        }
        DnaCmd::Revert { horse, hash } => {
            let to = Hash::parse(&hash)?;
            dna::explain::revert(st, &horse, &to)?;
            println!("the DNA of {horse} is now {to}");
        }
        DnaCmd::History { horse } => {
            for (hash, e) in dna::history(st, &horse)? {
                let s = |k: &str| e[k].as_str().unwrap_or("-").to_string();
                let mut line = format!(
                    "{} {} from {} to {} at {}",
                    hash,
                    s("type"),
                    s("from_hash"),
                    s("to_hash"),
                    e["at_ms"]
                );
                for k in ["parent_horse", "child_horse", "donor_horse", "compat"] {
                    if let Some(v) = e[k].as_str() {
                        line.push_str(&format!(" {k} {v}"));
                    }
                }
                println!("{line}");
            }
        }
    }
    Ok(0)
}
