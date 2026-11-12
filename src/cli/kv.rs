use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use paddock::env::HostEnv;
use std::path::PathBuf;

/// Which scope a `paddock kv` command reads. Without a flag: the pastures
/// of the project of the current folder.
#[derive(Args)]
pub(crate) struct KvScope {
    /// The project folder (default: the current folder).
    #[arg(long, conflicts_with_all = ["user", "sandbox", "tree", "ns"])]
    project: Option<PathBuf>,
    /// A pasture of the project: a name from [pastures] of the user
    /// configuration, or solo-<sandbox id> for a sandbox started without one.
    #[arg(long, conflicts_with_all = ["user", "sandbox", "tree", "legacy", "ns"])]
    pasture: Option<String>,
    /// The project scope that all sandboxes of the project shared in older
    /// paddock versions. Sandboxes no longer read it.
    #[arg(long, conflicts_with_all = ["user", "sandbox", "tree", "ns"])]
    legacy: bool,
    /// The scope that all projects share.
    #[arg(long, conflicts_with_all = ["sandbox", "tree", "ns"])]
    user: bool,
    /// The self scope of one sandbox, by its id or short id.
    #[arg(long, conflicts_with_all = ["tree", "ns"])]
    sandbox: Option<String>,
    /// The tree scope of a spawn tree, by the id or short id of its root sandbox.
    #[arg(long, conflicts_with = "ns")]
    tree: Option<String>,
    /// A namespace of [kv.namespaces] in the user configuration, by name.
    /// The host is the user: it needs no membership, and its writes record
    /// the writer `user`.
    #[arg(long, value_name = "NAME")]
    ns: Option<String>,
}

/// `--as NAME`: a principal from [kv.principals] of the user configuration.
#[derive(Args)]
pub(crate) struct KvAs {
    /// Act as this principal of [kv.principals]; needs --pasture.
    #[arg(
        long = "as",
        id = "as_",
        value_name = "NAME",
        requires = "pasture",
        conflicts_with = "ns"
    )]
    as_: Option<String>,
}

