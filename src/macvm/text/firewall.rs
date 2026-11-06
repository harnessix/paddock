//! The texts of the firewall layer: the rules file and its CLI, the
//! question of mode `ask` as the approvals list shows it, and the lines
//! of the connection log.

use std::path::Path;

fn shown(p: &Path) -> String {
    p.display().to_string()
}

// ---- The rules file ------------------------------------------------------

/// The rules file does not parse.
pub fn rules_file_invalid(path: &Path, error: &str) -> String {
    format!(
        "the firewall rules file {} does not parse ({error}). Fix it, or remove it and add the rules again with paddock app rules",
        shown(path)
    )
}

/// The comment at the top of the rules file.
pub fn rules_file_header() -> String {
    "# The firewall rules of a desktop app in paddock's macOS guest.\n# paddock app rules NAME [list | add | remove] edits this file.\n".into()
}

/// A rule with `host = "*"` and `verdict = "allow"` without the flag
/// (security checklist item 19).
pub fn all_hosts_needs_flag() -> String {
    "a rule that allows every host (host = \"*\") turns the firewall off for that app. Add it with --yes-all-hosts if that is what you want".into()
}

/// A host of a rule that is no host name, no IP address and no glob.
pub fn bad_host_glob(host: &str) -> String {
    format!("the host {host:?} is not a host name, an IP address, * or *.name")
}

/// A port of a rule that is not a number or `*`.
pub fn bad_port(port: &str) -> String {
    format!("the port {port:?} is not a number from 1 to 65535 or *")
}

/// A verdict word that is not `allow` or `deny`.
pub fn bad_verdict(word: &str) -> String {
    format!("the verdict {word:?} is not allow or deny")
}

/// A scope word that is not `always` or `session`.
pub fn bad_scope(word: &str) -> String {
    format!("the scope {word:?} is not always or session")
}

/// `paddock app rules NAME remove ID` with an id that no rule has.
pub fn no_rule(id: &str, app: &str) -> String {
    format!("no rule {id} in the firewall rules of {app}. paddock app rules {app} list shows them")
}

/// `remove` of a seed rule: the file does not hold it.
pub fn rule_is_seed(id: &str, app: &str) -> String {
    format!(
        "{id} is a seed rule of the profile {app}, which the rules file does not hold. To block its host, add a deny rule for it: paddock app rules {app} add HOST --deny"
    )
}

/// `remove` of a session rule from the CLI: the session holds it.
pub fn rule_is_session(id: &str, app: &str) -> String {
    format!("{id} is a rule of the running session of {app}, which ends with the session. The rules file does not hold it")
}

/// The header of `paddock app rules NAME list`.
pub fn rules_header(app: &str, path: &Path) -> String {
    format!("Firewall rules of {app} ({}):", shown(path))
}

/// `paddock app rules NAME list` with no rule at all.
pub fn rules_empty(app: &str) -> String {
    format!("{app} has no firewall rules. In network mode ask, the first connection to each host asks. paddock app rules {app} add HOST --allow adds a rule")
}

/// One rule of the list: the id, the verdict, the app, the target, the
/// scope and who made it.
#[allow(clippy::too_many_arguments)]
pub fn rule_line(
    id: &str,
    verdict: &str,
    app: &str,
    host: &str,
    port: &str,
    scope: &str,
    by: &str,
    made: &str,
) -> String {
    format!("{id:<16} {verdict:<5} {app:<24} {host}:{port}  {scope}, by {by}, {made}")
}

/// A rule went into the file.
pub fn rule_added(id: &str, path: &Path) -> String {
    format!("Added rule {id} to {}", shown(path))
}

/// A rule left the file.
pub fn rule_removed(id: &str, path: &Path) -> String {
    format!("Removed rule {id} from {}", shown(path))
}

/// `paddock app rules` for a name that has no app profile on the macOS
/// guest route.
pub fn rules_need_macvm(app: &str) -> String {
    format!("firewall rules are for desktop apps in paddock's macOS guest. {app} is not one, or this is not a Mac")
}

// ---- The question --------------------------------------------------------

/// The one line of a `connect` question in `paddock approvals`.
pub fn ask_summary(app: &str, host: &str, port: u16) -> String {
    format!("{app} wants to connect to {host} port {port}")
}

/// The count on a question that more than one connection waits on.
pub fn ask_count(count: u64) -> String {
    match count {
        0 | 1 => String::new(),
        n => format!("{n} connections wait on this answer"),
    }
}

/// The help of `paddock approvals answer` for a `connect` question.
pub fn connect_answers() -> String {
    "a connect question takes --allow-once, --allow-session, --allow-always, --deny or --deny-always".into()
}

/// `paddock approvals answer ID --allow-always` and the others.
pub fn connect_answered(id: &str, answer: &str) -> String {
    format!("Answered {id}: {answer}")
}

// ---- The connection log --------------------------------------------------

/// One line of `paddock app log NAME`: the time, the verdict, the app,
/// the target, the rule, and for a closed connection its bytes and length.
#[allow(clippy::too_many_arguments)]
pub fn connection_line(
    time: &str,
    verdict: &str,
    app: &str,
    host: &str,
    port: u16,
    rule: &str,
    count: u64,
    bytes: Option<(u64, u64, u64)>,
) -> String {
    let mut s = format!("{time}  {verdict:<5} {app:<24} {host}:{port}  {rule}");
    if count > 1 {
        s.push_str(&format!("  x{count}"));
    }
    if let Some((up, down, secs)) = bytes {
        s.push_str(&format!("  {} up, {} down, {secs} s", kib(up), kib(down)));
    }
    s
}

/// A byte count for the log line: `512 B`, `12.3 KiB`, `1.2 MiB`.
pub fn kib(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// The reason in the 403 of a refused connection, for the app's log.
pub fn connection_refused(host: &str, port: u16, rule: &str) -> String {
    format!("paddock's firewall refused {host}:{port} ({rule})")
}

#[cfg(test)]
#[path = "firewall_tests.rs"]
mod tests;
