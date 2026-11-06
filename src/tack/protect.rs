//! How tackd protects a program that it starts from the other processes
//! of the same user: the two levels, their defaults, the helper accounts,
//! and the plan of a start (who it runs as, what it may reach, its
//! environment).
//!
//! - `protect = "uid"` (full): the program runs as a helper account of
//!   its own, `_tack_<user>`, that `sudo paddock tack setup` creates. A
//!   process of another uid cannot read the environment or the memory of
//!   the program (the kernel checks the uid for `/proc/PID/environ`,
//!   `/proc/PID/mem` and ptrace). The program reaches the working folder
//!   through a POSIX ACL (`acl`), and its home is its own folder under
//!   `/var/lib/paddock-tackd/homes`. The programs that it starts go through
//!   an exec wrapper (`wrap`) that puts the references back.
//! - `protect = "refs"`: the program runs as the user. The only guarantee
//!   is that shells, startup files and the other programs see references.
//!   Other processes of the user can still read the program's environment
//!   while it runs.
//!
//! The built-in profiles give `uid` to programs that only talk to the
//! network and write no project files: `gh` and the git credential
//! helpers. Every other program gets `refs` unless its entry says
//! otherwise.
//!
//! Everything here is a plain function of its inputs, so that the tests
//! run on every system; the system calls are in `start_uid` (Linux) and
//! `helpers`.

use super::config::{Allow, Tack};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// The program runs as the user; only the references are kept from
    /// the other processes.
    Refs,
    /// The program runs as the user's helper account.
    Uid,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Refs => "refs",
            Level::Uid => "uid",
        }
    }
}

/// What `protect = "refs"` does not protect, said the same way in the
/// audit, status, explain and the README.
pub const REFS_LIMIT: &str = "other processes of your user can still read this program's environment and memory while it runs";

/// What `protect = "uid"` protects.
pub const UID_GAIN: &str =
    "processes of your user cannot read this program's environment or memory";

/// The built-in profile of a program, by the name of its file: the level
/// and the profile's name. None: no profile, `refs`.
pub fn profile(program: &Path) -> Option<(Level, &'static str)> {
    let base = program.file_name()?.to_str()?;
    if base == "gh" {
        return Some((Level::Uid, "gh"));
    }
    if base.starts_with("git-credential-") && base.len() > "git-credential-".len() {
        return Some((Level::Uid, "git credential helper"));
    }
    None
}

/// The level of an entry that does not set `protect`.
pub fn default_level(program: &Path) -> Level {
    profile(program).map(|(l, _)| l).unwrap_or(Level::Refs)
}

/// Where an entry's level comes from, for status and explain.
pub fn source(a: &Allow) -> String {
    match (a.protect, profile(&a.program)) {
        (Some(_), _) => "set in its entry".into(),
        (None, Some((_, name))) => format!("the default of the built-in profile for {name}"),
        (None, None) => "the default".into(),
    }
}

/// Entries for the same program that disagree on the level or on write:
/// one start has one level, so the configuration must say one.
pub fn conflicts(t: &Tack) -> Vec<String> {
    let mut by: BTreeMap<&Path, Vec<(&str, Level, bool)>> = BTreeMap::new();
    for (name, s) in &t.secrets {
        for a in &s.allow {
            by.entry(a.program.as_path())
                .or_default()
                .push((name, a.level(), a.write));
        }
    }
    let mut out = Vec::new();
    for (p, v) in by {
        let (n0, l0, w0) = v[0];
        for &(n, l, w) in &v[1..] {
            if l != l0 {
                out.push(format!(
                    "{} has protect = \"{}\" in [tack.secrets.{n0}] and protect = \"{}\" in [tack.secrets.{n}]: give it the same level in every entry",
                    p.display(), l0.name(), l.name()
                ));
            } else if w != w0 {
                out.push(format!(
                    "{} has write = {w0} in [tack.secrets.{n0}] and write = {w} in [tack.secrets.{n}]: give it the same write in every entry",
                    p.display()
                ));
            }
        }
    }
    out
}

/// The level and write of one start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub level: Level,
    pub write: bool,
}

