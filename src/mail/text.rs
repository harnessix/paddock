//! How mail looks: the one-line form of `paddock mail watch`, the block
//! form of `paddock-host mail wait`, and the status texts.
//!
//! Mail text is data. Every line names its sender, so a reader never takes
//! an agent's text for a line of paddock, and control characters never
//! reach a terminal.

use super::{Kind, Message, PADDOCK};

/// The longest text of one watch line. `paddock mail show` prints all of it.
pub const LINE_TEXT: usize = 2000;

/// The start reminder for the agent: the session note of each run and the
/// end of the first prompt of a detached run.
pub const REMINDER: &str = "paddock: this run has a mailbox. Whoever started it can send you messages and reads what you send. Read new messages with `paddock-host mail wait --timeout 60` (exit 75: none yet). Answer with `paddock-host mail send TEXT`, and report your state with `paddock-host mail status working`, `blocked: REASON` or `done: SUMMARY`.";

/// The first prompt of a detached run, with the start reminder at its end.
pub fn with_reminder(prompt: &[u8]) -> Vec<u8> {
    let mut out = prompt.to_vec();
    while out.last().is_some_and(u8::is_ascii_whitespace) {
        out.pop();
    }
    out.extend_from_slice(b"\n\n---\n");
    out.extend_from_slice(REMINDER.as_bytes());
    out.push(b'\n');
    out
}

/// A status is `working`, `blocked: REASON` or `done: SUMMARY`.
pub fn check_status(text: &str) -> Result<(), String> {
    let t = text.trim();
    let ok = t == "working"
        || ["blocked:", "done:"]
            .iter()
            .any(|p| t.strip_prefix(p).is_some_and(|r| !r.trim().is_empty()));
    if ok {
        Ok(())
    } else {
        Err("a status is `working`, `blocked: REASON` or `done: SUMMARY`".into())
    }
}

/// Text without control characters or invisible format characters.
pub fn clean(s: &str) -> String {
    crate::broker::clean_text(s)
}

/// The text on one line: line breaks become ` ⏎ `, and long text ends with
/// a pointer to `paddock mail show`.
pub fn one_line(m: &Message, run: &str) -> String {
    let flat = clean(&m.text)
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join(" ⏎ ")
        .replace('\t', " ");
    if flat.chars().count() <= LINE_TEXT {
        return flat;
    }
    let cut: String = flat.chars().take(LINE_TEXT).collect();
    format!(
        "{cut} … (cut; `paddock mail show {} {}` prints all of it)",
        crate::ids::short(run),
        m.seq
    )
}

/// The local time as `HH:MM:SS`.
pub fn clock(secs: u64) -> String {
    // SAFETY: localtime_r writes only into `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return secs.to_string();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Who wrote a message, for a reader on the host.
pub fn sender(m: &Message, run: &str) -> String {
    if m.from == PADDOCK {
        return "from paddock".into();
    }
    match &m.child {
        Some(c) => format!("from sandbox {c} (a child of {run}), agent text"),
        None if m.from == super::HOST => "from the host".into(),
        None => format!("from sandbox {run}, agent text"),
    }
}

/// What kind of message, with its event name and reply.
fn label(m: &Message) -> String {
    let mut s = match (&m.kind, &m.event) {
        (Kind::Event, Some(e)) => format!("event {e}"),
        (k, _) => k.name().to_string(),
    };
    if let Some(r) = m.reply_to {
        s.push_str(&format!(" (reply to #{r})"));
    }
    if m.interrupt {
        s.push_str(" (interrupt)");
    }
    s
}

/// One line of `paddock mail watch`:
/// `[1a2b3c #7 13:05:01] from sandbox ID, agent text, say: TEXT`.
pub fn watch_line(m: &Message, run: &str) -> String {
    let mut line = format!(
        "[{} #{} {}] {}, {}: {}",
        crate::ids::short(run),
        m.seq,
        clock(m.ts),
        sender(m, run),
        label(m),
        one_line(m, run)
    );
    if !m.refs.is_empty() {
        line.push_str(&format!(" [refs: {}]", clean(&m.refs.join(", "))));
    }
    line
}

/// The JSON form of a watch line: the message with its run.
pub fn watch_json(m: &Message, run: &str) -> String {
    let mut v = serde_json::to_value(m).unwrap_or_default();
    v["run"] = run.into();
    v["short"] = crate::ids::short(run).into();
    v.to_string()
}

/// A message for the agent, in `paddock-host mail wait`: a header line and
/// the text as it is.
pub fn agent_block(m: &Message) -> String {
    let who = if m.from == super::HOST {
        "your starter on the host".to_string()
    } else if let Some(p) = m.from.strip_prefix("sandbox:") {
        format!("sandbox {p} that started you")
    } else {
        m.from.clone()
    };
    let mut head = format!("--- mail #{} {} from {who}", m.seq, label(m));
    if m.kind == Kind::Ask {
        head.push_str(&format!(
            ". Answer with: paddock-host mail send --reply-to {} TEXT",
            m.seq
        ));
    }
    if !m.refs.is_empty() {
        head.push_str(&format!(". Files: {}", clean(&m.refs.join(", "))));
    }
    format!("{head}\n{}\n", clean(&m.text).trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_has_one_of_three_forms() {
        assert!(check_status("working").is_ok());
        assert!(check_status("blocked: no network").is_ok());
        assert!(check_status("done: all tests pass").is_ok());
        assert!(check_status("blocked:").is_err());
        assert!(check_status("great").is_err());
    }

    #[test]
    fn a_watch_line_is_one_line_and_names_the_sender() {
        let mut m = Message::new("agent", Kind::Say, "first\nsecond\u{1b}[2J");
        m.seq = 7;
        let l = watch_line(&m, "claude-app-1a2b3c");
        assert!(!l.contains('\n') && !l.contains('\u{1b}'), "{l}");
        assert!(l.starts_with("[1a2b3c #7 "), "{l}");
        assert!(
            l.contains("from sandbox claude-app-1a2b3c, agent text, say: first ⏎ second"),
            "{l}"
        );
        let e = Message::event("ended", "the sandbox ended with exit code 0");
        assert!(watch_line(&e, "x-1").contains("from paddock, event ended: the sandbox"));
    }

    #[test]
    fn long_text_is_cut_with_a_pointer() {
        let mut m = Message::new("agent", Kind::Say, "a".repeat(LINE_TEXT + 10));
        m.seq = 3;
        let l = watch_line(&m, "claude-app-1a2b3c");
        assert!(l.contains("paddock mail show 1a2b3c 3"), "{l}");
    }

    #[test]
    fn an_ask_tells_the_agent_how_to_answer() {
        let mut m = Message::new("host", Kind::Ask, "Which branch?");
        m.seq = 4;
        let b = agent_block(&m);
        assert!(b.contains("paddock-host mail send --reply-to 4"), "{b}");
        assert!(b.ends_with("Which branch?\n"), "{b}");
    }
}
