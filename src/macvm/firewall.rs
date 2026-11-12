//! The firewall layer of the macOS guest route: the rules of an app profile
//! and their file, the glob matcher for hosts, the specificity order, and
//! `decide`, the pure decision for one connection. Pure: tests on Linux.
//!
//! The rules file, one per app profile, `~/.config/paddock/firewall/
//! <app>.toml`, mode 0600, written whole and renamed into place:
//!
//! ```toml
//! [[rule]]
//! id = "rule-1a2b3c"
//! app = "com.example.notes"      # bundle id, or "*" for any process
//! team = "EXAMPLE123"          # optional
//! host = "*.example.com"        # "*", "*.x.y", "x.y", or an IP
//! port = 443                   # or "*"
//! verdict = "allow"            # allow | deny
//! scope = "always"             # always | session
//! made = "2026-11-04T10:22:00Z"
//! by = "user"                  # user | profile | approver
//! ```
//!
//! Three sets of rules take part in a decision: the file, the rules of
//! the running session (`scope = "session"`, in memory, gone at quit),
//! and the seeds of the profile (`seed_hosts` of `[app.macos]`, `by =
//! "profile"`, never in the file). The most specific rule wins; at the
//! same specificity a user rule beats a seed, a session rule beats an
//! always rule, and deny beats allow.

use super::proxy_header::Identity;
use super::text;
use crate::broker::ConnectAnswer;
use crate::env::HostEnv;
use crate::proxy::allow::Allowlist;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::SystemTime;

#[cfg(test)]
thread_local! {
    pub(super) static READ_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
}

pub const BY_USER: &str = "user";
pub const BY_PROFILE: &str = "profile";
pub const BY_APPROVER: &str = "approver";

/// `allow` or `deny`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Deny,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    pub fn parse(word: &str) -> Result<Self> {
        match word {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            w => bail!(text::bad_verdict(w)),
        }
    }
}

/// `always` (the file) or `session` (memory, gone at quit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Always,
    Session,
}

impl Scope {
    pub fn word(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Session => "session",
        }
    }

    pub fn parse(word: &str) -> Result<Self> {
        match word {
            "always" => Ok(Self::Always),
            "session" => Ok(Self::Session),
            w => bail!(text::bad_scope(w)),
        }
    }
}

/// A port of a rule: one port, or `*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    Any,
    Num(u16),
}

impl Port {
    pub fn text(self) -> String {
        match self {
            Self::Any => "*".into(),
            Self::Num(n) => n.to_string(),
        }
    }

    pub fn parse(word: &str) -> Result<Self> {
        if word == "*" {
            return Ok(Self::Any);
        }
        match word.parse::<u16>() {
            Ok(n) if n > 0 => Ok(Self::Num(n)),
            _ => bail!(text::bad_port(word)),
        }
    }

    fn matches(self, port: u16) -> bool {
        match self {
            Self::Any => true,
            Self::Num(n) => n == port,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum PortRepr {
    Num(u16),
    Word(String),
}

impl Serialize for Port {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Any => PortRepr::Word("*".into()).serialize(s),
            Self::Num(n) => PortRepr::Num(*n).serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Port {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match PortRepr::deserialize(d)? {
            PortRepr::Num(n) if n > 0 => Ok(Self::Num(n)),
            PortRepr::Num(n) => Err(serde::de::Error::custom(text::bad_port(&n.to_string()))),
            PortRepr::Word(w) => Self::parse(&w).map_err(serde::de::Error::custom),
        }
    }
}

/// One rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    /// The bundle id of the process, or `*` for any process of the session.
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub host: String,
    pub port: Port,
    pub verdict: Verdict,
    pub scope: Scope,
    /// RFC 3339, UTC.
    pub made: String,
    /// Unix time at which a temporary rule stops applying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    /// `user`, `profile` or `approver`.
    pub by: String,
}

/// A connection, as the decision sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conn<'a> {
    /// The bundle id of the process, or its file name, or `unknown`.
    pub app: &'a str,
    /// The outermost app of a helper process.
    pub parent_app: Option<&'a str>,
    pub team: Option<&'a str>,
    pub host: &'a str,
    pub port: u16,
}

impl<'a> Conn<'a> {
    pub fn new(app: &'a str, host: &'a str, port: u16) -> Self {
        Self {
            app,
            parent_app: None,
            team: None,
            host,
            port,
        }
    }

