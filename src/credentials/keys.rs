//! Keys (ENV1): a value added once, kept in the keychain, and granted to
//! named agents in named projects.
//!
//! - The store: `from = "store"` reads the item NAME of the service
//!   `paddock-credentials` in the login keychain (macOS, `security`) or
//!   the Secret Service (Linux, `secret-tool`). paddock writes the value
//!   on the standard input of the tool, never in an argument and never in
//!   a file. Without a keychain, paddock refuses (`NO_KEYCHAIN`).
//! - Grants: `harnesses` names the agents, `projects` the projects (`*`:
//!   any). Elsewhere the first start of an agent in a project asks once:
//!   This run, Always in this project, or No. "Always" is a standing
//!   grant in the state folder, which a sandbox cannot see.
//! - Uses: the audit log of host requests has a line for each use, with
//!   the run, the agent and the project, never the value. `log` adds them
//!   up for `paddock credentials log`.

use super::{Credential, Kind, Source, Via, CAPABILITY};
use crate::credentials::secret::Secret;
use crate::env::HostEnv;
use crate::tack::store::{Keyring, Store as _, StoreError};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The service attribute of the keychain items of `[credentials]`.
pub const SERVICE: &str = "paddock-credentials";

/// The version of `paddock credentials log --json`.
pub const LOG_API: &str = "paddock.credentials.log/v1";
/// The version of the standing grants file.
pub const GRANTS_API: &str = "paddock.credentials.grants/v1";

/// Decision 3 of ENV1: no keychain, no key in a file.
pub const NO_KEYCHAIN: &str = "paddock cannot reach a keychain on this computer, and it does not keep keys in a file. Keep the key in your password manager and let paddock read it with backend = \"command\": in [credentials.NAME], set from = \"command\" and command = [\"PROGRAM\", \"ARGS\"]";

/// The answers of the grant question, besides No.
pub const THIS_RUN: &str = "This run";
pub const ALWAYS_HERE: &str = "Always in this project";

/// The keychain of this computer.
pub fn keychain(env: &HostEnv) -> Result<Keyring, StoreError> {
    Ok(Keyring::find(env)?.service(SERVICE))
}

/// Whether the keychain tool is there. It does not unlock anything.
pub fn keychain_tool(env: &HostEnv) -> bool {
    Keyring::find(env).is_ok()
}

fn no_keychain(e: StoreError) -> anyhow::Error {
    match e {
        StoreError::Unavailable(d) => anyhow::anyhow!("{NO_KEYCHAIN}. ({d})"),
        other => anyhow::Error::from(other),
    }
}

/// The value of a key in the keychain.
pub fn read_value(env: &HostEnv, name: &str) -> Result<Secret, StoreError> {
    keychain(env)?.get(name)
}

/// The variable names that select a preset, and the prefixes of values.
pub fn preset_for(name: &str, value: Option<&[u8]>) -> Option<&'static str> {
    let n = name.to_ascii_uppercase();
    if n.starts_with("ANTHROPIC") {
        return Some("anthropic");
    }
    if n.starts_with("OPENAI") {
        return Some("openai");
    }
    if n.starts_with("GITHUB") || n.starts_with("GH_") {
        return Some("github");
    }
    let v = value?;
    if v.starts_with(b"sk-ant-") {
        Some("anthropic")
    } else if v.starts_with(b"sk-") {
        Some("openai")
    } else if v.starts_with(b"ghp_") || v.starts_with(b"github_pat_") || v.starts_with(b"gho_") {
        Some("github")
    } else {
        None
    }
}

/// What `paddock credentials add` writes.
#[derive(Debug, Clone, Default)]
pub struct NewKey {
    pub name: String,
    /// "gateway" (the default) or "deliver".
    pub deliver: bool,
    pub preset: Option<String>,
    /// For a key without a preset kept on this computer.
    pub hosts: Vec<String>,
    pub agents: Vec<String>,
    /// Project keys (project_key), or `*`.
    pub projects: Vec<String>,
}