/// The level of a start: of the entries that name the program (by its
/// path or its real path) in the allowed secrets. Entries that disagree
/// through a link get the stronger level, and write only when every entry
/// of that level says write.
pub fn effective(cfg: &Tack, secrets: &[String], path: &Path, real: &Path) -> Effective {
    let mut entries: Vec<&Allow> = Vec::new();
    for n in secrets {
        let Some(s) = cfg.secrets.get(n) else {
            continue;
        };
        for a in &s.allow {
            if a.program == path
                || a.program == real
                || super::program::real_path(&a.program) == real
            {
                entries.push(a);
            }
        }
    }
    let level = entries
        .iter()
        .map(|a| a.level())
        .max()
        .unwrap_or(Level::Refs);
    let write = level == Level::Uid
        && entries
            .iter()
            .filter(|a| a.level() == level)
            .all(|a| a.write)
        && !entries.is_empty();
    Effective { level, write }
}

/// Whether any entry of a configuration runs at `uid`.
pub fn wants_uid(cfg: &Tack) -> bool {
    cfg.secrets
        .values()
        .flat_map(|s| s.allow.iter())
        .any(|a| a.level() == Level::Uid)
}

/// The first words of the comment (GECOS) of every helper account: setup
/// adopts and remove deletes only an account that has it.
pub const MARK: &str = "paddock tack helper";

/// The comment of the helper account of `uid`.
pub fn comment(user: &str, uid: u32) -> String {
    format!("{MARK} for uid {uid} ({user})")
}

/// Whether a comment is the one that setup writes for `uid`.
pub fn our_comment(gecos: &str, uid: u32) -> bool {
    gecos
        .strip_prefix(MARK)
        .and_then(|r| r.strip_prefix(" for uid "))
        .and_then(|r| r.split_whitespace().next())
        .is_some_and(|n| n == uid.to_string())
}

/// The name of the helper account of a user: `_tack_<user>`, or
/// `_tack_u<uid>` when the user name is long or has characters that
/// `useradd` refuses.
pub fn helper_name(user: &str, uid: u32) -> String {
    let plain = !user.is_empty()
        && user.len() <= 32 - "_tack_".len()
        && user
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-');
    if plain {
        format!("_tack_{user}")
    } else {
        format!("_tack_u{uid}")
    }
}

/// One helper account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Helper {
    /// The uid of the user that it belongs to.
    pub owner: u32,
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

/// The helper accounts that setup made: `helpers.json` in tackd's state
/// folder, root's, 0644 (it holds no secret).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub version: u32,
    pub helpers: Vec<Helper>,
}

impl Registry {
    pub const VERSION: u32 = 1;

    pub fn file(state: &Path) -> PathBuf {
        state.join("helpers.json")
    }

    /// Read the registry. No file: empty. A file that `owner` does not own
    /// or that others can write is refused: tackd runs programs as the
    /// accounts that it names.
    pub fn load(state: &Path, owner: u32) -> Result<Registry, String> {
        use std::os::unix::fs::MetadataExt;
        let f = Self::file(state);
        let m = match std::fs::symlink_metadata(&f) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::default()),
            Err(e) => return Err(format!("cannot read {}: {e}", f.display())),
        };
        if !m.is_file() || m.uid() != owner || m.mode() & 0o022 != 0 {
            return Err(format!(
                "{} is not a file that only uid {owner} can write: run sudo paddock tack setup",
                f.display()
            ));
        }
        let text =
            std::fs::read_to_string(&f).map_err(|e| format!("cannot read {}: {e}", f.display()))?;
        let r: Registry = serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} is damaged ({e}): run sudo paddock tack setup",
                f.display()
            )
        })?;
        if r.version != Self::VERSION {
            return Err(format!(
                "{} has version {}, this paddock reads version {}: run sudo paddock tack setup",
                f.display(),
                r.version,
                Self::VERSION
            ));
        }
        Ok(r)
    }

    /// Write it atomically (0644).
    pub fn save(&self, state: &Path) -> anyhow::Result<()> {
        let mut r = self.clone();
        r.version = Self::VERSION;
        r.helpers.sort_by_key(|h| h.owner);
        let text = serde_json::to_string_pretty(&r)? + "\n";
        super::service::atomic_write(&Self::file(state), text.as_bytes(), 0o644)
    }

    pub fn of_owner(&self, owner: u32) -> Option<&Helper> {
        self.helpers.iter().find(|h| h.owner == owner)
    }

    /// The helper whose uid this is: a request that comes from a program
    /// that tackd started at `uid`.
    pub fn of_helper(&self, uid: u32) -> Option<&Helper> {
        self.helpers.iter().find(|h| h.uid == uid)
    }

    pub fn put(&mut self, h: Helper) {
        self.helpers
            .retain(|x| x.owner != h.owner && x.uid != h.uid);
        self.helpers.push(h);
    }
}

