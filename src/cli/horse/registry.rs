//! `paddock horse registry`: the signing key, the trust policy, the
//! indexes, and published DNA, on the host. A sandbox never runs these:
//! it asks the broker with `paddock-host horse registry fetch`.
use anyhow::{bail, Context as _, Result};
use clap::Subcommand;
use paddock::horse::registry::{self, host, trust, Reference, Refusal, Resolved, Transport as _};
use paddock::horse::{dna, Store};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub(crate) enum RegistryCmd {
    /// The signing key of this host: `new` makes one, `show` prints its
    /// public key.
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// The keys that may sign for an org.
    Trust {
        #[command(subcommand)]
        cmd: TrustCmd,
    },
    /// The indexes that hold published DNA, searched in order.
    Index {
        #[command(subcommand)]
        cmd: IndexCmd,
    },
    /// List the indexes and what they hold, or the versions of ORG/NAME.
    List {
        #[arg(value_name = "ORG/NAME")]
        name: Option<String>,
    },
    /// Resolve a reference: print its manifest, the trust verdict and the
    /// hash. Nothing is imported. Exits 1 when the trust policy refuses it.
    Resolve {
        reference: String,
        /// Search only this index.
        #[arg(long)]
        index: Option<String>,
    },
    /// Publish the DNA of HORSE as REFERENCE (registry://org/name@version),
    /// signed with the key of this host.
    Publish {
        horse: String,
        reference: String,
        /// The index to publish into. Default: the first index.
        #[arg(long)]
        index: Option<String>,
        /// Publish without a signature.
        #[arg(long)]
        unsigned: bool,
    },
    /// Pull an index that is a git checkout (`git pull --ff-only`). A
    /// plain folder has nothing to pull.
    Sync { index: String },
}

#[derive(Subcommand)]
pub(crate) enum KeyCmd {
    /// Make the signing key. An existing key is never replaced.
    New,
    /// Print the public key.
    Show,
}

#[derive(Subcommand)]
pub(crate) enum TrustCmd {
    /// Trust PUBLIC (64 hex digits) to sign for ORG.
    Add {
        org: String,
        public: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// List the trusted keys.
    List,
}

#[derive(Subcommand)]
pub(crate) enum IndexCmd {
    /// Add an index: a folder, or a git checkout of one.
    Add { name: String, path: PathBuf },
}

pub(crate) fn run(state: &Path, st: &Store, cmd: RegistryCmd) -> Result<i32> {
    match cmd {
        RegistryCmd::Key { cmd: KeyCmd::New } => {
            let path = host::key_path(state);
            if path.exists() {
                bail!(
                    "a signing key exists at {}; it is not replaced (`paddock horse registry key show` prints it)",
                    path.display()
                );
            }
            let k = trust::new_keypair();
            trust::save_keypair(&k, &path)?;
            println!("{}", k.public_hex());
            eprintln!(
                "paddock: the secret key is in {}; share only the public key above",
                path.display()
            );
        }
        RegistryCmd::Key { cmd: KeyCmd::Show } => println!("{}", load_key(state)?.public_hex()),
        RegistryCmd::Trust {
            cmd: TrustCmd::Add { org, public, name },
        } => {
            if host::trust_add(state, &org, &public, name.as_deref())? {
                println!("the key {public} is trusted for {org}");
            } else {
                println!("the key {public} was already trusted for {org}");
            }
        }
        RegistryCmd::Trust {
            cmd: TrustCmd::List,
        } => {
            let t = registry::Trust::load(&host::trust_path(state))?;
            println!("require_signature {}", t.require_signature);
            for k in t.keys {
                println!(
                    "{} {} {}",
                    k.org,
                    k.public,
                    k.name.as_deref().unwrap_or("-")
                );
            }
        }
        RegistryCmd::Index {
            cmd: IndexCmd::Add { name, path },
        } => {
            host::add_index(state, &name, &path)?;
            println!("index {name} at {}", path.display());
        }
        RegistryCmd::List { name } => list(state, name.as_deref())?,
        RegistryCmd::Resolve { reference, index } => {
            let r: Reference = reference.parse()?;
            return Ok(resolve(st, state, &r, index.as_deref()));
        }
        RegistryCmd::Publish {
            horse,
            reference,
            index,
            unsigned,
        } => {
            let r: Reference = reference.parse()?;
            let key = if unsigned {
                None
            } else if host::key_path(state).exists() {
                Some(load_key(state)?)
            } else {
                bail!("run paddock horse registry key new first, or publish with --unsigned");
            };
            let indexes = registry::configured(state)?;
            let target = match &index {
                Some(n) => indexes.iter().find(|i| &i.name == n).with_context(|| {
                    format!("there is no index {n}: `paddock horse registry list` shows them")
                })?,
                None => indexes.first().context("no registry index is set up")?,
            };
            let m = dna::registry::publish(st, &horse, target, &r, key.as_ref())?;
            println!("{}", m.hash);
            println!(
                "published {r} into index {} ({})",
                target.name,
                if m.signature.is_some() {
                    "signed"
                } else {
                    "unsigned"
                }
            );
        }
        RegistryCmd::Sync { index } => match host::sync(state, &index)? {
            host::Synced::Plain => {
                println!("index {index} is a plain folder, not a git checkout: nothing to sync")
            }
            host::Synced::Pulled(out) => {
                print!("{out}");
                println!("index {index} is up to date");
            }
        },
    }
    Ok(0)
}

fn load_key(state: &Path) -> Result<registry::Keypair> {
    let path = host::key_path(state);
    if !path.exists() {
        bail!("there is no signing key: run paddock horse registry key new first");
    }
    trust::load_keypair(&path)
}

fn list(state: &Path, name: Option<&str>) -> Result<()> {
    let indexes = registry::configured(state)?;
    let Some(name) = name else {
        for i in &indexes {
            println!("index {} {}", i.name, i.root.display());
            for (org, n) in i.names()? {
                let vs = i.versions(&org, &n)?;
                println!("  {org}/{n} {}", vs.join(" "));
            }
        }
        return Ok(());
    };
    let Some((org, n)) = name.split_once('/') else {
        bail!("{name:?} is not ORG/NAME");
    };
    for i in &indexes {
        for v in i.versions(org, n)? {
            let Some(m) = i.manifest(org, n, &v)? else {
                continue;
            };
            println!(
                "{} {name}@{v} {} {}",
                i.name,
                m.hash,
                if m.signature.is_some() {
                    "signed"
                } else {
                    "unsigned"
                }
            );
        }
    }
    Ok(())
}

/// Print a resolved reference, or why it was refused. 0 or 1.
fn resolve(st: &Store, state: &Path, r: &Reference, index: Option<&str>) -> i32 {
    match dna::registry::resolve_on_host(st, state, r, index) {
        Ok(got) => {
            print_resolved(&got);
            0
        }
        Err(e) => {
            if let Some(v) = e.downcast_ref::<Refusal>().and_then(|x| x.verdict.as_ref()) {
                println!("reference {r}");
                println!("verdict {}", v.name());
            }
            eprintln!("paddock: {e:#}");
            1
        }
    }
}

fn print_resolved(got: &Resolved) {
    println!("reference {}", dna::registry::pinned(got));
    println!("hash {}", got.manifest.hash);
    match &got.signed_by {
        Some(k) => println!("verdict Trusted ({k})"),
        None => println!("verdict Unsigned"),
    }
    println!("index {}", got.index);
    print!(
        "manifest {}",
        String::from_utf8_lossy(&got.manifest.encode())
    );
}
