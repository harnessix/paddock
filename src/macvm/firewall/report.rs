//! Rule and seed descriptions for callers.

use super::*;

/// The seed hosts of an app profile, or none.
pub fn seed_hosts(profile: Option<&crate::profile::Profile>) -> Vec<String> {
    profile
        .and_then(|p| p.app.as_ref())
        .and_then(|a| a.macos.as_ref())
        .map(|m| m.seed_hosts.clone())
        .unwrap_or_default()
}

/// The line of a rule for `paddock app rules NAME list`.
pub fn rule_line(r: &Rule) -> String {
    let mut line = text::rule_line(
        &r.id,
        r.verdict.word(),
        &r.app,
        &r.host,
        &r.port.text(),
        r.scope.word(),
        &r.by,
        &r.made,
    );
    if let Some(expires) = r.expires {
        line.push_str(&text::rule_time_left(
            expires.saturating_sub(crate::state::now()),
        ));
    }
    line
}

/// The rules file, when there is one, for the report and the reset.
pub fn rules_file_if_any(env: &HostEnv, app: &str) -> Option<PathBuf> {
    let p = Rules::path(env, app);
    p.is_file().then_some(p)
}