fn valid_key_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.starts_with(|c: char| c.is_ascii_alphabetic())
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The `[credentials.NAME]` table of a new key, checked like the rest of
/// the configuration.
pub fn table(k: &NewKey) -> Result<toml_edit::Table> {
    if !valid_key_name(&k.name) {
        bail!(
            "'{}' is not a key name: use the variable name, with letters, digits and '_'",
            k.name
        );
    }
    let mut t = toml_edit::Table::new();
    t.insert(
        "via",
        toml_edit::value(if k.deliver { "deliver" } else { "gateway" }),
    );
    if let Some(p) = &k.preset {
        t.insert("preset", toml_edit::value(p.as_str()));
    }
    t.insert("from", toml_edit::value("store"));
    let list = |v: &[String]| {
        let mut a = toml_edit::Array::new();
        for s in v {
            a.push(s.as_str());
        }
        toml_edit::value(a)
    };
    if !k.deliver && k.preset.is_none() {
        if k.hosts.is_empty() {
            bail!(
                "{} is kept on this computer through the gateway, which needs the host name of the service: --host api.example.com, or a preset (--preset anthropic|openai|github). Or hand it to the agent with --deliver",
                k.name
            );
        }
        t.insert("hosts", list(&k.hosts));
        t.insert(
            "base_url_env",
            toml_edit::value(format!(
                "{}_BASE_URL",
                k.name
                    .trim_end_matches("_KEY")
                    .trim_end_matches("_TOKEN")
                    .trim_end_matches("_API")
            )),
        );
    }
    if !k.agents.is_empty() {
        t.insert("harnesses", list(&k.agents));
    }
    if !k.projects.is_empty() {
        t.insert("projects", list(&k.projects));
    }
    let doc = format!("[credentials.{}]\n{}", k.name, t);
    super::parse(&doc).with_context(|| format!("[credentials.{}]", k.name))?;
    Ok(t)
}

fn edit_config(env: &HostEnv, f: impl FnOnce(&mut toml_edit::Table) -> Result<()>) -> Result<()> {
    let file = env.paddock_config().join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is invalid", file.display()))?;
    let creds = doc
        .entry("credentials")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .context("[credentials] is not a table")?;
    creds.set_implicit(true);
    f(creds)?;
    let out = doc.to_string();
    super::parse(&out).context("the change would make [credentials] invalid")?;
    std::fs::create_dir_all(file.parent().context("no configuration folder")?)?;
    std::fs::write(&file, out)?;
    Ok(())
}

/// `paddock credentials add`: the value into the keychain, then the table
/// into the configuration. A table that is there already keeps its
/// grants; only the value is replaced ("Replace Value").
pub fn add(env: &HostEnv, k: &NewKey, value: &Secret) -> Result<bool> {
    if value.is_empty() {
        bail!("the value is empty: nothing was stored");
    }
    let existing = super::load(env)?;
    if let Some(c) = existing.get(&k.name) {
        if c.from != Source::Store {
            bail!(
                "[credentials.{}] reads its value from {} already: remove it first, or change it in the configuration",
                k.name,
                match c.from {
                    Source::Env => "the environment",
                    _ => "a command",
                }
            );
        }
    }
    let t = if existing.contains_key(&k.name) {
        None
    } else {
        Some(table(k)?)
    };
    let kc = keychain(env).map_err(no_keychain)?;
    kc.put(&k.name, value).map_err(no_keychain)?;
    // The value must come back the same, or nothing is configured.
    match kc.get(&k.name) {
        Ok(v) if v.as_bytes() == value.as_bytes() => {}
        Ok(_) | Err(StoreError::Missing) => {
            bail!(
                "the keychain did not keep the value of {}; nothing was configured",
                k.name
            )
        }
        Err(e) => return Err(no_keychain(e)),
    }
    let Some(t) = t else { return Ok(false) };
    edit_config(env, |creds| {
        creds.insert(&k.name, toml_edit::Item::Table(t));
        Ok(())
    })?;
    Ok(true)
}

