//! The notifications of an app: what it left in its worktree, the buttons
//! on that notification, and a start that failed.

use crate::build;
use crate::env::HostEnv;
use std::path::Path;

#[cfg(not(target_os = "macos"))]
/// After the app exits: print what it left in its worktree, and send a
/// notification when no terminal shows the text (a launcher start).
pub(super) fn report_worktree(env: &HostEnv, name: &str, w: &build::Worktree) {
    let (report, removed) = crate::worktree::finish(env, w);
    eprintln!("\n{report}");
    if removed || std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        return;
    }
    let (commits, changed) = build::worktree_counts(env, w);
    let note = worktree_note(name, w, commits, changed);
    // The Merge and Remove buttons. Without actions, only the text shows.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let shown = crate::broker::notify::ask(env, &note, move |key| {
        let _ = tx.send(key.to_string());
    });
    match shown {
        Ok(_handle) => {
            if let Ok(key) = rx.recv_timeout(std::time::Duration::from_secs(WORKTREE_ANSWER_SECS)) {
                desktop_notice(env, &worktree_action(env, &w.path, &key));
            }
        }
        Err(e) => log::debug!("no desktop notification: {e:#}"),
    }
}

#[cfg(not(target_os = "macos"))]
/// How long paddock waits for a button of the worktree notification.
const WORKTREE_ANSWER_SECS: u64 = 30 * 60;

/// Run the button that the user picked on the worktree notification, the
/// same as `paddock worktree merge` or `paddock worktree remove` (without
/// --force), and give back a notification with the result.
pub fn worktree_action(env: &HostEnv, path: &Path, key: &str) -> crate::broker::notify::Note {
    use crate::worktree;
    let all = worktree::list(env);
    let r = worktree::find(&all, &path.display().to_string()).and_then(|w| match key {
        "merge" => worktree::merge(env, &w),
        "remove" => worktree::remove(env, &w, false),
        _ => anyhow::bail!("unknown action {key}"),
    });
    match r {
        Ok(t) => crate::broker::notify::Note {
            what: "worktree".into(),
            summary: "paddock worktree".into(),
            body: t,
            ..Default::default()
        },
        Err(e) => crate::broker::notify::Note {
            what: "worktree".into(),
            summary: format!("paddock could not {key} the worktree"),
            body: format!("{e:#}"),
            ..Default::default()
        },
    }
}

/// The notification after an app worked in a worktree. The branch name and
/// the paths come from a repository that the sandbox wrote, so they go only
/// into the D-Bus message, never into the arguments of a host process.
pub fn worktree_note(
    name: &str,
    w: &build::Worktree,
    commits: usize,
    changed: Option<usize>,
) -> crate::broker::notify::Note {
    let changed = match changed {
        Some(n) => format!("{n}"),
        None => "an unknown number of".to_string(),
    };
    crate::broker::notify::Note {
        what: "worktree".into(),
        action: Some(w.path.display().to_string()),
        summary: format!("{name} worked on branch {}", w.branch),
        body: format!(
            "{commits} commits and {changed} uncommitted changes in {}. Review: git -C {} log {}",
            w.path.display(),
            w.repo.display(),
            w.branch
        ),
        actions: vec![
            ("merge".into(), "Merge".into()),
            ("remove".into(), "Remove".into()),
        ],
        ..Default::default()
    }
}

/// The notification when an app from a launcher entry cannot start.
pub fn start_error_note(name: &str, e: &anyhow::Error) -> crate::broker::notify::Note {
    crate::broker::notify::Note {
        what: "start_failed".into(),
        summary: format!("paddock could not start {name}"),
        body: format!("{e:#}"),
        ..Default::default()
    }
}

/// Show a notification on the desktop through the session bus. The text can
/// come from a sandbox: no process gets it as arguments, where a leading "-"
/// would read as an option and every local user could read it in /proc.
/// Without a session bus, the terminal text is all there is.
pub fn desktop_notice(env: &HostEnv, n: &crate::broker::notify::Note) {
    if let Err(e) = crate::broker::notify::show(env, n) {
        log::debug!("no desktop notification: {e:#}");
    }
}
