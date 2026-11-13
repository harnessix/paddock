//! The JSON of `paddock app rules NAME list|add|remove|change --json` and
//! `paddock app connections NAME --json`, for the launcher's Network
//! page. The rules of the file change here, under the lock of the rules
//! file. A rule of the running session only (`--session`, `--minutes`)
//! goes to the session as a `grant rule ...` line of the control socket
//! (`gate::Firewall::rule_op`), and the file stays as it is.
//!
//! The field names follow the launcher's `NetworkJSON.swift`. A new
//! field keeps `v`. A field that changes its meaning raises `v`.

use super::connlog::{self, Entry};
use super::firewall::{self, site, Port, Rule, Rules, Scope, Verdict};
use super::grants_cli::ask;
use super::grants_live::{Op, RuleAdd};
use super::host::{self, Session, DEFAULT_HOME};
use super::layout::Layout;
use super::session::Profile;
use super::text;
use crate::env::HostEnv;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The version of the JSON of this file.
pub const V: u32 = 1;
/// The minutes of `--minutes` without a number.
pub const DEFAULT_MINUTES: u64 = 10;
/// The longest `--minutes`: a day.
pub const MAX_MINUTES: u64 = 24 * 60;
/// The suffix of the id of the `*.` partner of a site rule.
pub const SUBDOMAINS: &str = "-subdomains";

/// How long a new rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// The rules file.
    Always,
    /// The running session, until the app stops.
    Session,
    /// The running session, for this many minutes at most.
    Minutes(u64),
}

/// One rule, as the launcher reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleJson {
    pub id: String,
    /// The bundle id of the process, or `*`.
    pub app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// `*`, `*.example.com`, `api.example.com` or an IP address.
    pub host: String,
    /// A number, or `"*"`.
    pub port: Port,
    /// `allow` or `deny`.
    pub verdict: &'static str,
    /// The same word as `verdict`.
    pub answer: &'static str,
    /// `always` (the rules file) or `session` (the running session).
    pub scope: &'static str,
    /// `always`, `session`, or `minutes` for a session rule with an end.
    pub lifetime: &'static str,
    /// RFC 3339, UTC.
    pub made: String,
    /// When a rule with an end stops applying, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// `user`, `profile` or `approver`, as the rule holds it.
    pub by: String,
    /// Who made it: `you`, `seed` (a seed host of the app profile),
    /// `profile` or `approver`.
    pub source: &'static str,
    /// The connections that the rule decided, from the connection log.
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

/// Who made a rule.
pub fn source(r: &Rule) -> &'static str {
    match r.by.as_str() {
        firewall::BY_PROFILE if r.id.starts_with("seed-") => "seed",
        firewall::BY_PROFILE => "profile",
        firewall::BY_APPROVER => "approver",
        _ => "you",
    }
}

/// How long a rule applies.
pub fn lifetime(r: &Rule) -> &'static str {
    match (r.scope, r.expires) {
        (Scope::Always, _) => "always",
        (Scope::Session, None) => "session",
        (Scope::Session, Some(_)) => "minutes",
    }
}

/// The count and the last time per rule id, from the log lines.
pub fn usage(entries: &[Entry]) -> BTreeMap<String, (u64, u64)> {
    let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for e in entries {
        let u = out.entry(e.rule.clone()).or_default();
        u.0 += e.count;
        u.1 = u.1.max(e.last.max(e.t));
    }
    out
}

pub fn rule_json(r: &Rule, used: &BTreeMap<String, (u64, u64)>) -> RuleJson {
    let (count, last) = used.get(&r.id).copied().unwrap_or_default();
    RuleJson {
        id: r.id.clone(),
        app: r.app.clone(),
        team: r.team.clone(),
        host: r.host.clone(),
        port: r.port,
        verdict: r.verdict.word(),
        answer: r.verdict.word(),
        scope: r.scope.word(),
        lifetime: lifetime(r),
        made: r.made.clone(),
        expires: r.expires.map(crate::waiting::format_rfc3339),
        by: r.by.clone(),
        source: source(r),
        count,
        last_used: (count > 0).then(|| crate::waiting::format_rfc3339(last)),
    }
}