/// `paddock credentials remove`: the table, its standing grants, and its
/// keychain item.
pub fn remove(env: &HostEnv, name: &str) -> Result<()> {
    let all = super::load(env)?;
    let Some(c) = all.get(name) else {
        bail!("there is no key {name} in [credentials]");
    };
    if c.from == Source::Store {
        keychain(env)
            .map_err(no_keychain)?
            .delete(name)
            .map_err(no_keychain)?;
    }
    edit_config(env, |creds| {
        creds.remove(name);
        Ok(())
    })?;
    let mut g = Grants::load(env);
    g.grants.remove(name);
    g.save(env)?;
    Ok(())
}

// ---------------------------------------------------------------------
// Grants.

/// How an agent in a project gets a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// `projects = ["*"]`.
    Any,
    /// The project is in `projects`.
    Project,
    /// An answer "Always in this project".
    Standing,
    /// One question at the next start.
    Ask,
}

impl Grant {
    pub fn name(self) -> &'static str {
        match self {
            Grant::Any => "any",
            Grant::Project => "project",
            Grant::Standing => "standing",
            Grant::Ask => "ask",
        }
    }
}

/// One standing grant: an agent in a project.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pair {
    pub agent: String,
    pub project: PathBuf,
    #[serde(default)]
    pub since: u64,
}

/// The standing grants, in the state folder.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Grants {
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub grants: BTreeMap<String, Vec<Pair>>,
}

fn grants_file(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("credential-grants.json")
}

impl Grants {
    pub fn load(env: &HostEnv) -> Self {
        std::fs::read(grants_file(env))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, env: &HostEnv) -> Result<()> {
        crate::privfs::dir(&env.paddock_state())?;
        let mut me = self.clone();
        me.api_version = GRANTS_API.into();
        me.grants.retain(|_, v| !v.is_empty());
        crate::privfs::write_atomic(&grants_file(env), serde_json::to_vec_pretty(&me)?)?;
        Ok(())
    }

    pub fn has(&self, name: &str, agent: &str, project: &Path) -> bool {
        self.grants
            .get(name)
            .is_some_and(|v| v.iter().any(|p| p.agent == agent && p.project == project))
    }

    pub fn add(&mut self, name: &str, agent: &str, project: &Path) {
        if self.has(name, agent, project) {
            return;
        }
        let v = self.grants.entry(name.to_string()).or_default();
        v.push(Pair {
            agent: agent.into(),
            project: project.to_path_buf(),
            since: crate::state::now(),
        });
        v.sort();
    }