    /// The connection of a guest process. `app_name` is
    /// `Identity::app_name()` of the same identity, owned by the caller.
    pub fn of(id: &'a Identity, app_name: &'a str, host: &'a str, port: u16) -> Self {
        Self {
            app: app_name,
            parent_app: id.parent_app.as_deref(),
            team: id.team.as_deref(),
            host,
            port,
        }
    }
}

// ---- Matching --------------------------------------------------------------

fn norm_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Whether a host glob matches a host: `*` every host, `*.x.y` every
/// name under `x.y` (not `x.y` itself), `x.y` the name itself, and an
/// IP address the same address in any spelling. Case and a trailing dot
/// do not count.
pub fn host_matches(glob: &str, host: &str) -> bool {
    let glob = norm_host(glob);
    let host = norm_host(host);
    if glob == "*" {
        return true;
    }
    // An address is no name: only `*` or the same address matches it.
    if let Ok(b) = host.parse::<IpAddr>() {
        return glob.parse::<IpAddr>().is_ok_and(|a| a == b);
    }
    if let Some(base) = glob.strip_prefix("*.") {
        return host.len() > base.len() + 1
            && host.ends_with(base)
            && host.as_bytes()[host.len() - base.len() - 1] == b'.';
    }
    glob == host
}

// Whether two supported host patterns can match the same host. Unknown
// patterns are treated as overlapping: file contents are not necessarily
// produced by `add`, and an uncertain overlap must not create a grant.
pub(super) fn hosts_overlap(a: &str, b: &str) -> bool {
    if check_host_glob(a).is_err() || check_host_glob(b).is_err() {
        return true;
    }
    let a = norm_host(a);
    let b = norm_host(b);
    if a == "*" || b == "*" {
        return true;
    }
    match (a.strip_prefix("*."), b.strip_prefix("*.")) {
        (Some(x), Some(y)) => x == y || host_matches(&a, y) || host_matches(&b, x),
        (Some(_), None) => host_matches(&a, &b),
        (None, Some(_)) => host_matches(&b, &a),
        (None, None) => host_matches(&a, &b),
    }
}

// An app rule can match either the process or its parent, so even two
// differently named apps can overlap. Teams and ports have one value each.
pub(super) fn rule_scopes_overlap(a: &Rule, b: &Rule) -> bool {
    let teams_overlap = match (&a.team, &b.team) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    let ports_overlap = a.port == Port::Any || b.port == Port::Any || a.port == b.port;
    teams_overlap && ports_overlap && hosts_overlap(&a.host, &b.host)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// Check a host glob for `add`: `*`, `*.name`, a host name, or an IP.
pub fn check_host_glob(glob: &str) -> Result<String> {
    let g = norm_host(glob);
    let ok = !glob.trim().ends_with('.')
        && (g == "*"
            || g.strip_prefix("*.")
                .map(valid_name)
                .unwrap_or_else(|| valid_name(&g) || g.parse::<IpAddr>().is_ok()));
    if !ok {
        bail!(text::bad_host_glob(glob));
    }
    Ok(g)
}

/// The security guard of checklist item 19: a rule that allows every
/// host needs `--yes-all-hosts`.
pub fn all_hosts_allow(rule: &Rule) -> bool {
    norm_host(&rule.host) == "*" && rule.verdict == Verdict::Allow
}

/// Whether a rule matches a connection: the app (the process or its
/// outermost app), the team when the rule names one, the host glob and
/// the port.
pub fn matches(rule: &Rule, conn: &Conn) -> bool {
    let app_ok =
        rule.app == "*" || rule.app == conn.app || conn.parent_app.is_some_and(|p| p == rule.app);
    let team_ok = match &rule.team {
        None => true,
        Some(t) => conn.team == Some(t.as_str()),
    };
    app_ok && team_ok && host_matches(&rule.host, conn.host) && rule.port.matches(conn.port)
}

/// How specific a rule is, most specific first: an exact app, then the
/// host (exact, then `*.x.y` with more labels first, then `*`), then an
/// exact port. "Wildcards from the right": a wildcard port costs the
/// least, a wildcard app the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity {
    pub app_exact: bool,
    pub host: u8,
    pub host_labels: usize,
    pub port_exact: bool,
}

pub fn specificity(rule: &Rule) -> Specificity {
    let host = norm_host(&rule.host);
    let (h, labels) = if host == "*" {
        (0, 0)
    } else if let Some(base) = host.strip_prefix("*.") {
        (1, base.split('.').count())
    } else {
        (2, host.split('.').count())
    };
    Specificity {
        app_exact: rule.app != "*",
        host: h,
        host_labels: labels,
        port_exact: rule.port != Port::Any,
    }
}

/// The rule that decides a connection: the most specific match; at the
/// same specificity a user rule beats a seed of the profile, a session
/// rule beats an always rule, and deny beats allow.
pub fn winner<'r>(rules: &'r [Rule], conn: &Conn) -> Option<&'r Rule> {
    rules
        .iter()
        .filter(|r| r.expires.is_none_or(|e| e > crate::state::now()) && matches(r, conn))
        .max_by_key(|r| {
            (
                specificity(r),
                r.by != BY_PROFILE,
                r.scope == Scope::Session,
                r.verdict == Verdict::Deny,
            )
        })
}

