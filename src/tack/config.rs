//! The `[tack]` table of the user configuration.
//!
//! ```toml
//! [tack.secrets.GH_TOKEN]
//! backend = "keyring"              # or "command", with command = [...]
//! hosts = ["api.github.com", "github.com"]
//!
//! [[tack.secrets.GH_TOKEN.allow]]
//! program = "/usr/bin/gh"
//! sha256 = "..."                   # optional: the content of the program
//! signer = "Developer ID ..."      # optional, macOS: the signing identity
//! args = ["api **", "pr *"]        # optional: the allowed argument shapes
//! protect = "uid"                  # or "refs"; the default comes from the
//!                                  # built-in profile (protect.rs)
//! write = true                     # protect = "uid": may write the folder
//!
//! [[tack.secrets.GH_TOKEN.callers]]
//! kind = "sandbox"                 # or "agent", or program = "/path"
//! action = "refuse"                # or "ask", or "allow"
//! ```
//!
//! Without a rule for a kind, a paddock sandbox or a coding agent among the
//! callers is refused. A rule with `action = "allow"` lifts that.
//!
//! Only the user configuration has this table. A project `paddock.toml`
//! that names it is refused (manifest.rs), because a project file is inside
//! the writable project and an agent can edit it.

use super::program;
use super::reference::valid_name;
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tack {
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretCfg>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The system keyring: the Secret Service (`secret-tool`) on Linux,
    /// the login keychain (`security`) on macOS.
    #[default]
    Keyring,
    /// The output of `command`, run on the host: a password manager.
    Command,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Keyring => "keyring",
            Backend::Command => "command",
        }
    }
}

/// One `[tack.secrets.NAME]` table. NAME is the environment variable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretCfg {
    #[serde(default)]
    pub backend: Backend,
    /// backend = "command": the program and its arguments. It prints the
    /// value on its standard output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// The programs that receive the value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<Allow>,
    /// Rules on the processes that start an allowlisted program.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub callers: Vec<Caller>,
    /// The hosts that a program with the value may connect to. Empty: no
    /// limit from tack.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

/// One allowlisted program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allow {
    pub program: PathBuf,
    /// The SHA-256 of the program file, in lower-case hex. When it is set,
    /// a changed program does not receive the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// macOS: the signing identity of the program (`codesign -dv`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// The allowed argument shapes, word by word as for host tools
    /// (`*` one word, `**` the rest; neither matches a word that starts
    /// with `-`). Empty: any arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// How tackd protects the program from the other processes of the
    /// user. None: the default of its built-in profile
    /// (`super::protect::default_level`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protect: Option<Level>,
    /// protect = "uid": the program may write in the working folder, not
    /// only read it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub write: bool,
}

pub use super::protect::Level;

impl Allow {
    /// The protection level of this entry: its own, or its default.
    pub fn level(&self) -> Level {
        self.protect
            .unwrap_or_else(|| super::protect::default_level(&self.program))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallerKind {
    /// A paddock sandbox is among the callers.
    Sandbox,
    /// A coding agent that paddock knows is among the callers.
    Agent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    #[default]
    Refuse,
    Ask,
    /// Only for a kind: a sandbox or an agent among the callers is fine.
    Allow,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Refuse => "refuse",
            Action::Ask => "ask",
            Action::Allow => "allow",
        }
    }
}

/// One rule on the chain of processes that starts an allowlisted program.
/// It names either a kind of caller or a caller program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CallerKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
    #[serde(default)]
    pub action: Action,
}

impl SecretCfg {
    pub fn source(&self) -> String {
        match self.backend {
            Backend::Keyring => "system keyring".into(),
            Backend::Command => format!(
                "command {}",
                self.command.first().map(String::as_str).unwrap_or("")
            ),
        }
    }
}

/// The user configuration file.
pub fn file(env: &HostEnv) -> PathBuf {
    env.paddock_config().join("config.toml")
}

/// `[tack]` of the user configuration. No file, or no table: nothing.
pub fn load(env: &HostEnv) -> Result<Tack> {
    let f = file(env);
    let text = match std::fs::read_to_string(&f) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Tack::default()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", f.display())),
    };
    parse(&text).with_context(|| format!("[tack] in {} is invalid", f.display()))
}

/// `[tack]` of a configuration text, checked.
pub fn parse(text: &str) -> Result<Tack> {
    let v: toml::Value = toml::from_str(text)?;
    let Some(t) = v.get("tack") else {
        return Ok(Tack::default());
    };
    let tack: Tack = t.clone().try_into()?;
    let problems = check(&tack);
    match problems.len() {
        0 => Ok(tack),
        1 => bail!("{}", problems[0]),
        n => bail!("{n} problems:\n  {}", problems.join("\n  ")),
    }
}