    pub fn remove(&mut self, name: &str, agent: &str, project: &Path) -> bool {
        let Some(v) = self.grants.get_mut(name) else {
            return false;
        };
        let before = v.len();
        v.retain(|p| !(p.agent == agent && p.project == project));
        before != v.len()
    }
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// How `agent` in `project` gets the key, when `harnesses` lets it.
pub fn grant(env: &HostEnv, name: &str, c: &Credential, agent: &str, project: &Path) -> Grant {
    grant_with(env, &Grants::load(env), name, c, agent, project)
}

fn grant_with(
    env: &HostEnv,
    g: &Grants,
    name: &str,
    c: &Credential,
    agent: &str,
    project: &Path,
) -> Grant {
    let here = canonical(project);
    if c.projects.iter().any(|p| p == "*") {
        return Grant::Any;
    }
    if c.projects
        .iter()
        .any(|p| canonical(&env.expand(p, &env.home)) == here)
    {
        return Grant::Project;
    }
    if g.has(name, agent, &here) {
        return Grant::Standing;
    }
    Grant::Ask
}

/// `paddock credentials grant NAME --agent A [--project P]`. Without a
/// project, the agent joins `harnesses`. With one, the agent gets a
/// standing grant there. Returns the words for the user.
pub fn grant_cmd(env: &HostEnv, name: &str, agent: &str, project: Option<&Path>) -> Result<String> {
    let all = super::load(env)?;
    let c = all
        .get(name)
        .with_context(|| format!("there is no key {name} in [credentials]"))?;
    let mut said = Vec::new();
    if !c.harnesses.is_empty() && !c.harnesses.iter().any(|h| h == agent) {
        edit_config(env, |creds| {
            let t = creds
                .get_mut(name)
                .and_then(|t| t.as_table_like_mut())
                .context("not a table")?;
            let arr = t
                .get_mut("harnesses")
                .and_then(|h| h.as_array_mut())
                .context("harnesses is not a list")?;
            arr.push(agent);
            Ok(())
        })?;
        said.push(format!("{agent} can use {name}"));
    }
    if let Some(p) = project {
        let p = canonical(p);
        let mut g = Grants::load(env);
        g.add(name, agent, &p);
        g.save(env)?;
        said.push(format!(
            "{agent} gets {name} in {} without a question",
            p.display()
        ));
    }
    if said.is_empty() {
        said.push(format!(
            "{agent} can use {name} already; the first start in each project asks once"
        ));
    }
    Ok(format!(
        "{}. {}",
        said.join(". "),
        crate::envset::NEXT_START
    ))
}

/// `paddock credentials ungrant NAME --agent A [--project P]`.
pub fn ungrant_cmd(
    env: &HostEnv,
    name: &str,
    agent: &str,
    project: Option<&Path>,
) -> Result<String> {
    let all = super::load(env)?;
    let c = all
        .get(name)
        .with_context(|| format!("there is no key {name} in [credentials]"))?;
    let mut g = Grants::load(env);
    if let Some(p) = project {
        let p = canonical(p);
        let removed = g.remove(name, agent, &p);
        g.save(env)?;
        let listed = c
            .projects
            .iter()
            .any(|x| x == "*" || canonical(&env.expand(x, &env.home)) == p);
        if listed {
            bail!(
                "[credentials.{name}] projects grants {} to every agent; remove it from projects in the configuration",
                p.display()
            );
        }
        if !removed {
            bail!("{agent} has no standing grant of {name} in {}", p.display());
        }
        return Ok(format!(
            "{agent} no longer gets {name} in {} without a question. {}",
            p.display(),
            crate::envset::NEXT_START
        ));
    }
    if c.harnesses.is_empty() {
        bail!(
            "[credentials.{name}] is for every agent. Name the agents that keep it: paddock credentials grant {name} --agent OTHER, for each one, after harnesses = [...] in the configuration"
        );
    }
    if !c.harnesses.iter().any(|h| h == agent) {
        bail!("{agent} has no grant of {name}");
    }
    if c.harnesses.len() == 1 {
        bail!("{agent} is the only agent of {name}; remove the key instead: paddock credentials remove {name}");
    }
    edit_config(env, |creds| {
        let arr = creds
            .get_mut(name)
            .and_then(|t| t.as_table_like_mut())
            .and_then(|t| t.get_mut("harnesses"))
            .and_then(|h| h.as_array_mut())
            .context("harnesses is not a list")?;
        arr.retain(|v| v.as_str() != Some(agent));
        Ok(())
    })?;
    if let Some(v) = g.grants.get_mut(name) {
        v.retain(|p| p.agent != agent);
    }
    g.save(env)?;
    Ok(format!(
        "{agent} no longer gets {name}. {}",
        crate::envset::NEXT_START
    ))
}

/// The answer to the grant question at a start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The keys that the run gets.
    pub allowed: Vec<String>,
    /// The cards that paddock showed: 0 or 1.
    pub cards: usize,
}

