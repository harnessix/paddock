//! The `session` lines of the launcher event stream from the event log of
//! a profile: each line that changes the state of the session publishes
//! one. Pure: [`change`] folds the kinds of `events.rs` into
//! [`SessionState`], and [`app_of`] reads the profile from the path of
//! the log.
//!
//! The rules:
//! - A state moves forward within a session: starting, running,
//!   stopping, stopped (or error). `start` begins a new session.
//! - A phase line of a later state is not repeated, but a new phase is.
//! - `window opened` is the running state, once.

use super::host::DEFAULT_HOME;
use super::layout::PROFILES_DIR;
use crate::eventstream::schema::{
    Applies, Event, Grant, GrantSource, GrantWhat, Session, SessionState, Stream, StreamState,
};
use std::path::Path;

/// The state and phase that a line of `kind` starts, from `prev`.
pub fn change(
    prev: Option<(SessionState, Option<&'static str>)>,
    kind: &str,
    text: &str,
) -> Option<(SessionState, Option<&'static str>)> {
    use SessionState::*;
    let next = match kind {
        "start" => return Some((Starting, Some("cloning"))),
        "prepare" => (Starting, Some("cloning")),
        "restore" | "running" | "hello" => (Starting, Some("booting")),
        "copy" => (Starting, Some("copying app")),
        "launch" => (Starting, Some("opening")),
        "window" if text == "opened" => (Running, None),
        "quit" => (Stopping, Some("quitting")),
        "save" => (Stopping, Some("saving")),
        "stop" | "end" => (Stopped, None),
        "crashed" => (Error, None),
        _ => return None,
    };
    match prev {
        Some(p) if p == next => None,
        // Never back: a copy while the app runs is no new start.
        Some((s, _)) if next.0 < s => None,
        // After the end only a new start counts.
        Some((Stopped | Error, _)) => None,
        _ => Some(next),
    }
}

/// `(app, home)` of a log at `<macvm>/profiles.noindex/<app>/<home>/logs/events.jsonl`.
pub fn app_of(log: &Path) -> Option<(String, String)> {
    let home_dir = log.parent()?.parent()?;
    let app_dir = home_dir.parent()?;
    if app_dir.parent()?.file_name()? != PROFILES_DIR {
        return None;
    }
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    Some((
        name(app_dir)?,
        name(home_dir).unwrap_or_else(|| DEFAULT_HOME.into()),
    ))
}

/// The event of a change.
pub fn event(app: &str, home: &str, state: SessionState, phase: Option<&str>, text: &str) -> Event {
    Event::Session(Session {
        app: app.to_string(),
        home: home.to_string(),
        state,
        phase: phase.map(str::to_string),
        text: text.to_string(),
    })
}

/// The `grant` line of a firewall rule that `action` (`add`, `remove`)
/// changed for the profile `app`. Rules apply to the next connection.
pub fn rule_grant(
    app: &str,
    r: &super::firewall::Rule,
    action: &str,
    source: GrantSource,
) -> Event {
    let (verdict, port, scope) = (r.verdict.word(), r.port.text(), r.scope.word());
    let detail = format!("{verdict} {}:{port} {scope}", r.host); // not user text
    Event::Grant(Grant {
        app: app.to_string(),
        what: GrantWhat::Rule,
        action: action.to_string(),
        detail,
        source,
        applies: Applies::Live,
        id: Some(r.id.clone()),
    })
}

/// The ordered rule event is unavailable. Request a legacy-readable global
/// refresh without claiming a live grant. Some consumers do not refresh open
/// rule views on this signal. Details are best-effort history, not current state.
pub fn historical_rule_grant(
    _app: &str,
    _rule: &super::firewall::Rule,
    _action: &str,
    _source: GrantSource,
) -> Event {
    Event::Stream(Stream {
        state: StreamState::Lost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::*;

    fn fold(lines: &[(&str, &str)]) -> Vec<(SessionState, Option<&'static str>)> {
        let mut prev = None;
        let mut out = Vec::new();
        for (k, t) in lines {
            if let Some(c) = change(prev, k, t) {
                out.push(c);
                prev = Some(c);
            }
        }
        out
    }

    #[test]
    fn a_session_from_start_to_stop() {
        let got = fold(&[
            ("start", "cold: x"),
            ("prepare", "cloned"),
            ("running", "booting"),
            ("hello", "agent 0.9"),
            ("copy", "copy Sample Mail"),
            ("launch", "sent"),
            ("window", "opened"),
            ("window", "opened"),
            ("copy", "copy while running"),
            ("menu", "x"),
            ("quit", "asked"),
            ("save", "saved"),
            ("stop", "quit with save"),
            ("end", "the session ended"),
        ]);
        assert_eq!(
            got,
            [
                (Starting, Some("cloning")),
                (Starting, Some("booting")),
                (Starting, Some("copying app")),
                (Starting, Some("opening")),
                (Running, None),
                (Stopping, Some("quitting")),
                (Stopping, Some("saving")),
                (Stopped, None),
            ]
        );
    }

    #[test]
    fn a_crash_then_a_new_start() {
        let got = fold(&[
            ("start", "warm"),
            ("crashed", "session ended without a save"),
            ("end", "x"),
            ("start", "cold"),
        ]);
        assert_eq!(
            got,
            [
                (Starting, Some("cloning")),
                (Error, None),
                (Starting, Some("cloning"))
            ]
        );
    }

    #[test]
    fn a_log_without_a_start_still_says_stopped() {
        // Another paddock saw the dead pid of a clean session.
        assert_eq!(change(None, "stop", "clean"), Some((Stopped, None)));
        assert_eq!(change(None, "network", "x"), None);
    }

    /// The event log emits session lines to an injected in-memory sink.
    #[test]
    fn the_event_log_publishes_session_lines() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let t = tempfile::tempdir().unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        super::super::events::SESSION_EVENT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |event| sink.borrow_mut().push(event)));
        });
        let profile = super::super::layout::Layout::at(t.path().join("macvm"))
            .profile("stream-test", "default");
        let ev = super::super::events::Events::open(&profile);
        ev.record("start", "cold: first", &[]);
        ev.record("menu", "x", &[]);
        ev.record("window", "opened", &[]);
        let mut got = Vec::new();
        for event in events.borrow().iter() {
            let line = crate::eventstream::Line::at(1, event.clone()).to_text();
            let l: crate::eventstream::Line = serde_json::from_str(&line).unwrap();
            if let Event::Session(s) = l.event {
                if s.app == "stream-test" {
                    got.push((s.state, s.phase, s.text));
                }
            }
        }
        assert_eq!(
            got,
            [
                (
                    Starting,
                    Some("cloning".to_string()),
                    "cold: first".to_string()
                ),
                (Running, None, "opened".to_string())
            ]
        );
        super::super::events::SESSION_EVENT_HOOK.with(|hook| *hook.borrow_mut() = None);
    }

    #[test]
    fn a_rule_grant_names_the_rule() {
        let r = crate::macvm::firewall::new_rule(
            "*",
            None,
            "api.example.com",
            crate::macvm::firewall::Port::parse("443").unwrap(),
            crate::macvm::firewall::Verdict::Allow,
            crate::macvm::firewall::Scope::Always,
            "user",
            0,
        );
        let Event::Grant(g) = rule_grant("sample-notes", &r, "add", GrantSource::QuestionPanel)
        else {
            panic!()
        };
        assert_eq!(g.detail, "allow api.example.com:443 always");
        assert_eq!(g.id.as_deref(), Some(r.id.as_str()));
        assert_eq!((g.app.as_str(), g.applies), ("sample-notes", Applies::Live));
    }

    #[test]
    fn rule_grants_match_the_launcher_fixture() {
        use crate::macvm::firewall::{new_rule, Port, Scope, Verdict};
        let mut r = new_rule(
            "*",
            None,
            "*.tracker.example.net",
            Port::parse("*").unwrap(),
            Verdict::Deny,
            Scope::Always,
            "user",
            0,
        );
        r.id = "r8".into();
        let fixture = include_str!("../../tests/fixtures/launcher/grant.jsonl");
        for (line, action) in fixture.lines().skip(1).take(2).zip(["add", "remove"]) {
            let expected: crate::eventstream::Line = serde_json::from_str(line).unwrap();
            assert_eq!(
                expected.event,
                rule_grant("sample-notes", &r, action, GrantSource::You)
            );
        }
    }

    #[test]
    fn app_and_home_from_the_log_path() {
        let p = Path::new("/m/profiles.noindex/sample-mail/default/logs/events.jsonl");
        assert_eq!(app_of(p), Some(("sample-mail".into(), "default".into())));
        assert_eq!(
            app_of(Path::new("/m/other/sample-mail/default/logs/events.jsonl")),
            None
        );
        assert_eq!(app_of(Path::new("events.jsonl")), None);
    }
}