/// The variables of the exec wrapper (`wrap`), in the environment of a
/// program at `uid`.
pub const WRAP_DIR: &str = "PADDOCK_TACK_WRAP";
/// The PATH that the program had before the wrapper folder.
pub const WRAP_PATH: &str = "PADDOCK_TACK_PATH";
/// The names of the user's secrets: the wrapper sets them to references.
pub const WRAP_SECRETS: &str = "PADDOCK_TACK_SECRETS";
/// The real paths of the allowlisted programs, `:` between them: the
/// wrapper sends their start to tackd.
pub const WRAP_LISTED: &str = "PADDOCK_TACK_LISTED";

/// Variables that name the user's own folders and sessions: a helper
/// cannot reach them, and a program that tried would fail or, worse,
/// write into a folder of the helper's own by that name.
const USER_ONLY: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "GNUPGHOME",
    "XAUTHORITY",
];

fn split(e: &str) -> Option<(&str, &str)> {
    e.split_once('=').filter(|(k, _)| !k.is_empty())
}

fn get<'a>(env: &'a [String], name: &str) -> Option<&'a str> {
    env.iter()
        .filter_map(|e| split(e))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The environment without the wrapper's variables, with the PATH from
/// before the wrapper folder: what a store tool or a nested start sees.
pub fn unwrap_env(env: &[String]) -> Vec<String> {
    let orig = get(env, WRAP_PATH).map(str::to_string);
    let mut out = Vec::new();
    for e in env {
        let Some((k, _)) = split(e) else { continue };
        if [WRAP_DIR, WRAP_PATH, WRAP_SECRETS, WRAP_LISTED].contains(&k) {
            continue;
        }
        if k == "PATH" {
            if let Some(o) = &orig {
                out.push(format!("PATH={o}"));
                continue;
            }
        }
        out.push(e.clone());
    }
    if orig.is_some() && get(env, "PATH").is_none() {
        out.push(format!("PATH={}", orig.unwrap_or_default()));
    }
    out
}

/// Add `safe.directory = *` as command-line git configuration
/// (`GIT_CONFIG_COUNT`), after what the caller has there: git refuses a
/// repository that another uid owns, and at `uid` every repository is
/// the user's. It reaches only what the ACL lets the helper read.
pub fn git_safe_directory(env: &mut Vec<String>) {
    let count = get(env, "GIT_CONFIG_COUNT").map(|c| c.trim().parse::<usize>());
    let n = match count {
        None => 0,
        Some(Ok(n)) => n,
        // git refuses a bad count by itself; leave it be.
        Some(Err(_)) => return,
    };
    let has = (0..n).any(|i| {
        get(env, &format!("GIT_CONFIG_KEY_{i}"))
            .is_some_and(|k| k.eq_ignore_ascii_case("safe.directory"))
            && get(env, &format!("GIT_CONFIG_VALUE_{i}")) == Some("*")
    });
    if has {
        return;
    }
    env.retain(|e| {
        split(e).is_none_or(|(k, _)| {
            k != "GIT_CONFIG_COUNT"
                && k != format!("GIT_CONFIG_KEY_{n}")
                && k != format!("GIT_CONFIG_VALUE_{n}")
        })
    });
    env.push(format!("GIT_CONFIG_COUNT={}", n + 1));
    env.push(format!("GIT_CONFIG_KEY_{n}=safe.directory"));
    env.push(format!("GIT_CONFIG_VALUE_{n}=*"));
}

/// What the program at `uid` needs besides the caller's environment.
pub struct HelperEnv<'a> {
    pub helper: &'a Helper,
    /// The wrapper folder, or None when tackd could not make one (the
    /// program's children then get the values).
    pub wrap: Option<&'a Path>,
    /// Every secret name of the user's configuration.
    pub names: &'a [String],
    /// The real paths of the user's allowlisted programs.
    pub listed: &'a [String],
}