/// The rules of an app, with its session when it runs.
pub struct Ctx {
    pub name: String,
    pub rules: Rules,
    pub profile: Profile,
    pub running: Option<Session>,
}

impl Ctx {
    /// The rules of `name`: the file, the seeds of the profile, and the
    /// session record when it runs.
    pub fn open(env: &HostEnv, name: &str) -> Result<Self> {
        if !host::valid_name(name) {
            bail!(text::bad_app_name(name));
        }
        let app_profile = crate::app::app_profile(env, name).ok();
        let mut rules = Rules::load(env, name)?;
        if app_profile.is_none() && !rules.path.exists() {
            bail!(text::rules_need_macvm(name));
        }
        rules.seed(
            &firewall::seed_hosts(app_profile.as_ref()),
            crate::state::now(),
        );
        let layout = Layout::new(env);
        Ok(Ctx {
            name: name.to_string(),
            rules,
            profile: layout.profile(name, DEFAULT_HOME),
            running: host::running(&layout, name, DEFAULT_HOME),
        })
    }

    /// The rules of the running session from its snapshot, without the
    /// ones that ended. The snapshot can lag a change.
    pub fn session_rules(&self) -> Vec<Rule> {
        if self.running.is_none() {
            return Vec::new();
        }
        let path = self.profile.logs().join("session-rules.json");
        let now = crate::state::now();
        std::fs::read(path)
            .ok()
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .and_then(|v| {
                serde_json::from_value::<Vec<Rule>>(v.get("rules").cloned().unwrap_or(v)).ok()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.expires.is_none_or(|e| e > now))
            .collect()
    }

    /// Every rule: the session, the file, the seeds.
    pub fn all(&self) -> Vec<Rule> {
        let mut v = self.session_rules();
        v.extend(self.rules.file.iter().cloned());
        v.extend(self.rules.seeds.iter().cloned());
        v
    }

    /// The lines of the connection log of the profile: the running
    /// session, or the last one.
    pub fn log(&self) -> Vec<Entry> {
        connlog::read_shared(&self.profile.connections_log())
    }

    fn applies(&self, scope: Scope) -> &'static str {
        if scope == Scope::Session || self.running.is_some() {
            "live"
        } else {
            "next_start"
        }
    }
}

