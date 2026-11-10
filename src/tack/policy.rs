//! The policy engine: may this program, started by this chain of callers,
//! receive which secrets?
//!
//! The inputs come from the kernel (the peer's uid and pid, the process
//! table), from the file system (the program file), and from the user
//! configuration of the peer's uid. The request itself only names the
//! program, its arguments and its working folder; a reason never repeats an
//! argument, because an argument can hold a value.
//!
//! Rules, in order:
//!
//! 1. Root asks for nothing: tackd gives no secret to uid 0.
//! 2. The caller pid in the request is the pid that the kernel reports.
//! 3. The calling process is still there, and is of the peer's uid.
//! 4. The program is a shell, interpreter, `env` or `printenv`: refused.
//! 5. Each secret whose allowlist names the program (by its real path):
//!    - the program's SHA-256 matches the entry, when the entry has one;
//!    - on macOS the signer matches, when the entry has one;
//!    - an entry with neither needs a program that only root can change;
//!    - the arguments match one of the entry's shapes, when it has any;
//!    - the caller rules: a sandbox or an agent among the callers is
//!      refused unless a rule for its kind says otherwise; a rule on a
//!      caller program applies when that program is among the callers.
//!      The strictest rule wins (refuse, then ask, then allow).
//! 6. The program receives the secrets that pass. None pass: deny (or
//!    ask, when a rule asks).

use super::config::{Action, Allow, CallerKind, SecretCfg, Tack};
use super::ident::{Ident, IdentError};
use super::procs::Chain;
use super::proto::{Decision, Launch, Program, Verdict};
use crate::hosttools::matches;
use std::collections::BTreeMap;
use std::path::Path;

/// Who asks, from the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub pid: u32,
}

/// Everything the engine needs, gathered by the caller (tackd or a test).
pub struct Input<'a> {
    pub peer: Peer,
    pub req: &'a Launch,
    /// The `[tack]` table of the peer's user configuration.
    pub cfg: Result<&'a Tack, String>,
    /// The caller chain from the peer's pid; None: the process ended.
    pub chain: Option<&'a Chain>,
    pub ident: Result<&'a Ident, IdentError>,
    /// The verified signer of a program file (macOS), or None.
    pub signer: &'a dyn Fn(&Path) -> Option<String>,
}

fn deny(reason: impl Into<String>, chain: Option<&Chain>, program: Option<Program>) -> Decision {
    Decision {
        verdict: Verdict::Deny,
        reason: reason.into(),
        secrets: Vec::new(),
        hosts: BTreeMap::new(),
        program,
        chain: chain.map(Chain::summary).unwrap_or_default(),
    }
}

/// The outcome for one secret.
#[derive(Debug, Clone, PartialEq, Eq)]
enum One {
    Allow,
    Ask(String),
    Deny(String),
}

impl One {
    fn rank(&self) -> u8 {
        match self {
            One::Allow => 0,
            One::Ask(_) => 1,
            One::Deny(_) => 2,
        }
    }
}

fn rank(a: Action) -> u8 {
    match a {
        Action::Allow => 0,
        Action::Ask => 1,
        Action::Refuse => 2,
    }
}

/// The caller rules of one secret against the chain.
fn callers(s: &SecretCfg, chain: &Chain) -> One {
    if let Some(c) = &chain.cut {
        return One::Deny(format!(
            "the caller chain could not be read to its end ({c})"
        ));
    }
    let mut worst: Option<(Action, String)> = None;
    let mut take = |a: Action, why: String| {
        if worst.as_ref().is_none_or(|(w, _)| rank(a) > rank(*w)) {
            worst = Some((a, why));
        }
    };
    let kind_rule = |k: CallerKind| {
        s.callers
            .iter()
            .filter(|c| c.kind == Some(k))
            .map(|c| c.action)
            .max_by_key(|a| rank(*a))
    };
    if chain.sandbox() {
        let a = kind_rule(CallerKind::Sandbox).unwrap_or(Action::Refuse);
        take(a, "a sandbox is among the callers".into());
    }
    if let Some(agent) = chain.agent() {
        let a = kind_rule(CallerKind::Agent).unwrap_or(Action::Refuse);
        take(a, format!("the coding agent {agent} is among the callers"));
    }
    for c in &s.callers {
        if let Some(p) = &c.program {
            if chain.has_program(p) {
                take(c.action, format!("{} is among the callers", p.display()));
            }
        }
    }
    match worst {
        None | Some((Action::Allow, _)) => One::Allow,
        Some((Action::Ask, why)) => One::Ask(format!("{why}, and a caller rule asks")),
        Some((Action::Refuse, why)) => One::Deny(why),
    }
}

/// One allowlist entry against the program and the request.
fn entry(
    a: &Allow,
    s: &SecretCfg,
    id: &Ident,
    req: &Launch,
    chain: &Chain,
    signer: &dyn Fn(&Path) -> Option<String>,
) -> One {
    let mut pinned = false;
    if let Some(want) = &a.sha256 {
        if *want != id.sha256 {
            return One::Deny(format!(
                "{} changed since its sha256 was written (now {}): if the update is yours, put the new hash in the configuration",
                id.real.display(),
                id.sha256
            ));
        }
        pinned = true;
    }
    if let Some(want) = a.signer.as_deref().filter(|_| cfg!(target_os = "macos")) {
        match signer(&id.real) {
            Some(got) if got == want => pinned = true,
            Some(got) => {
                return One::Deny(format!(
                    "{} is signed by {got:?}, not by {want:?}",
                    id.real.display()
                ))
            }
            None => {
                return One::Deny(format!(
                    "{} has no valid signature, and its entry names a signer",
                    id.real.display()
                ))
            }
        }
    }
    if !pinned {
        if let Some(why) = &id.changeable {
            return One::Deny(format!(
                "{} can be replaced by a process that is not root ({why}): pin it with sha256 = \"{}\" in its allow entry",
                id.real.display(),
                id.sha256
            ));
        }
    }
    if !a.args.is_empty() && !a.args.iter().any(|shape| matches(shape, &req.args)) {
        return One::Deny(format!(
            "the arguments match no allowed shape of {} ({} shapes)",
            id.real.display(),
            a.args.len()
        ));
    }
    callers(s, chain)
}

