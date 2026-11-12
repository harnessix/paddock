//! Notifications for approval cards.

use super::*;
use crate::broker::notify;
use crate::state;

/// The notification for a card with fixed choices, when the bar does not
/// run: one button per choice, and Stop.
pub(super) fn notify_choices(env: &HostEnv, p: &Pending) -> Option<notify::Handle> {
    if approval_ui_active(env) {
        return None;
    }
    let mut actions: Vec<(String, String)> = p
        .choices
        .iter()
        .enumerate()
        .map(|(i, c)| (format!("choice:{i}"), format!("+{c}")))
        .collect();
    actions.push(("deny".into(), "Stop".into()));
    let note = notify::Note {
        summary: format!("paddock: {}", p.summary),
        body: p.detail.clone(),
        actions,
        critical: true,
    };
    let (env2, id, choices) = (env.clone(), p.id.clone(), p.choices.clone());
    let on_action = move |key: &str| {
        if let Some(c) = key
            .strip_prefix("choice:")
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| choices.get(i))
        {
            let _ = answer_reply(&env2, &id, c);
        } else if key == "deny" {
            let _ = answer(&env2, &id, Answer::Deny);
        }
    };
    match notify::ask(env, &note, on_action) {
        Ok(h) => Some(h),
        Err(e) => {
            log::debug!("cannot show the card as a notification: {e:#}");
            None
        }
    }
}

/// A desktop notification with buttons. Its answer goes through the same
/// answer file, so the panel, the CLI and the notification never disagree.
/// The bar plugin's approval service writes the Unix time to this file every
/// few seconds while it runs. Then it shows the request itself, and a second
/// popup would only repeat it.
pub fn approval_ui_file(env: &HostEnv) -> PathBuf {
    env.paddock_run().join("approval-ui")
}

pub fn approval_ui_active(env: &HostEnv) -> bool {
    std::fs::read_to_string(approval_ui_file(env))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .is_some_and(|t| state::now().saturating_sub(t) < 15)
}

/// The fallback prompt when the bar does not run. The request detail can
/// hold clipboard text, so it goes over the user's session bus, never into
/// the arguments of a process, which every local user can read.
pub fn approval_note(p: &Pending) -> notify::Note {
    let mut actions = vec![("once".to_string(), "Allow once".to_string())];
    if p.session_allowed {
        actions.push(("session".into(), "Allow for this session".into()));
    }
    actions.push(("deny".into(), "Deny".into()));
    if p.session_allowed {
        actions.push(("deny-session".into(), "Deny for this session".into()));
    }
    notify::Note {
        summary: format!("paddock: {} asks for {}", p.harness, p.capability),
        body: {
            let mut b = match crate::spawn::chain_text(&p.started_by) {
                Some(c) => format!("{c}\n\n{}", p.detail),
                None => p.detail.clone(),
            };
            if !p.reason.is_empty() {
                b.push_str(&format!("\n\nReason: {}", p.reason));
            }
            b
        },
        actions,
        critical: true,
    }
}

pub(super) fn notify_with_actions(env: &HostEnv, p: &Pending) -> Option<notify::Handle> {
    if approval_ui_active(env) {
        return None;
    }
    let (env2, id) = (env.clone(), p.id.clone());
    let on_action = move |key: &str| {
        if let Ok(a) = key.parse::<Answer>() {
            let _ = answer(&env2, &id, a);
        }
    };
    match notify::ask(env, &approval_note(p), on_action) {
        Ok(h) => Some(h),
        Err(e) => {
            log::debug!("cannot show the approval notification: {e:#}");
            None
        }
    }
}