/// `paddock app rules NAME list --json`.
#[derive(Debug, Serialize)]
pub struct Listing {
    pub v: u32,
    pub app: String,
    pub running: bool,
    pub rules: Vec<RuleJson>,
    /// Why the session rules can be missing or stale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

pub fn listing(ctx: &Ctx) -> Listing {
    let used = usage(&ctx.log());
    Listing {
        v: V,
        app: ctx.name.clone(),
        running: ctx.running.is_some(),
        rules: ctx.all().iter().map(|r| rule_json(r, &used)).collect(),
        warning: ctx
            .running
            .is_some()
            .then(|| text::session_rules_stale(&ctx.name)),
    }
}

/// A new rule: what `add` and `change` take from the command line.
#[derive(Debug, Clone)]
pub struct Spec {
    pub host: String,
    pub port: Port,
    pub app: String,
    pub team: Option<String>,
    pub verdict: Verdict,
    pub lifetime: Lifetime,
    /// Also `*.host`, with the id of the rule and [`SUBDOMAINS`].
    pub site: bool,
    pub yes_all_hosts: bool,
}

/// What a change did.
#[derive(Debug, Default)]
pub struct Done {
    pub applies: &'static str,
    /// The rules that the command wrote: one, or two for a site.
    pub written: Vec<Rule>,
    /// The ids that the command removed or replaced.
    pub removed: Vec<String>,
}

/// The rules of a spec, checked: a host glob, the confirmation of every
/// host, a site of a name. Nothing changes yet.
fn rules_of(ctx: &Ctx, s: &Spec) -> Result<Vec<Rule>> {
    let now = crate::state::now();
    let scope = match s.lifetime {
        Lifetime::Always => Scope::Always,
        Lifetime::Session => Scope::Session,
        Lifetime::Minutes(m) => {
            if m == 0 || m > MAX_MINUTES {
                bail!(text::bad_minutes(m));
            }
            Scope::Session
        }
    };
    if scope == Scope::Session && ctx.running.is_none() {
        bail!(text::rule_session_not_running(&ctx.name));
    }
    let host = firewall::check_host_glob(&s.host)?;
    if s.site && site::registrable_site(&host).is_none() {
        bail!(text::rule_no_site(&s.host));
    }
    let mut r = firewall::new_rule(
        &s.app,
        s.team.as_deref(),
        &host,
        s.port,
        s.verdict,
        scope,
        firewall::BY_USER,
        now,
    );
    if let Lifetime::Minutes(m) = s.lifetime {
        r.expires = Some(now + m * 60);
    }
    let mut out = vec![r.clone()];
    if s.site {
        let mut w = r;
        w.id.push_str(SUBDOMAINS);
        w.host = format!("*.{host}"); // not user text
        out.push(w);
    }
    // The checks of `Rules::add`, on a copy.
    let mut scratch = Rules::broken(ctx.rules.path.clone());
    for r in &out {
        scratch.add(r.clone(), s.yes_all_hosts)?;
    }
    Ok(out)
}

fn session_of(ctx: &Ctx) -> Result<&Session> {
    ctx.running
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!(text::rule_session_not_running(&ctx.name)))
}

/// Add `new` to the session, one line each. A partner that fails takes
/// the rules of this call out again.
fn add_live(ctx: &Ctx, new: &[Rule], yes_all_hosts: bool) -> Result<Vec<Rule>> {
    let s = session_of(ctx)?;
    let mut added = Vec::new();
    for r in new {
        let op = Op::RuleAdd(RuleAdd {
            rule: r.clone(),
            yes_all_hosts,
        });
        match ask(&s.control, &op) {
            Ok(a) => added.push(a.rule.unwrap_or_else(|| r.clone())),
            Err(e) => {
                for r in &added {
                    let _ = ask(&s.control, &Op::RuleRemove { id: r.id.clone() });
                }
                return Err(e);
            }
        }
    }
    Ok(added)
}

/// Put `new` into the file without the ids of `drop`, in one save.
/// Returns the rules written and the ids that left the file.
fn write_file(ctx: &mut Ctx, new: &[Rule], drop: &[String], yes: bool) -> Result<Vec<String>> {
    let before: Vec<String> = ctx.rules.file.iter().map(|r| r.id.clone()).collect();
    ctx.rules.file.retain(|r| !drop.contains(&r.id));
    for r in new {
        ctx.rules.add(r.clone(), yes)?;
    }
    ctx.rules.save()?;
    Ok(before
        .into_iter()
        .filter(|id| !ctx.rules.file.iter().any(|r| &r.id == id))
        .collect())
}

/// `add HOST`: a rule of the file, or of the running session.
pub fn add(env: &HostEnv, ctx: &mut Ctx, s: &Spec) -> Result<Done> {
    let new = rules_of(ctx, s)?;
    let scope = new[0].scope;
    let (written, removed) = if scope == Scope::Always {
        let removed = write_file(ctx, &new, &[], s.yes_all_hosts)?;
        (new, removed)
    } else {
        (add_live(ctx, &new, s.yes_all_hosts)?, Vec::new())
    };
    for r in &written {
        note(env, &ctx.name, r, "add");
    }
    Ok(Done {
        applies: ctx.applies(scope),
        written,
        removed,
    })
}

