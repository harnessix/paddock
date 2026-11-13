use anyhow::Result;
use clap::Subcommand;
use paddock::credentials::keys;
use paddock::env::HostEnv;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum CredentialsCmd {
    /// List the registered credentials.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Stop a running sandbox from using a credential through the gateway or
    /// the ssh agent channel.
    /// Without NAME: every one. Its next request fails; it keeps running.
    Revoke { id: String, name: Option<String> },
    /// Add a key once: the value goes into the keychain (a hidden prompt,
    /// or the standard input), never into a file or an argument. A known
    /// name selects a preset.
    Add {
        /// The variable name, such as OPENAI_API_KEY.
        name: String,
        /// anthropic, openai or github. Default: from the name or the value.
        #[arg(long)]
        preset: Option<String>,
        /// Hand the value to the agent, which can then read it. Default:
        /// keep it on this computer; the agent gets a stand-in.
        #[arg(long)]
        deliver: bool,
        /// The host of the service, for a key without a preset that stays
        /// on this computer (repeatable).
        #[arg(long)]
        host: Vec<String>,
        /// The agents that may use it (repeatable). Default: every agent.
        #[arg(long)]
        agent: Vec<String>,
        /// A project where it passes without a question (repeatable).
        #[arg(long, conflicts_with = "any_project")]
        project: Vec<PathBuf>,
        /// Pass it in any project without a question.
        #[arg(long)]
        any_project: bool,
    },
    /// Remove a key: its table, its grants, and its keychain item.
    Remove { name: String },
    /// Let an agent use a key; with --project, without a question there.
    Grant {
        name: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Take a grant back. It applies at the next start; to stop a
    /// running agent now: paddock credentials revoke.
    Ungrant {
        name: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Who used which key: agent, project, count and last use. Never a value.
    Log {
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// The value of a new key: a hidden prompt in a terminal, else the
/// standard input. One final newline is not part of it.
fn read_value() -> Result<paddock::credentials::secret::Secret> {
    use std::io::{IsTerminal as _, Read as _};
    let raw = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Value (hidden): ")?.into_bytes()
    } else {
        let mut v = Vec::new();
        std::io::stdin()
            .take(paddock::tack::store::MAX_VALUE as u64 + 1)
            .read_to_end(&mut v)?;
        if v.len() > paddock::tack::store::MAX_VALUE {
            anyhow::bail!(
                "the value has more than {} bytes",
                paddock::tack::store::MAX_VALUE
            );
        }
        v
    };
    let mut raw = raw;
    if raw.last() == Some(&b'\n') {
        raw.pop();
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
    }
    Ok(paddock::credentials::secret::Secret::new(raw))
}

pub(crate) fn run(env: &HostEnv, cmd: CredentialsCmd) -> Result<i32> {
    match cmd {
        CredentialsCmd::Add {
            name,
            preset,
            deliver,
            host,
            agent,
            project,
            any_project,
        } => {
            let mut projects = Vec::new();
            for p in &project {
                let p = paddock::build::resolve_project(Some(p))?;
                projects.push(paddock::envset::project_key(env, &p));
            }
            if any_project {
                projects.push("*".to_string());
            }
            let mut k = keys::NewKey {
                name: name.clone(),
                deliver,
                preset: preset.clone(),
                hosts: host,
                agents: agent,
                projects,
            };
            // The table first, so a wrong flag stops before the prompt.
            if k.preset.is_none() {
                k.preset = keys::preset_for(&name, None).map(str::to_string);
            }
            if !paddock::credentials::load(env)?.contains_key(&name) {
                if let Err(e) = keys::table(&k) {
                    if k.preset.is_some() || deliver || !k.hosts.is_empty() {
                        return Err(e);
                    }
                }
            }
            let value = read_value()?;
            if k.preset.is_none() {
                k.preset = keys::preset_for(&name, Some(value.as_bytes())).map(str::to_string);
            }
            let new = keys::add(env, &k, &value)?;
            println!(
                "{name} is in the keychain{}. {}",
                if new {
                    if deliver {
                        ". It is handed to the agent: the agent can read it, and you cannot take it back during a run"
                    } else {
                        ". It stays on this computer: the agent gets a stand-in, and paddock adds the real key to each request"
                    }
                } else {
                    ", with the new value; its grants did not change"
                },
                paddock::envset::NEXT_START
            );
            Ok(0)
        }
        CredentialsCmd::Remove { name } => {
            keys::remove(env, &name)?;
            println!(
                "{name} is removed: its table, its grants and its keychain item. Running agents lose it at their next request through the gateway."
            );
            Ok(0)
        }
        CredentialsCmd::Grant {
            name,
            agent,
            project,
        } => {
            let p = match project {
                Some(p) => Some(paddock::build::resolve_project(Some(&p))?),
                None => None,
            };
            println!("{}", keys::grant_cmd(env, &name, &agent, p.as_deref())?);
            Ok(0)
        }
        CredentialsCmd::Ungrant {
            name,
            agent,
            project,
        } => {
            let p = match project {
                Some(p) => Some(paddock::build::resolve_project(Some(&p))?),
                None => None,
            };
            println!("{}", keys::ungrant_cmd(env, &name, &agent, p.as_deref())?);
            Ok(0)
        }
        CredentialsCmd::Log { name, json } => {
            if let Some(n) = &name {
                if !paddock::credentials::load(env)?.contains_key(n) {
                    anyhow::bail!("there is no key {n} in [credentials]");
                }
            }
            let rows = keys::log(env, name.as_deref());
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "api_version": keys::LOG_API, "uses": rows,
                    }))?
                );
                return Ok(0);
            }
            if rows.is_empty() {
                println!("Not used yet.");
            }
            let now = paddock::state::now();
            for r in &rows {
                println!(
                    "{:<24} {:<10} {} {} in {} runs, last {}\n{:<24} {}",
                    paddock::envset::cut(&r.name, 24),
                    paddock::envset::cut(&r.agent, 10),
                    r.count,
                    r.unit,
                    r.runs,
                    paddock::envset::ago(now, r.last),
                    "",
                    r.project.display()
                );
            }
            Ok(0)
        }
        CredentialsCmd::Revoke { id, name } => {
            let id = paddock::ids::resolve(env, &id)?;
            let names = paddock::credentials::revoke(env, &id, name.as_deref())?;
            println!(
                "Revoked for {id}: {}. The gateway refuses their next request with 403, the ssh agent channel signs no more, and their secret files are gone.",
                names.join(", ")
            );
            Ok(0)
        }
        CredentialsCmd::List { json } => {
            let rows = paddock::credentials::rows(env)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "api_version": "paddock.credentials/v1", "credentials": rows,
                    }))?
                );
                return Ok(0);
            }
            if rows.is_empty() {
                println!(
                    "No credentials are registered. Add [credentials.NAME] to {}.",
                    env.paddock_config().join("config.toml").display()
                );
            }
            for r in &rows {
                println!(
                    "{:<24} {:<9} {:<28} {}{}",
                    r.name,
                    r.via,
                    r.source,
                    if !r.available {
                        "not available yet".to_string()
                    } else if !r.ready {
                        "source missing".to_string()
                    } else {
                        "ready".to_string()
                    },
                    match r.last_delivered {
                        Some(t) => format!(
                            ", delivered {} times, last {}",
                            r.delivered,
                            paddock::storage::date(t)
                        ),
                        None => String::new(),
                    },
                );
                if r.used > 0 {
                    if r.via == "agent" {
                        println!("{:<24} {} ssh signatures allowed", "", r.used);
                    } else if r.deliver_as == Some("file") {
                        println!("{:<24} {} reads of the secret file", "", r.used);
                    } else {
                        println!("{:<24} {} requests through the gateway", "", r.used);
                    }
                }
            }
            Ok(0)
        }
    }
}
