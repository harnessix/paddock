//! Create rules from seeds and selected answers.

use super::*;

/// A new rule. The id names the scope: `session-xxxxxx` for a session
/// rule, `rule-xxxxxx` for one of the file. `now` is Unix seconds.
#[allow(clippy::too_many_arguments)]
pub fn new_rule(
    app: &str,
    team: Option<&str>,
    host: &str,
    port: Port,
    verdict: Verdict,
    scope: Scope,
    by: &str,
    now: u64,
) -> Rule {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let hash = hash6(&[
        app,
        team.unwrap_or(""),
        host,
        &port.text(),
        verdict.word(),
        &now.to_string(),
        &nanos.to_string(),
        &n.to_string(),
        &std::process::id().to_string(),
    ]);
    let prefix = match scope {
        Scope::Session => "session",
        Scope::Always => "rule",
    };
    Rule {
        id: format!("{prefix}-{hash}"), // not user text
        app: app.to_string(),
        team: team.map(str::to_string),
        host: norm_host(host),
        port,
        verdict,
        scope,
        made: crate::waiting::format_rfc3339(now),
        expires: None,
        by: by.to_string(),
    }
}

/// A seed rule of the profile: every process of the session may reach
/// the host on port 443. Its id comes from the host, so a seed is the
/// same across starts and shows the same in `paddock app rules`.
pub fn seed_rule(host: &str, made: u64) -> Rule {
    Rule {
        id: format!("seed-{}", hash6(&[host])), // not user text
        app: "*".into(),
        team: None,
        host: norm_host(host),
        port: Port::Num(443),
        verdict: Verdict::Allow,
        scope: Scope::Always,
        made: crate::waiting::format_rfc3339(made),
        expires: None,
        by: BY_PROFILE.into(),
    }
}

/// The rule that an answer of the panel makes, for the exact app, host
/// and port of the question: `allow-session` a session rule, `allow-always`
/// and `deny-always` a rule of the file, `allow-once` and `deny` none.
pub fn rule_of_answer(answer: ConnectAnswer, conn: &Conn, by: &str, now: u64) -> Option<Rule> {
    let (verdict, scope) = match answer {
        ConnectAnswer::AllowOnce | ConnectAnswer::Deny => return None,
        ConnectAnswer::AllowSession | ConnectAnswer::AllowEverything => {
            (Verdict::Allow, Scope::Session)
        }
        ConnectAnswer::AllowSite => (Verdict::Allow, Scope::Always),
        ConnectAnswer::AllowAlways => (Verdict::Allow, Scope::Always),
        ConnectAnswer::DenyAlways => (Verdict::Deny, Scope::Always),
    };
    // A helper's connection gets the rule of its app, so the next helper
    // of the same app is covered too.
    let app = conn.parent_app.unwrap_or(conn.app);
    let site = site::registrable_site(conn.host);
    let host = match answer {
        ConnectAnswer::AllowSite => site.as_deref().unwrap_or(conn.host),
        ConnectAnswer::AllowEverything => "*",
        _ => conn.host,
    };
    let port = if answer == ConnectAnswer::AllowEverything
        || (answer == ConnectAnswer::AllowSite && site.is_some())
    {
        Port::Any
    } else {
        Port::Num(conn.port)
    };
    let mut rule = new_rule(app, conn.team, host, port, verdict, scope, by, now);
    if answer == ConnectAnswer::AllowEverything {
        rule.expires = Some(now.saturating_add(600));
    }
    Some(rule)
}