#[derive(Args)]
pub(crate) struct KvAsRequired {
    /// The principal of [kv.principals] to act as; needs --pasture. With
    /// --ns instead, the user on the host writes.
    #[arg(
        long = "as",
        id = "as_",
        value_name = "NAME",
        requires = "pasture",
        conflicts_with = "ns",
        required_unless_present = "ns"
    )]
    as_: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum KvCmd {
    /// The pastures of every project, the user scope and the data of older
    /// versions: key counts and members, never a value. The bar panel
    /// reads it.
    Overview {
        #[arg(long)]
        json: bool,
    },
    /// List keys, with the sandbox that wrote each one.
    List {
        prefix: Option<String>,
        #[command(flatten)]
        scope: KvScope,
        #[arg(long)]
        json: bool,
    },
    /// Print one value.
    Get {
        key: String,
        #[command(flatten)]
        scope: KvScope,
        #[arg(long)]
        json: bool,
    },
    /// Delete one key. With --as: as that principal, only when the version
    /// matches with --version.
    Delete {
        key: String,
        #[command(flatten)]
        scope: KvScope,
        #[command(flatten)]
        principal: KvAs,
        #[arg(long, requires = "as_")]
        version: Option<u64>,
    },
    /// Free the lease lease:NAME, whatever sandbox holds it. With --as: only
    /// a lease of that principal.
    Release {
        name: String,
        #[command(flatten)]
        scope: KvScope,
        #[command(flatten)]
        principal: KvAs,
    },
    /// Write a key of a pasture as a principal of [kv.principals], or of a
    /// namespace (--ns) as the user.
    Put {
        key: String,
        /// The value (default: read from stdin).
        value: Option<String>,
        #[command(flatten)]
        scope: KvScope,
        #[command(flatten)]
        principal: KvAsRequired,
        /// Seconds until the key expires.
        #[arg(long)]
        ttl: Option<u64>,
        /// The version the key must have (0: it must not exist).
        #[arg(long)]
        version: Option<u64>,
    },
    /// Wait for a change of a key or a prefix of a pasture as a principal, or
    /// of a namespace (--ns).
    Watch {
        key: Option<String>,
        #[command(flatten)]
        scope: KvScope,
        #[command(flatten)]
        principal: KvAsRequired,
        #[arg(long, conflicts_with = "key", required_unless_present = "key")]
        prefix: Option<String>,
        /// The version the caller knows.
        #[arg(long)]
        version: Option<u64>,
        /// Seconds to wait (default 60).
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Take or renew the lease lease:NAME of a pasture as a principal, or of
    /// a namespace (--ns) as the user.
    Claim {
        name: String,
        #[command(flatten)]
        scope: KvScope,
        #[command(flatten)]
        principal: KvAsRequired,
        /// Seconds until the lease expires.
        #[arg(long)]
        ttl: u64,
    },
}

/// A request of a principal (`paddock kv ... --as NAME`).
fn kv_request(
    op: &str,
    key: Option<String>,
    f: impl FnOnce(&mut paddock::kv::Request),
) -> paddock::kv::Request {
    let mut r = paddock::kv::Request {
        v: paddock::kv::PROTOCOL,
        op: op.into(),
        key,
        ..Default::default()
    };
    f(&mut r);
    r
}

impl KvScope {
    /// A write or a watch: as a principal, or as the user in a namespace.
    fn call(&self, env: &HostEnv, who: Option<&str>, req: paddock::kv::Request) -> Result<i32> {
        match (who, &self.ns) {
            (Some(who), _) => self.principal(env, who, req),
            (None, Some(name)) => paddock::kv::host::in_namespace(env, name, req),
            (None, None) => {
                bail!("name a principal with --as NAME --pasture P, or a namespace with --ns NAME")
            }
        }
    }

    fn principal(&self, env: &HostEnv, who: &str, req: paddock::kv::Request) -> Result<i32> {
        let Some(pasture) = &self.pasture else {
            bail!("--as needs --pasture NAME");
        };
        let project = paddock::kv::host::project_of(self.project.as_deref())?;
        paddock::kv::principal::cli(env, who, &project, &pasture_name(env, pasture)?, req)
    }

    fn target(&self, env: &HostEnv) -> Result<paddock::kv::host::Target> {
        use paddock::kv::host::{project_of, Target};
        use paddock::kv::shard::Ns;
        Ok(if let Some(name) = &self.ns {
            paddock::kv::namespace::check_name(name)?;
            Target::One(Ns::Named(name.clone()))
        } else if self.user {
            Target::One(Ns::User)
        } else if let Some(id) = &self.sandbox {
            Target::One(Ns::Own(paddock::ids::resolve(env, id)?))
        } else if let Some(root) = &self.tree {
            Target::One(Ns::Tree(paddock::ids::resolve(env, root)?))
        } else {
            let project = project_of(self.project.as_deref())?;
            if let Some(name) = &self.pasture {
                Target::One(Ns::pasture(&project, &pasture_name(env, name)?)?)
            } else if self.legacy {
                Target::One(Ns::Legacy(project))
            } else {
                Target::Project(project)
            }
        })
    }
}

/// `solo-ID` with the full sandbox id, so `solo-1a2b` works too.
fn pasture_name(env: &HostEnv, name: &str) -> Result<String> {
    let solo = paddock::pasture::SOLO_PREFIX;
    let Some(id) = name.strip_prefix(solo) else {
        return Ok(name.to_owned());
    };
    let known = paddock::ids::known(env);
    Ok(match paddock::ids::find(id, &known, |s| s.as_str())? {
        Some(full) => format!("{solo}{full}"),
        None => name.to_owned(),
    })
}

pub(crate) fn run(env: &HostEnv, cmd: KvCmd) -> Result<i32> {
    use paddock::kv::host;
    match cmd {
        KvCmd::Overview { json } => {
            let v = host::overview(env)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(0);
            }
            let rows = v["pastures"].as_array().cloned().unwrap_or_default();
            if rows.is_empty() {
                println!("No pasture holds keys or has a running member.");
            }
            for p in &rows {
                println!(
                    "{:<32} {:>5} keys  {}",
                    p["name"].as_str().unwrap_or_default(),
                    p["keys"],
                    p["project"].as_str().unwrap_or_default()
                );
            }
            println!("user scope: {} keys", v["user"]["keys"]);
            for n in v["namespaces"].as_array().into_iter().flatten() {
                println!(
                    "ns:{}: {} keys, {} leases (paddock kv list --ns {})",
                    n["name"].as_str().unwrap_or_default(),
                    n["keys"],
                    n["leases"],
                    n["name"].as_str().unwrap_or_default()
                );
            }
            for l in v["legacy"].as_array().into_iter().flatten() {
                println!(
                    "older paddock: {} keys of {} (paddock kv list --legacy)",
                    l["keys"],
                    l["project"].as_str().unwrap_or_default()
                );
            }
            Ok(0)
        }
        KvCmd::List {
            prefix,
            scope,
            json,
        } => host::list(
            env,
            &scope.target(env)?,
            prefix.as_deref().unwrap_or(""),
            json,
        ),
        KvCmd::Get { key, scope, json } => host::get(env, &scope.target(env)?, &key, json),
        KvCmd::Delete {
            key,
            scope,
            principal: KvAs { as_: Some(who) },
            version,
        } => scope.principal(
            env,
            &who,
            kv_request("delete", Some(key), |r| r.version = version),
        ),
        KvCmd::Delete { key, scope, .. } => host::delete(env, &scope.target(env)?, &key),
        KvCmd::Release {
            name,
            scope,
            principal: KvAs { as_: Some(who) },
        } => scope.principal(env, &who, kv_request("release", Some(name), |_| {})),
        KvCmd::Release { name, scope, .. } => host::release(env, &scope.target(env)?, &name),
        KvCmd::Put {
            key,
            value,
            scope,
            principal,
            ttl,
            version,
        } => {
            let value = match value {
                Some(v) => v,
                None => {
                    let mut v = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut v)?;
                    v
                }
            };
            scope.call(
                env,
                principal.as_.as_deref(),
                kv_request("put", Some(key), |r| {
                    r.value = Some(value);
                    r.ttl = ttl;
                    r.version = version;
                }),
            )
        }
        KvCmd::Watch {
            key,
            scope,
            principal,
            prefix,
            version,
            timeout,
        } => scope.call(
            env,
            principal.as_.as_deref(),
            kv_request("watch", key, |r| {
                r.prefix = prefix;
                r.version = version;
                r.timeout = timeout;
            }),
        ),
        KvCmd::Claim {
            name,
            scope,
            principal,
            ttl,
        } => scope.call(
            env,
            principal.as_.as_deref(),
            kv_request("claim", Some(name), |r| r.ttl = Some(ttl)),
        ),
    }
}