// ---- The decision ----------------------------------------------------------

/// The network mode of a session on this route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    None,
    Api,
    Allowlist,
    Egress,
    Ask,
}

impl Mode {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "none" => Self::None,
            "api" => Self::Api,
            "allowlist" => Self::Allowlist,
            "egress" => Self::Egress,
            "ask" => Self::Ask,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Api => "api",
            Self::Allowlist => "allowlist",
            Self::Egress => "egress",
            Self::Ask => "ask",
        }
    }
}

/// What happens to a connection, with the id of the rule that said so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow(Option<String>),
    Deny(Option<String>),
    Ask,
}

/// The decision for a connection, in this order: the mode first (`none`
/// denies, `api` and `allowlist` deny a host outside their list before any
/// rule, because the identity in a rule can be forged), the rules second,
/// and with no match the default of the mode: `egress`, `api` and
/// `allowlist` allow, `ask` asks.
pub fn decide(mode: Mode, api_list: Option<&Allowlist>, rules: &[Rule], conn: &Conn) -> Decision {
    match mode {
        Mode::None => return Decision::Deny(None),
        Mode::Api | Mode::Allowlist => {
            if api_list.is_none_or(|l| l.check(conn.host, conn.port).is_err()) {
                return Decision::Deny(None);
            }
        }
        Mode::Egress | Mode::Ask => {}
    }
    if let Some(r) = winner(rules, conn) {
        return match r.verdict {
            Verdict::Allow => Decision::Allow(Some(r.id.clone())),
            Verdict::Deny => Decision::Deny(Some(r.id.clone())),
        };
    }
    match mode {
        Mode::Ask => Decision::Ask,
        _ => Decision::Allow(None),
    }
}

// ---- Making rules ----------------------------------------------------------

/// Six hex characters of a hash, for an id.
fn hash6(parts: &[&str]) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    crate::manifest::hex(&h.finalize())[..6].to_string()
}

mod rule_creation;
pub use rule_creation::{new_rule, rule_of_answer, seed_rule};

pub mod site;

// ---- The rules of a profile ------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

/// The rules of an app profile: the file, the session rules and the
/// seeds.
#[derive(Debug, Clone)]
pub struct Rules {
    pub path: PathBuf,
    pub file: Vec<Rule>,
    pub session: Vec<Rule>,
    pub seeds: Vec<Rule>,
    /// The modification time of the file at the last read, for
    /// `reload_if_changed`.
    modified: Option<SystemTime>,
    /// Exact contents last read or saved, for detecting stale writers.
    snapshot: Option<String>,
    /// The file did not parse: `save` refuses, so the file stays as the
    /// user left it, and the session runs with the rules it gets.
    broken: bool,
}

/// A cooperating writer committed after this snapshot was read. This is
/// different from an I/O failure: the edit must not be applied as-is.
#[derive(Debug)]
pub struct SaveConflict;

impl std::fmt::Display for SaveConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("firewall rules changed on disk; reload and retry")
    }
}

impl std::error::Error for SaveConflict {}

impl Rules {
    /// The rules file of an app profile:
    /// `~/.config/paddock/firewall/<app>.toml`.
    pub fn path(env: &HostEnv, app: &str) -> PathBuf {
        super::report::rules_file(env, app)
    }

    /// The rules of the file of `app`, or none when there is no file.
    pub fn load(env: &HostEnv, app: &str) -> Result<Self> {
        Self::load_at(Self::path(env, app))
    }

    pub fn load_at(path: PathBuf) -> Result<Self> {
        let mut r = Self::broken(path);
        r.broken = false;
        r.read()?;
        Ok(r)
    }

    /// No rules of the file, and no save of it: for a file that does not
    /// parse. The seeds and the session rules work as usual.
    pub fn broken(path: PathBuf) -> Self {
        Self {
            path,
            file: Vec::new(),
            session: Vec::new(),
            seeds: Vec::new(),
            modified: None,
            snapshot: None,
            broken: true,
        }
    }