/// The ids of a target of `remove`: a rule id, or a host, in the file
/// and in the session. A seed is refused.
fn resolve(ctx: &Ctx, target: &str) -> Result<(Vec<String>, Vec<String>)> {
    let hit = |r: &Rule| r.id == target || r.host == target;
    let file: Vec<String> = ctx
        .rules
        .file
        .iter()
        .filter(|r| hit(r))
        .map(|r| r.id.clone())
        .collect();
    let mut session: Vec<String> = ctx
        .session_rules()
        .iter()
        .filter(|r| hit(r))
        .map(|r| r.id.clone())
        .collect();
    // The snapshot can lag: an id of the session goes to the session.
    if file.is_empty()
        && session.is_empty()
        && ctx.running.is_some()
        && target.starts_with("session-")
    {
        session.push(target.to_string());
    }
    if file.is_empty() && session.is_empty() {
        if ctx.rules.seeds.iter().any(hit) {
            bail!(text::rule_is_seed(target, &ctx.name));
        }
        bail!(text::no_rule(target, &ctx.name));
    }
    Ok((file, session))
}

/// Take the session rules `ids` out of the running session.
fn remove_live(ctx: &Ctx, ids: &[String]) -> Result<Vec<Rule>> {
    let mut gone = Vec::new();
    if ids.is_empty() {
        return Ok(gone);
    }
    let s = session_of(ctx)?;
    for id in ids {
        let a = ask(&s.control, &Op::RuleRemove { id: id.clone() })?;
        if let Some(r) = a.rule {
            gone.push(r);
        }
    }
    Ok(gone)
}

/// `remove ID|HOST...`: every target must name a rule, or nothing
/// changes.
pub fn remove(env: &HostEnv, ctx: &mut Ctx, targets: &[String]) -> Result<Done> {
    let mut file = Vec::new();
    let mut session = Vec::new();
    for t in targets {
        let (f, s) = resolve(ctx, t)?;
        file.extend(f);
        session.extend(s);
    }
    file.dedup();
    session.dedup();
    let old: Vec<Rule> = ctx
        .rules
        .file
        .iter()
        .filter(|r| file.contains(&r.id))
        .cloned()
        .collect();
    let mut removed = Vec::new();
    if !file.is_empty() {
        removed = write_file(ctx, &[], &file, false)?;
        for r in &old {
            note(env, &ctx.name, r, "remove");
        }
    }
    for r in remove_live(ctx, &session)? {
        removed.push(r.id.clone());
        note(env, &ctx.name, &r, "remove");
    }
    Ok(Done {
        applies: ctx.applies(if session.is_empty() {
            Scope::Always
        } else {
            Scope::Session
        }),
        written: Vec::new(),
        removed,
    })
}

/// What `change ID` keeps of the old rule when a flag is left out.
#[derive(Debug, Clone, Default)]
pub struct ChangeSpec {
    pub host: Option<String>,
    pub port: Option<Port>,
    pub verdict: Option<Verdict>,
    pub lifetime: Option<Lifetime>,
    pub site: bool,
    pub yes_all_hosts: bool,
}