/// One question for the keys that this agent has no grant for in this
/// project: This run, Always in this project, or No. Keys with a grant
/// pass without one. Every step goes to the audit log, never a value.
pub fn ask_once(
    env: &HostEnv,
    spec: &crate::spec::SandboxSpec,
    approver: &dyn crate::broker::Approver,
    wait: std::time::Duration,
    plan: &super::Plan,
) -> Asked {
    let mut g = Grants::load(env);
    let mut allowed = Vec::new();
    let mut open = Vec::new();
    for (name, c) in &plan.deliver {
        let how = grant_with(env, &g, name, c, &spec.harness, &spec.project);
        if how == Grant::Ask {
            open.push((name.clone(), c.clone()));
        } else {
            super::audit(env, spec, name, &format!("granted ({})", how.name()));
            allowed.push(name.clone());
        }
    }
    if open.is_empty() {
        return Asked { allowed, cards: 0 };
    }
    let names: Vec<&str> = open.iter().map(|(n, _)| n.as_str()).collect();
    let list: Vec<String> = open.iter().map(|(n, c)| super::describe(n, c)).collect();
    let card = crate::broker::Pending {
        id: format!("cred-{}-grant-{}", spec.id, crate::state::now()),
        sandbox: spec.id.clone(),
        harness: spec.harness.clone(),
        project: spec.project.clone(),
        capability: CAPABILITY.into(),
        summary: format!(
            "{} asks for {} in {}",
            spec.harness,
            names.join(", "),
            spec.project.display()
        ),
        detail: format!(
            "{}\n\n{THIS_RUN}: only this start. {ALWAYS_HERE}: {} gets it here without this question. No: the agent starts without it.",
            list.join("\n"),
            spec.harness
        ),
        created: crate::state::now(),
        expires: crate::state::now() + wait.as_secs(),
        session_allowed: false,
        choices: vec![THIS_RUN.into(), ALWAYS_HERE.into()],
        ..Default::default()
    };
    let answer = approver.choose(env, &card, wait);
    let decision = match answer.as_deref() {
        Some(ALWAYS_HERE) => {
            let here = canonical(&spec.project);
            for n in &names {
                g.add(n, &spec.harness, &here);
            }
            if let Err(e) = g.save(env) {
                log::warn!("cannot keep the standing grant: {e:#}");
            }
            "allowed always in this project"
        }
        Some(THIS_RUN) => "allowed for this run",
        _ => "not allowed",
    };
    super::audit(env, spec, &names.join(", "), decision);
    if answer
        .as_deref()
        .is_some_and(|a| a == ALWAYS_HERE || a == THIS_RUN)
    {
        allowed.extend(names.iter().map(|n| n.to_string()));
    }
    Asked { allowed, cards: 1 }
}

// ---------------------------------------------------------------------
// Uses.

/// One use of a key, from the audit log. Never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Use {
    pub name: String,
    pub run: String,
    pub agent: String,
    pub project: PathBuf,
    pub time: u64,
    /// "request", "start", "signature", "read".
    pub kind: &'static str,
}

/// Every use in the audit log of host requests.
pub fn uses(env: &HostEnv) -> Vec<Use> {
    let Ok(text) = std::fs::read_to_string(crate::broker::audit_file(env)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["capability"] != CAPABILITY {
            continue;
        }
        let detail = v["detail"].as_str().unwrap_or("");
        let decision = v["decision"].as_str().unwrap_or("");
        let first = |p: &str| {
            detail
                .strip_prefix(p)
                .and_then(|d| d.split(' ').next())
                .map(str::to_string)
        };
        let (names, kind): (Vec<String>, &'static str) = if decision.starts_with("forwarded") {
            (first("gateway ").into_iter().collect(), "request")
        } else if decision == "read" {
            (first("secret ").into_iter().collect(), "read")
        } else if decision == "allowed" && detail.starts_with("agent ") {
            (first("agent ").into_iter().collect(), "signature")
        } else if decision == "delivered" {
            let rest = detail
                .strip_prefix("credentials ")
                .or_else(|| detail.strip_prefix("deliver "))
                .unwrap_or("");
            (
                rest.split(", ")
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                "start",
            )
        } else {
            continue;
        };
        for name in names {
            out.push(Use {
                name,
                run: v["sandbox"].as_str().unwrap_or("").into(),
                agent: v["harness"].as_str().unwrap_or("").into(),
                project: PathBuf::from(v["project"].as_str().unwrap_or("")),
                time: v["time"].as_u64().unwrap_or(0),
                kind,
            });
        }
    }
    out
}

