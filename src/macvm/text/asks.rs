//! The texts of the question queue of the firewall: the panel of one
//! question, the notice when a session asks too much, the lines of the
//! session log and the rows of `paddock app status`.

// ---- The panel of one question -------------------------------------------

/// The title of the panel: `ChatGPT wants to connect to api.example.com`.
pub fn ask_title(app: &str, host: &str) -> String {
    format!("{app} wants to connect to {host}")
}

/// The detail line under the title.
pub fn ask_detail(port: u16) -> String {
    format!("port {port} · first connection to this host")
}

/// The line of the questions that wait behind the panel, empty for none.
pub fn ask_more(more: u64) -> String {
    match more {
        0 => String::new(),
        n => format!("and {n} more waiting"),
    }
}

/// The accessibility label of a panel button: the button and the host.
pub fn ask_button_label(button: &str, host: &str) -> String {
    format!("{button} for {host}")
}

/// The title of the button that holds the answers past the first three.
pub fn ask_more_button() -> &'static str {
    "More…"
}

/// The warning of the session log for a connection whose guest process
/// the guest proxy could not name, once per session.
pub fn ask_unknown_process(host: &str, port: u16) -> String {
    format!("the guest proxy could not name the process of a connection to {host}:{port}; mode ask denies such connections as the guest system's, without a question. paddock app log NAME lists them with the process unknown")
}

// ---- The notice of a paused session ----------------------------------------

/// The one line of the notice in `paddock approvals`.
pub fn asks_notice_summary(app: &str, open: usize) -> String {
    format!("{app} made many new connections ({open} questions open); asking is paused")
}

/// The text of the notice panel.
pub fn asks_notice_detail(app: &str, name: &str, open: usize) -> String {
    format!(
        "{app} made many new connections: {open} questions are open. Review them in paddock app status {name}. Deny all denies them once; Ask one by one shows them one at a time"
    )
}

/// The two buttons of the notice: deny every open question once, or go on
/// with one question at a time.
pub fn asks_notice_button(deny_all: bool) -> &'static str {
    if deny_all {
        "Deny all"
    } else {
        "Ask one by one"
    }
}

/// The line of the session log when asking pauses.
pub fn asks_paused(app: &str, name: &str, open: usize) -> String {
    format!(
        "{app} made more than {} new connections that wait for an answer ({open} open). Asking is paused for this session: paddock app status {name} lists them",
        super::super::ask_queue::GUARD_MAX
    )
}

/// The line of the session log after the notice is answered.
pub fn asks_resumed(deny_all: bool, open: usize) -> String {
    if deny_all {
        format!("denied the {open} open questions once; asking goes on")
    } else {
        format!("asking goes on, one question at a time ({open} open)")
    }
}

// ---- `paddock app status` --------------------------------------------------

/// The value of the `questions` row.
pub fn status_questions_row(open: usize, paused: bool, system_denied: u64) -> String {
    let mut s = match (open, paused) {
        (0, _) => "none open".to_string(),
        (n, false) => format!("{n} open, one on the screen at a time"),
        (n, true) => format!(
            "{n} open, asking paused: answer the notice, or add rules with paddock app rules"
        ),
    };
    if system_denied > 0 {
        s.push_str(&format!(
            "; {system_denied} connections of the guest system denied without asking"
        ));
    }
    s
}

/// One open question under the row.
pub fn status_question_line(app: &str, host: &str, port: u16, waiting: u64, shown: bool) -> String {
    let mut s = format!("  {app} -> {host}:{port}");
    if shown {
        s.push_str(" (on the screen)");
    }
    if waiting > 0 {
        s.push_str(&format!(", {waiting} connections wait"));
    }
    s
}

/// One host of the guest system that was denied without asking.
pub fn status_system_line(host: &str, port: u16, process: &str, count: u64) -> String {
    format!("  denied {host}:{port} x{count} (guest system, last process {process})")
}

#[cfg(test)]
#[path = "asks_tests.rs"]
mod tests;
