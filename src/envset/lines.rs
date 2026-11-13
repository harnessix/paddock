//! Plain lines: `paddock env show`, and the TUI with --plain or a narrow
//! terminal. Each line fits in 80 columns; each state is a word.

use super::*;

/// The widest line.
pub const COLUMNS: usize = 80;

/// A name cut to `max` characters, with `…` at the end.
pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Words in lines of at most `COLUMNS`, the first with `head`, the rest
/// with `indent`.
fn wrap(head: &str, indent: &str, words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = head.to_string();
    let mut empty = true;
    for w in words {
        let w = cut(w, COLUMNS - indent.chars().count() - 1);
        let need = line.chars().count() + usize::from(!empty) + w.chars().count();
        if !empty && need > COLUMNS {
            out.push(line);
            line = indent.to_string();
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(&w);
        empty = false;
    }
    out.push(line);
    out
}

fn row(on: bool, name: &str, detail: &str) -> String {
    format!(
        "  {} {:<40} {}",
        if on { "[x]" } else { "[ ]" },
        cut(name, 40),
        cut(detail, COLUMNS - 47)
    )
    .trim_end()
    .to_string()
}

fn scope_words(scope: Option<&str>) -> &'static str {
    match scope {
        Some("agent") => "this agent",
        Some("project") => "this project",
        Some("file") => "paddock.toml",
        _ => "all agents",
    }
}

/// "214 requests, 2m ago".
pub fn ago(now: u64, t: u64) -> String {
    let d = now.saturating_sub(t);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86_400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86_400)
    }
}

/// The state words of a key in the TUI.
pub fn key_words(state: &str) -> &'static str {
    match state {
        "kept_on_host" => "kept on host",
        "handed_over" => "handed over",
        "agent" => "agent channel",
        _ => "not available",
    }
}

/// The view as plain lines.
pub fn plain_lines(v: &View) -> Vec<String> {
    let now = crate::state::now();
    let mut out = wrap(
        "",
        "",
        &v.summary.split(' ').map(str::to_string).collect::<Vec<_>>(),
    );
    out.push(format!("mode: {} ({})", v.mode.name(), v.mode_origin));
    if let Some(r) = &v.review {
        out.extend(wrap(
            "",
            "",
            &format!(
                "Host variables pass unseen. Review them once: paddock env migrate. From paddock {}, only the lists pass.",
                r.forced_from
            )
            .split(' ')
            .map(str::to_string)
            .collect::<Vec<_>>(),
        ));
    }
    for g in &v.groups {
        out.push(String::new());
        match g.id.as_str() {
            "keys" => {
                out.push(format!("{} ({})", g.title, g.keys.len()));
                if g.keys.is_empty() {
                    out.push(
                        "  No keys yet. An agent that signs in with your account needs none."
                            .into(),
                    );
                }
                for k in &g.keys {
                    let grant = match k.grant.as_str() {
                        "any" | "project" | "standing" => {
                            if k.uses > 0 {
                                format!(
                                    "{} {}{}",
                                    k.uses,
                                    k.unit,
                                    k.last_used
                                        .map(|t| format!(", {}", ago(now, t)))
                                        .unwrap_or_default()
                                )
                            } else {
                                "granted".to_string()
                            }
                        }
                        "ask" => "asks once here".into(),
                        "agent" => "no grant here".into(),
                        _ => "for the agents it names".into(),
                    };
                    out.push(
                        format!(
                            "  {} {:<20} {:<16} {}",
                            if k.granted { "[x]" } else { "[ ]" },
                            cut(&k.name, 20),
                            key_words(&k.state),
                            cut(&grant, COLUMNS - 44)
                        )
                        .trim_end()
                        .to_string(),
                    );
                }
            }
            "host" => {
                let n = g.count.unwrap_or(0);
                if g.passes == Some(true) {
                    out.push(format!("{} ({n} pass unseen)", g.title));
                } else {
                    out.push(format!("{} ({n} stay here)", g.title));
                }
                if !g.suggested.is_empty() {
                    out.extend(wrap("  suggested:", "    ", &g.suggested));
                }
            }
            _ => {
                out.push(format!("{} ({})", g.title, g.items.len()));
                if g.id == "yours" && g.items.is_empty() {
                    out.push(
                        "  Nothing added. Add a variable that your tools need, such as JAVA_HOME."
                            .into(),
                    );
                }
                for i in &g.items {
                    let name = match &i.value {
                        Some(val) => format!("{}={val}", i.name),
                        None => i.name.clone(),
                    };
                    let detail = match (g.id.as_str(), i.kind.as_deref()) {
                        ("yours", Some("pass")) => {
                            let base = format!("from host, {}", scope_words(i.scope.as_deref()));
                            match i.reason.as_deref() {
                                Some("not_set") => format!("{base}, not set"),
                                Some("held") => "held".to_string(),
                                Some("key") => "a key: see Keys".to_string(),
                                _ => base,
                            }
                        }
                        ("yours", _) => format!("fixed, {}", scope_words(i.scope.as_deref())),
                        _ if i.handed_over => "login, handed over".to_string(),
                        _ => i.origin.clone(),
                    };
                    out.push(row(i.passes, &name, &detail));
                }
            }
        }
    }
    if !v.held.is_empty() {
        out.push(String::new());
        let names: Vec<String> = v.held.iter().map(|h| h.name.clone()).collect();
        out.extend(wrap(&format!("Held ({}):", v.held.len()), "  ", &names));
    }
    out.push(String::new());
    out.push(v.note.to_string());
    out
}

/// A run record as plain lines: "What it got at 10:02".
pub fn record_lines(r: &Record) -> Vec<String> {
    let mut out = vec![format!(
        "What {} got at {} in {} ({})",
        r.agent,
        crate::storage::date(r.started),
        cut(&r.project.display().to_string(), 30),
        r.mode.name()
    )];
    for g in &r.groups {
        out.push(String::new());
        if g.id == "host" && g.passes != Some(true) {
            out.push(format!("{} (stayed there)", g.title));
            continue;
        }
        out.push(format!("{} ({})", g.title, g.items.len()));
        for i in &g.items {
            out.push(row(true, &i.name, &i.origin));
        }
    }
    if !r.held.is_empty() {
        out.push(String::new());
        out.push(format!("Held back ({})", r.held.len()));
        for h in &r.held {
            out.push(row(false, &h.name, &format!("{}: {}", h.rule, h.reason)));
        }
    }
    out.push(String::new());
    out.push("A running agent keeps its variables. Changes apply at the next start.".into());
    out
}