/// Every problem of a `[tack]` table, one line each.
pub fn check(t: &Tack) -> Vec<String> {
    let mut out = Vec::new();
    for (name, s) in &t.secrets {
        let at = format!("[tack.secrets.{name}]");
        if !valid_name(name) {
            out.push(format!("{at}: '{name}' is not an environment variable name: use letters, digits and '_', not starting with a digit"));
        }
        match s.backend {
            Backend::Command if s.command.is_empty() => out.push(format!(
                "{at}: backend = \"command\" needs command, the program that prints the value, such as [\"op\", \"read\", \"op://Private/GitHub/token\"]"
            )),
            Backend::Command if s.command[0].is_empty() => {
                out.push(format!("{at}: the first word of command is empty"))
            }
            Backend::Keyring if !s.command.is_empty() => out.push(format!(
                "{at}: command is only for backend = \"command\"; set backend = \"command\" to use it"
            )),
            _ => {}
        }
        let mut seen = Vec::new();
        for (i, a) in s.allow.iter().enumerate() {
            let at = format!("{at} allow entry {}", i + 1);
            let p = &a.program;
            if !program::valid_path(p) {
                out.push(format!(
                    "{at}: program '{}' must be an absolute path without . or .. parts, such as \"/usr/bin/gh\"",
                    p.display()
                ));
                continue;
            }
            if let Some(why) = program::refused(p) {
                out.push(format!("{at}: {} cannot be allowlisted: {why}. It would give the value to any command it runs; allowlist the program that needs the value instead", p.display()));
            }
            if let Some(h) = &a.sha256 {
                if !program::valid_sha256(h) {
                    out.push(format!(
                        "{at}: sha256 must be 64 lower-case hex digits (sha256sum {})",
                        p.display()
                    ));
                }
            }
            if a.signer.as_deref().is_some_and(|s| s.trim().is_empty()) {
                out.push(format!("{at}: signer is empty"));
            }
            for shape in &a.args {
                if let Some(why) = bad_shape(shape) {
                    out.push(format!("{at}: args {shape:?}: {why}"));
                }
            }
            if a.write && a.level() != Level::Uid {
                out.push(format!(
                    "{at}: write = true is only for protect = \"uid\" (a program at protect = \"{}\" runs as you and writes what you can)",
                    a.level().name()
                ));
            }
            if seen.contains(&p) {
                out.push(format!("{at}: {} is in the list twice", p.display()));
            }
            seen.push(p);
        }
        for (i, c) in s.callers.iter().enumerate() {
            let at = format!("{at} callers entry {}", i + 1);
            match (&c.kind, &c.program) {
                (Some(_), Some(_)) | (None, None) => out.push(format!(
                    "{at}: name either kind (\"sandbox\" or \"agent\") or program, not both and not neither"
                )),
                (None, Some(p)) if !program::valid_path(p) => out.push(format!(
                    "{at}: program '{}' must be an absolute path",
                    p.display()
                )),
                (None, Some(_)) if c.action == Action::Allow => out.push(format!(
                    "{at}: action = \"allow\" is only for a kind; a caller program is allowed when no rule names it"
                )),
                _ => {}
            }
        }
        for h in &s.hosts {
            if !valid_host(h) {
                out.push(format!(
                    "{at}: '{h}' is not a host: use HOST, HOST:PORT or *.DOMAIN"
                ));
            }
        }
    }
    out.extend(super::protect::conflicts(t));
    out
}

/// Why an argument shape is invalid, or None. The same words as host tools.
pub fn bad_shape(shape: &str) -> Option<&'static str> {
    let words: Vec<&str> = shape.split_whitespace().collect();
    if let Some(i) = words.iter().position(|w| *w == "**") {
        if i + 1 != words.len() {
            return Some("** is allowed only as the last word");
        }
    }
    if words
        .iter()
        .any(|w| w.contains('*') && *w != "*" && *w != "**")
    {
        return Some("* and ** stand alone as whole words");
    }
    None
}