/// `change ID`: the rule (and its `*.` partner) gives way to a new one
/// with the flags. The new rule is checked before anything changes.
pub fn change(env: &HostEnv, ctx: &mut Ctx, id: &str, c: &ChangeSpec) -> Result<Done> {
    let session = ctx.session_rules();
    let Some(old) = ctx
        .rules
        .file
        .iter()
        .chain(&session)
        .find(|r| r.id == id)
        .cloned()
    else {
        if ctx.rules.seeds.iter().any(|r| r.id == id) {
            bail!(text::rule_is_seed(id, &ctx.name));
        }
        bail!(text::no_rule(id, &ctx.name));
    };
    let old_lifetime = match (old.scope, old.expires) {
        (Scope::Always, _) => Lifetime::Always,
        (Scope::Session, None) => Lifetime::Session,
        (Scope::Session, Some(e)) => {
            Lifetime::Minutes(e.saturating_sub(crate::state::now()).div_ceil(60).max(1))
        }
    };
    let spec = Spec {
        host: c.host.clone().unwrap_or_else(|| old.host.clone()),
        port: c.port.unwrap_or(old.port),
        app: old.app.clone(),
        team: old.team.clone(),
        verdict: c.verdict.unwrap_or(old.verdict),
        lifetime: c.lifetime.unwrap_or(old_lifetime),
        site: c.site,
        yes_all_hosts: c.yes_all_hosts,
    };
    let new = rules_of(ctx, &spec)?;
    let partner = format!("{id}{SUBDOMAINS}");
    let gone: Vec<Rule> = ctx
        .rules
        .file
        .iter()
        .chain(&session)
        .filter(|r| r.id == id || r.id == partner)
        .cloned()
        .collect();
    let old_file: Vec<String> = gone
        .iter()
        .filter(|r| r.scope == Scope::Always)
        .map(|r| r.id.clone())
        .collect();
    let old_session: Vec<String> = gone
        .iter()
        .filter(|r| r.scope == Scope::Session)
        .map(|r| r.id.clone())
        .collect();
    let scope = new[0].scope;
    let mut removed = Vec::new();
    let written = if scope == Scope::Always {
        removed = write_file(ctx, &new, &old_file, spec.yes_all_hosts)?;
        removed.extend(remove_live(ctx, &old_session)?.into_iter().map(|r| r.id));
        new
    } else {
        let added = add_live(ctx, &new, spec.yes_all_hosts)?;
        if !old_file.is_empty() {
            removed = write_file(ctx, &[], &old_file, false)?;
        }
        // The session replaces a rule for the same target itself: an old
        // id that is gone already is no error.
        let s = session_of(ctx)?;
        for id in old_session {
            if let Ok(a) = ask(&s.control, &Op::RuleRemove { id: id.clone() }) {
                removed.push(a.rule.map(|r| r.id).unwrap_or(id));
            } else if !added.iter().any(|r| r.id == id) {
                removed.push(id);
            }
        }
        added
    };
    for r in &gone {
        note(env, &ctx.name, r, "remove");
    }
    for r in &written {
        note(env, &ctx.name, r, "add");
    }
    Ok(Done {
        applies: ctx.applies(scope),
        written,
        removed,
    })
}

/// The grant change and the stream event of a rule change.
fn note(env: &HostEnv, app: &str, r: &Rule, action: &str) {
    use crate::eventstream::schema::GrantSource;
    // The session notes its own rules in the grant changes.
    if r.scope == Scope::Always {
        let target = format!("{}:{}", r.host, r.port.text()); // not user text
        let value = format!("{} {}", r.verdict.word(), r.scope.word()); // not user text
        super::grants::note_user_rule(env, app, action, &target, &value);
    }
    crate::eventstream::publish(
        env,
        super::events_stream::historical_rule_grant(app, r, action, GrantSource::You),
    );
}

/// The answer of `add`, `remove` and `change` with `--json`.
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub v: u32,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applies: Option<&'static str>,
    /// The first rule that the command wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleJson>,
    /// The rules that the command wrote: one, or two for a site.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleJson>,
    /// The ids that the command removed or replaced.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Every rule after the change, as `list --json` prints them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<Vec<RuleJson>>,
    /// The line of a change that paddock refused: nothing changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The line of a failure on the way, for example of the disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The outcome of a change. The list is read again after it.