    fn read(&mut self) -> Result<()> {
        #[cfg(test)]
        READ_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        let text = match self.read_text()? {
            Some(text) => text,
            None => {
                self.file.clear();
                self.modified = None;
                self.snapshot = None;
                return Ok(());
            }
        };
        let f: File = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!(text::rules_file_invalid(&self.path, &e.to_string())))?;
        self.file = f.rules;
        self.snapshot = Some(text);
        self.modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        Ok(())
    }

    // A dangling rules link is uninspectable state, not an absent file.
    fn read_text(&self) -> Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(&self.path) {
                    Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Ok(_) => Err(e).with_context(|| text::cannot_read(&self.path)),
                    Err(error) => Err(error).with_context(|| text::cannot_read(&self.path)),
                }
            }
            Err(e) => Err(e).with_context(|| text::cannot_read(&self.path)),
        }
    }

    /// Serialize a rule decision with cooperating writers of the rules file.
    /// The lock inode is permanent; removing it would split the lock domain.
    pub fn decision_lock(&self) -> std::io::Result<std::fs::File> {
        use std::os::unix::io::AsRawFd;
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".lock");
        let file = crate::privfs::append(&PathBuf::from(name))?;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(file);
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// Read the file again when another process wrote it (`paddock app
    /// rules add` while the session runs). `Ok(true)` when it changed.
    pub fn reload_if_changed(&mut self) -> Result<bool> {
        // A replacement can retain the same timestamp. Compare the actual
        // rules instead of treating metadata equality as proof of identity.
        let before = self.file.clone();
        let modified = self.modified;
        self.read()?;
        Ok(before != self.file || modified != self.modified)
    }

    /// Write the file: 0600, whole, renamed into place. The session rules
    /// and the seeds stay out of it.
    pub fn save(&mut self) -> Result<()> {
        let lock = self.decision_lock()?;
        self.save_locked(&lock)
    }

    /// Save while the caller retains the same cooperating decision lock.
    pub(super) fn save_locked(&mut self, _lock: &std::fs::File) -> Result<()> {
        if self.broken {
            bail!(text::rules_file_invalid(&self.path, "not read"));
        }
        let current = self.read_text()?;
        if current != self.snapshot {
            return Err(SaveConflict.into());
        }
        let f = File {
            rules: self.file.clone(),
        };
        let body = toml::to_string(&f).context("firewall rules")?; // not user text
        let text = format!("{}\n{body}", text::rules_file_header()); // not user text
        crate::privfs::write_atomic(&self.path, &text)
            .with_context(|| text::cannot_write(&self.path))?;
        self.snapshot = Some(text);
        self.modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        Ok(())
    }

    /// Add a rule: a session rule to the session, any other to the file
    /// (not saved yet). A rule for every host with `allow` needs
    /// `yes_all_hosts`. A rule with the same app, team, host, port and
    /// scope replaces the old one.
    pub fn add(&mut self, mut rule: Rule, yes_all_hosts: bool) -> Result<&Rule> {
        rule.host = check_host_glob(&rule.host)?;
        if rule.app.trim().is_empty() {
            rule.app = "*".into();
        }
        if all_hosts_allow(&rule) && !yes_all_hosts {
            bail!(text::all_hosts_needs_flag());
        }
        let same = |r: &Rule| {
            r.app == rule.app && r.team == rule.team && r.host == rule.host && r.port == rule.port
        };
        let list = match rule.scope {
            Scope::Session => &mut self.session,
            Scope::Always => &mut self.file,
        };
        list.retain(|r| !same(r));
        list.push(rule);
        Ok(list.last().expect("just pushed"))
    }

    /// Remove a rule of the file or of the session by id. A seed cannot
    /// go: it is not in the file.
    pub fn remove(&mut self, id: &str, app: &str) -> Result<Rule> {
        if let Some(i) = self.file.iter().position(|r| r.id == id) {
            return Ok(self.file.remove(i));
        }
        if let Some(i) = self.session.iter().position(|r| r.id == id) {
            return Ok(self.session.remove(i));
        }
        if self.seeds.iter().any(|r| r.id == id) {
            bail!(text::rule_is_seed(id, app));
        }
        bail!(text::no_rule(id, app))
    }

    /// Seed the hosts of the profile (`seed_hosts` of `[app.macos]`): an
    /// allow rule each, `by = "profile"`, port 443. A host that is no
    /// glob is skipped.
    pub fn seed(&mut self, hosts: &[String], now: u64) {
        self.seeds = hosts
            .iter()
            .filter(|h| check_host_glob(h).is_ok())
            .map(|h| seed_rule(h, now))
            .collect();
    }

    /// Every rule that takes part in a decision: the session, the file,
    /// the seeds.
    pub fn all(&self) -> Vec<Rule> {
        let mut v = self.session.clone();
        v.extend(self.file.iter().cloned());
        v.extend(self.seeds.iter().cloned());
        v
    }

    pub fn find(&self, id: &str) -> Option<&Rule> {
        self.session
            .iter()
            .chain(self.file.iter())
            .chain(self.seeds.iter())
            .find(|r| r.id == id)
    }
}

mod report;
pub use report::{rule_line, rules_file_if_any, seed_hosts};

#[cfg(test)]
#[path = "firewall_tests.rs"]
mod tests;