/// HOST, HOST:PORT, or `*.DOMAIN` for every host under DOMAIN.
pub fn valid_host(h: &str) -> bool {
    let (host, port) = match h.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (h, None),
    };
    if port.is_some_and(|p| p.parse::<u16>().map_or(true, |n| n == 0)) {
        return false;
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[tack.secrets.GH_TOKEN]
hosts = ["api.github.com", "github.com:443", "*.githubusercontent.com"]
[[tack.secrets.GH_TOKEN.allow]]
program = "/usr/bin/gh"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
[[tack.secrets.GH_TOKEN.allow]]
program = "/usr/bin/git"
signer = "Developer ID Application: Example (ABCDE12345)"
[[tack.secrets.GH_TOKEN.callers]]
kind = "sandbox"
[[tack.secrets.GH_TOKEN.callers]]
program = "/usr/bin/claude"
action = "ask"

[tack.secrets.OPENAI_API_KEY]
backend = "command"
command = ["op", "read", "op://Private/OpenAI/key"]
"#;

    #[test]
    fn parses_the_documented_form() {
        let t = parse(GOOD).unwrap();
        let gh = &t.secrets["GH_TOKEN"];
        assert_eq!(gh.backend, Backend::Keyring);
        assert_eq!(gh.allow.len(), 2);
        assert_eq!(gh.callers[0].kind, Some(CallerKind::Sandbox));
        assert_eq!(gh.callers[0].action, Action::Refuse);
        assert_eq!(gh.callers[1].action, Action::Ask);
        let oa = &t.secrets["OPENAI_API_KEY"];
        assert_eq!(oa.backend, Backend::Command);
        assert_eq!(oa.source(), "command op");
        assert_eq!(parse("").unwrap(), Tack::default());
        assert_eq!(parse("[tack]\n").unwrap(), Tack::default());
    }

    fn err(text: &str) -> String {
        format!("{:#}", parse(text).unwrap_err())
    }

    #[test]
    fn shells_interpreters_env_and_printenv_cannot_be_allowlisted() {
        for p in [
            "/bin/bash",
            "/usr/bin/python3",
            "/usr/bin/env",
            "/usr/bin/printenv",
        ] {
            let e = err(&format!(
                "[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"{p}\"\n"
            ));
            assert!(e.contains("cannot be allowlisted"), "{p}: {e}");
            assert!(e.contains(p), "{e}");
        }
    }

    #[test]
    fn every_problem_is_named_with_its_place() {
        let e = err(r#"
[tack.secrets.A]
backend = "command"
hosts = ["https://x", "a:0", "-a.com"]
[[tack.secrets.A.allow]]
program = "gh"
[[tack.secrets.A.allow]]
program = "/usr/bin/gh"
sha256 = "ABC"
[[tack.secrets.A.allow]]
program = "/usr/bin/gh"
args = ["a ** b", "x*"]
[[tack.secrets.A.callers]]
kind = "sandbox"
program = "/usr/bin/x"
[[tack.secrets.A.callers]]
program = "/usr/bin/y"
action = "allow"
[tack.secrets.B]
command = ["x"]
"#);
        for want in [
            "12 problems",
            "allow entry 3: args \"a ** b\": ** is allowed only as the last word",
            "allow entry 3: args \"x*\": * and ** stand alone",
            "callers entry 2: action = \"allow\" is only for a kind",
            "[tack.secrets.A]: backend = \"command\" needs command",
            "allow entry 1: program 'gh' must be an absolute path",
            "allow entry 2: sha256 must be 64",
            "allow entry 3: /usr/bin/gh is in the list twice",
            "callers entry 1: name either kind",
            "'https://x' is not a host",
            "'a:0' is not a host",
            "'-a.com' is not a host",
            "[tack.secrets.B]: command is only for backend = \"command\"",
        ] {
            assert!(e.contains(want), "missing {want:?} in:\n{e}");
        }
    }

    #[test]
    fn unknown_fields_and_bad_names_are_refused() {
        assert!(err("[tack.secrets.A]\nallowlist = []\n").contains("unknown field"));
        assert!(err("[tack]\nsecret = {}\n").contains("unknown field"));
        assert!(err("[tack.secrets.A]\nbackend = \"file\"\n").contains("unknown variant"));
        assert!(err("[tack.secrets.\"1A\"]\n").contains("not an environment variable name"));
    }

    #[test]
    fn load_reads_the_user_file_and_names_it_in_errors() {
        let t = tempfile::tempdir().unwrap();
        let env = crate::privfs::tests::env_in(t.path());
        assert_eq!(load(&env).unwrap(), Tack::default());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(file(&env), GOOD).unwrap();
        assert_eq!(load(&env).unwrap().secrets.len(), 2);
        std::fs::write(file(&env), "[tack.secrets.A]\nbackend = 1\n").unwrap();
        let e = format!("{:#}", load(&env).unwrap_err());
        assert!(e.contains("config.toml") && e.contains("[tack]"), "{e}");
    }
}