/// The caller's environment for a program at `uid`: the helper's home,
/// none of the user's own folders and sessions, the wrapper first on
/// PATH, and git's `safe.directory`. The values come later (`launchenv`).
pub fn helper_env(caller: &[String], h: &HelperEnv) -> Vec<String> {
    let base = unwrap_env(caller);
    let path = get(&base, "PATH")
        .unwrap_or("/usr/local/bin:/usr/bin:/bin")
        .to_string();
    let mut out: Vec<String> = base
        .iter()
        .filter(|e| {
            split(e).is_some_and(|(k, _)| k != "HOME" && k != "PATH" && !USER_ONLY.contains(&k))
        })
        .cloned()
        .collect();
    out.push(format!("HOME={}", h.helper.home.display()));
    match h.wrap {
        Some(w) => {
            out.push(format!("PATH={}:{path}", w.display()));
            out.push(format!("{WRAP_DIR}={}", w.display()));
            out.push(format!("{WRAP_PATH}={path}"));
            out.push(format!("{WRAP_SECRETS}={}", h.names.join(",")));
            let listed: Vec<&str> = h
                .listed
                .iter()
                .map(String::as_str)
                .filter(|p| !p.contains(':'))
                .collect();
            out.push(format!("{WRAP_LISTED}={}", listed.join(":")));
        }
        None => out.push(format!("PATH={path}")),
    }
    git_safe_directory(&mut out);
    out
}

/// The ACL that a start grants: the helper reaches `folder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub folder: PathBuf,
    pub owner: u32,
    pub helper: u32,
    pub write: bool,
}

/// Who a program runs as, and what it gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPlan {
    pub level: Level,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    /// The helper's name, at `uid`.
    pub helper: Option<String>,
    /// The ACL to grant before the start, or None.
    pub grant: Option<Grant>,
    /// The environment before the values.
    pub env: Vec<String>,
}

/// The user and groups of the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ids {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

/// The inputs of a plan.
pub struct PlanInput<'a> {
    pub level: Level,
    pub write: bool,
    pub caller: &'a Ids,
    /// The helper of the user, from the registry.
    pub helper: Option<&'a Helper>,
    /// The caller is itself a program at `uid` (its helper): the start
    /// runs at `uid` too, and grants nothing new.
    pub nested: bool,
    pub cwd: &'a Path,
    pub env: &'a [String],
    pub wrap: Option<&'a Path>,
    pub names: &'a [String],
    pub listed: &'a [String],
}

/// The plan of a start.
pub fn plan(i: &PlanInput) -> Result<StartPlan, String> {
    let level = if i.nested { Level::Uid } else { i.level };
    if level == Level::Refs {
        return Ok(StartPlan {
            level,
            uid: i.caller.uid,
            gid: i.caller.gid,
            groups: i.caller.groups.clone(),
            helper: None,
            grant: None,
            env: i.env.to_vec(),
        });
    }
    let Some(h) = i.helper else {
        return Err(
            "its entry has protect = \"uid\" and your helper account does not exist yet: run sudo paddock tack setup (or set protect = \"refs\" in the entry)"
                .into(),
        );
    };
    if !i.nested && h.owner != i.caller.uid {
        return Err(format!(
            "the helper account {} belongs to uid {}, not to you: run sudo paddock tack setup",
            h.name, h.owner
        ));
    }
    if h.uid == 0 || h.uid == i.caller.uid && !i.nested {
        return Err(format!(
            "the helper account {} has uid {}: run sudo paddock tack setup",
            h.name, h.uid
        ));
    }
    let env = helper_env(
        i.env,
        &HelperEnv {
            helper: h,
            wrap: i.wrap,
            names: i.names,
            listed: i.listed,
        },
    );
    Ok(StartPlan {
        level,
        uid: h.uid,
        gid: h.gid,
        groups: vec![h.gid],
        helper: Some(h.name.clone()),
        grant: (!i.nested).then(|| Grant {
            folder: i.cwd.to_path_buf(),
            owner: i.caller.uid,
            helper: h.uid,
            write: i.write,
        }),
        env,
    })
}

/// Who a started program runs as, for the audit and the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsAs {
    pub level: Level,
    pub uid: u32,
    pub text: String,
}

/// The text of a start for the audit and the stand-in.
pub fn describe(p: &StartPlan) -> String {
    match (p.level, &p.helper) {
        (Level::Uid, Some(name)) => {
            format!("protect = uid: runs as {name} (uid {}); {UID_GAIN}", p.uid)
        }
        _ => format!("protect = refs: runs as uid {}; {REFS_LIMIT}", p.uid),
    }
}

#[cfg(test)]
#[path = "protect_tests.rs"]
mod tests;