/// Decide one launch request.
pub fn decide(i: &Input) -> Decision {
    let chain = i.chain;
    if i.peer.uid == 0 {
        return deny("tackd gives no secret to root", chain, None);
    }
    if i.req.caller_pid != i.peer.pid {
        return deny(
            format!(
                "the request names caller {} but the kernel reports {} for this connection: a process in another pid namespace, or a forged request",
                i.req.caller_pid, i.peer.pid
            ),
            chain,
            None,
        );
    }
    let Some(chain) = chain else {
        return deny(
            "the calling process ended before tackd could read it",
            None,
            None,
        );
    };
    match chain.links.first() {
        Some(l) if l.proc.uid == i.peer.uid => {}
        _ => {
            return deny(
                "the calling process changed its user while tackd read it",
                Some(chain),
                None,
            )
        }
    }
    let cfg = match &i.cfg {
        Ok(c) => *c,
        Err(e) => {
            return deny(
                format!("your [tack] configuration cannot be used: {e}"),
                Some(chain),
                None,
            )
        }
    };
    let named = |p: &Path| {
        cfg.secrets
            .values()
            .flat_map(|s| s.allow.iter())
            .any(|a| a.program == p)
    };
    let id = match &i.ident {
        Ok(id) => *id,
        // Only a program that the allowlist names gets a precise reason:
        // a client cannot probe other paths through tackd.
        Err(e) if named(Path::new(&i.req.program)) => {
            return deny(format!("{}: {e}", i.req.program), Some(chain), None)
        }
        Err(_) => return deny(not_listed(&i.req.program), Some(chain), None),
    };
    let program = Some(Program {
        path: id.path.display().to_string(),
        real: id.real.display().to_string(),
        sha256: id.sha256.clone(),
        dev: id.dev,
        ino: id.ino,
        signer: None,
    });
    let mut outcomes: BTreeMap<String, One> = BTreeMap::new();
    for (name, s) in &cfg.secrets {
        for a in &s.allow {
            if a.program != id.path && super::program::real_path(&a.program) != id.real {
                continue;
            }
            let o = entry(a, s, id, i.req, chain, i.signer);
            let better = outcomes.get(name).is_none_or(|old| o.rank() < old.rank());
            if better {
                outcomes.insert(name.clone(), o);
            }
        }
    }
    if outcomes.is_empty() {
        // Nothing about a file that no entry names: not its hash, not
        // that it exists.
        return deny(not_listed(&i.req.program), Some(chain), None);
    }
    // The configuration check refuses these by name; a link that now
    // points to a shell is refused here.
    if let Some(why) =
        super::program::refused(&id.real).or_else(|| super::program::refused(&id.path))
    {
        return deny(
            format!("{why}: it cannot receive secrets, because it runs whatever it is told to"),
            Some(chain),
            program,
        );
    }
    let pick = |want: u8| -> Vec<String> {
        outcomes
            .iter()
            .filter(|(_, o)| o.rank() == want)
            .map(|(n, _)| n.clone())
            .collect()
    };
    let others: Vec<String> = outcomes
        .iter()
        .filter_map(|(n, o)| match o {
            One::Allow => None,
            One::Ask(w) | One::Deny(w) => Some(format!("{n}: {w}")),
        })
        .collect();
    let (verdict, secrets) = match (pick(0), pick(1)) {
        (a, _) if !a.is_empty() => (Verdict::Allow, a),
        (_, k) if !k.is_empty() => (Verdict::Ask, k),
        _ => (Verdict::Deny, Vec::new()),
    };
    let reason = match verdict {
        Verdict::Allow if others.is_empty() => format!("allowlisted for {}", secrets.join(", ")),
        Verdict::Allow => format!(
            "allowlisted for {}; not for {}",
            secrets.join(", "),
            others.join("; ")
        ),
        _ => others.join("; "),
    };
    let hosts = secrets
        .iter()
        .map(|n| (n.clone(), cfg.secrets[n].hosts.clone()))
        .collect();
    Decision {
        verdict,
        reason,
        secrets,
        hosts,
        program,
        chain: chain.summary(),
    }
}

/// Whether an allowlist entry names `path` or the file it resolves to.
/// tackd hashes a program only when this is true, so a client cannot make
/// it read any file it likes.
pub fn listed(cfg: &Tack, path: &Path) -> bool {
    let real = std::fs::canonicalize(path).ok();
    cfg.secrets.values().flat_map(|s| s.allow.iter()).any(|a| {
        a.program == path
            || real
                .as_ref()
                .is_some_and(|r| super::program::real_path(&a.program) == *r)
    })
}

fn not_listed(program: &str) -> String {
    format!("{program} is not on the allowlist of any secret")
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