/// The uses of a key by an agent in a project (None: any), and the last.
pub fn count(
    uses: &[Use],
    name: &str,
    agent: Option<&str>,
    project: Option<&Path>,
) -> (u64, Option<u64>) {
    let mut n = 0;
    let mut last = 0;
    for u in uses {
        if u.name == name
            && agent.is_none_or(|a| u.agent == a)
            && project.is_none_or(|p| u.project == p)
        {
            n += 1;
            last = last.max(u.time);
        }
    }
    (n, (last > 0).then_some(last))
}

/// One line of `paddock credentials log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogRow {
    pub name: String,
    pub agent: String,
    pub project: PathBuf,
    pub count: u64,
    /// "requests", "starts", "signatures", "reads".
    pub unit: &'static str,
    pub runs: u64,
    pub last: u64,
}

/// A key, an agent, a project and a kind of use.
type LogKey = (String, String, PathBuf, &'static str);

/// The uses by key, agent, project and kind, the newest first.
pub fn log(env: &HostEnv, name: Option<&str>) -> Vec<LogRow> {
    let mut map: BTreeMap<LogKey, (u64, u64, Vec<String>)> = BTreeMap::new();
    for u in uses(env) {
        if name.is_some_and(|n| n != u.name) {
            continue;
        }
        let e = map
            .entry((u.name.clone(), u.agent.clone(), u.project.clone(), u.kind))
            .or_default();
        e.0 += 1;
        e.1 = e.1.max(u.time);
        if !e.2.contains(&u.run) {
            e.2.push(u.run.clone());
        }
    }
    let mut rows: Vec<LogRow> = map
        .into_iter()
        .map(
            |((name, agent, project, kind), (count, last, runs))| LogRow {
                name,
                agent,
                project,
                count,
                unit: match kind {
                    "request" => "requests",
                    "start" => "starts",
                    "signature" => "signatures",
                    _ => "reads",
                },
                runs: runs.len() as u64,
                last,
            },
        )
        .collect();
    rows.sort_by(|a, b| b.last.cmp(&a.last).then(a.name.cmp(&b.name)));
    rows
}

/// The state words of a key: "kept_on_host", "handed_over", "agent",
/// "not_available".
pub fn state(c: &Credential) -> &'static str {
    match c.via {
        Via::Gateway => "kept_on_host",
        Via::Deliver => "handed_over",
        Via::Agent => "agent",
        Via::Intercept => "not_available",
    }
}

/// What a use counts, by the way the key passes.
pub fn unit(c: &Credential) -> &'static str {
    match c.via {
        Via::Gateway => "requests",
        Via::Deliver if c.as_ == super::DeliverAs::File => "reads",
        Via::Deliver => "starts",
        Via::Agent if c.kind() == Kind::Ssh => "signatures",
        Via::Agent => "uses",
        Via::Intercept => "requests",
    }
}

/// Where the value lives, in words.
pub fn store_name(name: &str, c: &Credential) -> String {
    match (c.via, c.from) {
        (Via::Agent, _) if c.kind() == Kind::Docker => "docker login".into(),
        (Via::Agent, _) if c.kind() == Kind::Aws => "aws profile".into(),
        (Via::Agent, _) => "ssh agent".into(),
        (_, Source::Store) => "keychain".into(),
        (_, Source::Env) => format!("env {}", c.host_var(name)),
        (_, Source::Command) => format!(
            "command {}",
            c.command.first().map(String::as_str).unwrap_or("")
        ),
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