pub fn outcome(env: &HostEnv, name: &str, r: &Result<Done>) -> Outcome {
    let list = Ctx::open(env, name).ok().map(|c| listing(&c).rules);
    match r {
        Ok(d) => {
            let rules: Vec<RuleJson> = d
                .written
                .iter()
                .map(|r| rule_json(r, &BTreeMap::new()))
                .collect();
            Outcome {
                v: V,
                ok: true,
                applies: Some(d.applies),
                rule: rules.first().cloned(),
                rules,
                removed: d.removed.clone(),
                list,
                refused: None,
                error: None,
            }
        }
        Err(e) => {
            let line = format!("{e:#}");
            let failed = e
                .chain()
                .any(|c| c.is::<std::io::Error>() || c.is::<firewall::SaveConflict>());
            Outcome {
                v: V,
                ok: false,
                applies: None,
                rule: None,
                rules: Vec::new(),
                removed: Vec::new(),
                list,
                refused: (!failed).then(|| line.clone()),
                error: failed.then_some(line),
            }
        }
    }
}

/// One row of `paddock app connections NAME --json`: the log lines of
/// the same app, host, port, verdict and rule together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionJson {
    /// The first decision, RFC 3339.
    pub time: String,
    /// The last decision, RFC 3339.
    pub last: String,
    pub host: String,
    pub port: u16,
    /// `allow`, `deny` or `ask`.
    pub verdict: String,
    /// The same word as `verdict`.
    pub answer: String,
    /// The rule id that decided, or a word: `ask`, `once`, `timeout`,
    /// `stale`, `mode`, `egress`, `listed`, `system`, `approver`.
    pub rule: String,
    /// The process: its bundle id or its file name.
    pub process: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub count: u64,
    pub up: u64,
    pub down: u64,
}

/// `paddock app connections NAME --json`.
#[derive(Debug, Serialize)]
pub struct Connections {
    pub v: u32,
    pub app: String,
    pub running: bool,
    /// The lines are of a session that ended.
    pub last_session: bool,
    /// The log, for `paddock app log NAME`.
    pub log: PathBuf,
    pub connections: Vec<ConnectionJson>,
}

/// The rows of log lines, the newest last decision first, at most `limit`.
pub fn connection_rows(entries: &[Entry], limit: usize) -> Vec<ConnectionJson> {
    type Key = (String, Option<String>, String, u16, String, String);
    let mut rows: BTreeMap<Key, (u64, u64, u64, u64, u64)> = BTreeMap::new();
    for e in entries {
        let k = (
            e.app.clone(),
            e.team.clone(),
            e.host.clone(),
            e.port,
            e.verdict.clone(),
            e.rule.clone(),
        );
        let r = rows.entry(k).or_insert((e.t, 0, 0, 0, 0));
        r.0 = r.0.min(e.t);
        r.1 = r.1.max(e.last.max(e.t));
        r.2 += e.count;
        r.3 += e.up;
        r.4 += e.down;
    }
    let mut out: Vec<(u64, ConnectionJson)> = rows
        .into_iter()
        .map(
            |((app, team, host, port, verdict, rule), (t, last, count, up, down))| {
                (
                    last,
                    ConnectionJson {
                        time: crate::waiting::format_rfc3339(t),
                        last: crate::waiting::format_rfc3339(last),
                        host,
                        port,
                        answer: verdict.clone(),
                        verdict,
                        rule,
                        process: app,
                        team,
                        count,
                        up,
                        down,
                    },
                )
            },
        )
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().take(limit).map(|(_, c)| c).collect()
}

pub fn connections(env: &HostEnv, name: &str, limit: usize) -> Result<Connections> {
    if !host::valid_name(name) {
        bail!(text::bad_app_name(name));
    }
    let layout = Layout::new(env);
    let profile = layout.profile(name, DEFAULT_HOME);
    let running = host::running(&layout, name, DEFAULT_HOME).is_some();
    if !running && !profile.dir.is_dir() && crate::app::app_profile(env, name).is_err() {
        bail!(text::grant_no_profile(name));
    }
    let log = profile.connections_log();
    let entries = connlog::read_shared(&log);
    Ok(Connections {
        v: V,
        app: name.to_string(),
        running,
        last_session: !running && !entries.is_empty(),
        connections: connection_rows(&entries, limit),
        log,
    })
}

#[cfg(test)]
#[path = "rules_cli_tests.rs"]
mod tests;
