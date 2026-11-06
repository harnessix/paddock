//! The Accessibility mirror in the shim `paddock-vm app`, macOS only: the
//! `ax.tree` event of the guest goes through the mirror state of
//! `axmirror.rs`, whose changes `ax_host.rs` runs on the elements of the
//! host window; a press or a value of an Accessibility client becomes
//! `ax.press` or `ax.set` when the id is in the last tree; the `ax.tree`
//! request goes out when an Accessibility client first asks a session
//! window for its children, after a `hello` and after a press while the
//! mirror is watched, and when another window becomes key. A request
//! without an answer keeps the last tree. The `window ax` line of the
//! control socket and the `ax` field of the session record serve the Mac
//! check and `paddock app status`. `run.rs` holds the session.

use super::axmirror::{well_formed, Change, Mirror, Node, CAP};
use super::proto::Request;
use super::run::{Session, Why};
use super::text;
use std::time::{Duration, Instant};

/// After an `ax.tree` request failed or timed out, the next one waits
/// this long, so a guest without Accessibility costs one line per 10 s
/// while a client keeps asking, not one per second.
pub const RETRY: Duration = Duration::from_secs(10);

/// The mirror side of the shell.
#[derive(Default)]
pub(super) struct AxShell {
    pub(super) mirror: Mirror,
    /// The pending `ax.tree` request: its window and when it went out.
    requested: Option<(u32, Instant)>,
    /// When the last request failed, for [`RETRY`].
    failed_at: Option<Instant>,
    /// The cut line went into the event log once.
    cut_noted: bool,
}

impl Session<'_> {
    /// An `ax.tree` event: the tree replaces the last one, the elements
    /// of the host window change where the diff says, the record learns
    /// the state. A tree whose paths are not its positions is dropped,
    /// because the guest presses by path.
    pub(super) fn on_ax_tree(&mut self, window: u32, nodes: Vec<Node>, cut: bool) {
        let ms = self
            .shell
            .ax
            .requested
            .take()
            .map(|(_, t)| t.elapsed().as_secs_f64() * 1000.0);
        if !well_formed(&nodes) {
            let line = text::ax_tree_malformed(window);
            log::warn!("macvm: {line}");
            self.events
                .record("ax", &line, &[("window", window as f64)]);
            return;
        }
        self.shell.ax.failed_at = None;
        let count = Node::count(&nodes);
        if let Some(old) = self.shell.ax.mirror.window() {
            if old != window {
                self.shell.windows.ax_clear(old);
            }
        }
        let changes = self.shell.ax.mirror.tree(window, nodes);
        let mut numbers = vec![
            ("window", window as f64),
            ("nodes", count as f64),
            ("changes", changes.len() as f64),
        ];
        if let Some(ms) = ms {
            numbers.push(("ms", ms));
        }
        self.events
            .record("ax", &text::ax_tree_received(window, count), &numbers);
        if cut && !self.shell.ax.cut_noted {
            self.shell.ax.cut_noted = true;
            let line = text::ax_tree_cut(CAP);
            log::warn!("macvm: {line}");
            self.events.record("ax", &line, &[]);
        }
        self.apply_ax(window, &changes);
    }

    /// The changes on the elements of the window, and the record.
    fn apply_ax(&mut self, window: u32, changes: &[Change]) {
        for c in changes {
            log::debug!("macvm: ax change {}", c.kind());
        }
        if !changes.is_empty() {
            self.shell
                .windows
                .ax_apply(window, &self.shell.ax.mirror, changes);
        }
        self.write_ax_record();
    }

    fn write_ax_record(&mut self) {
        let record = self.shell.ax.mirror.record();
        let value = serde_json::to_value(&record).unwrap_or_default();
        self.update_record(|v| v["ax"] = value);
    }

    /// Ask the guest for the tree of a window, one request at a time.
    pub(super) fn ax_request(&mut self, window: u32) {
        if self.shell.ax.requested.is_some() {
            return;
        }
        if self.shell.ax.failed_at.is_some_and(|t| t.elapsed() < RETRY) {
            return;
        }
        if self.send(Request::AxTree { window }, Why::AxTree) {
            self.shell.ax.requested = Some((window, Instant::now()));
        }
    }

    /// An Accessibility client asked a session window for its children:
    /// the mirror is watched from now on, and the tree is requested.
    pub(super) fn ax_wanted(&mut self, window: u32) {
        if !self.shell.ax.mirror.watched() {
            self.shell.ax.mirror.wanted();
            let line = text::ax_watch_started(window);
            log::info!("macvm: {line}");
            self.events
                .record("ax", &line, &[("window", window as f64)]);
            self.write_ax_record();
        }
        self.ax_request(window);
    }

    /// The reply of an `ax.tree` request: the event came before it.
    pub(super) fn ax_answered(&mut self) {
        self.shell.ax.requested = None;
    }

    /// The `ax.tree` request got no reply in 2 s, or an error: the last
    /// tree stays, one line says so.
    pub(super) fn ax_timeout(&mut self) {
        let Some((window, _)) = self.shell.ax.requested.take() else {
            return;
        };
        self.shell.ax.failed_at = Some(Instant::now());
        let line = text::ax_tree_timeout_kept();
        log::warn!("macvm: {line}");
        self.events.record("ax", &line, &[]);
        let changes = self.shell.ax.mirror.timeout();
        self.apply_ax(window, &changes);
    }

    /// A new `hello`: presses go again, the last tree stays until the
    /// next event, and a watched mirror asks for the key window's tree.
    pub(super) fn ax_hello(&mut self) {
        self.shell.ax.requested = None;
        self.shell.ax.failed_at = None;
        self.shell.ax.mirror.agent_back();
        self.write_ax_record();
        if self.shell.ax.mirror.watched() {
            if let Some(key) = self.shell.windows.key_id() {
                self.ax_request(key);
            }
        }
    }

    /// The control connection closed: the last tree stays, presses are
    /// dropped until the next `hello`.
    pub(super) fn ax_agent_gone(&mut self) {
        self.shell.ax.requested = None;
        self.shell.ax.mirror.agent_gone();
        self.write_ax_record();
    }

    /// The app exited, or every window went: no tree.
    pub(super) fn ax_app_exited(&mut self) {
        self.shell.ax.requested = None;
        if let Some(window) = self.shell.ax.mirror.window() {
            self.shell.ax.mirror.clear();
            self.shell.windows.ax_clear(window);
            self.events.record("ax", &text::ax_tree_gone(), &[]);
        }
        self.write_ax_record();
    }

    /// A host window closed: its tree goes with it.
    pub(super) fn ax_window_closed(&mut self, id: u32) {
        if self.shell.ax.mirror.window_closed(id) {
            self.shell.windows.ax_clear(id);
            self.events
                .record("ax", &text::ax_tree_gone(), &[("window", id as f64)]);
            self.write_ax_record();
        }
    }

    /// Another session window became key: a watched mirror follows it.
    pub(super) fn ax_became_key(&mut self, id: u32) {
        if self.shell.ax.mirror.watched() && self.shell.ax.mirror.window() != Some(id) {
            self.ax_request(id);
        }
    }

    /// A press of an Accessibility client on the element with this id:
    /// `ax.press` when the last tree has the element, enabled, and the
    /// agent is there; else one log line and nothing is sent.
    pub(super) fn ax_press(&mut self, id: u64) {
        match self.shell.ax.mirror.press(id) {
            Ok((window, node)) => {
                let (path, role, name) = (node.path.clone(), node.role.clone(), node.name());
                self.events
                    .record("ax", &text::ax_pressed(&name), &[("window", window as f64)]);
                self.send(Request::AxPress { window, path, role }, Why::AxAction);
            }
            Err(why) => self.ax_dropped("press", why),
        }
    }

    /// A value of an Accessibility client for the element with this id:
    /// `ax.set` with the same checks. The value is user content and
    /// never goes to a log.
    pub(super) fn ax_set(&mut self, id: u64, value: String) {
        match self.shell.ax.mirror.set(id) {
            Ok((window, node)) => {
                let (path, role, name) = (node.path.clone(), node.role.clone(), node.name());
                self.events.record(
                    "ax",
                    &text::ax_set_sent(&name),
                    &[("window", window as f64)],
                );
                self.send(
                    Request::AxSet {
                        window,
                        path,
                        role,
                        value,
                    },
                    Why::AxAction,
                );
            }
            Err(why) => self.ax_dropped("set", why),
        }
    }

    fn ax_dropped(&mut self, what: &str, why: super::axmirror::Dropped) {
        let line = text::ax_action_dropped(what, text::ax_drop_reason(why));
        log::warn!("macvm: {line}");
        self.events.record("ax", &line, &[]);
    }

    /// The reply of an `ax.press` or `ax.set`, good or bad: the tree
    /// again, because the action changed a state or the tree moved on.
    pub(super) fn ax_action_done(&mut self) {
        if let Some(window) = self.shell.ax.mirror.window() {
            self.ax_request(window);
        }
    }

    /// The `window ax` lines of the control socket: `ax` gives the state
    /// and the tree as JSON, `ax-watch ID` starts the watch of a window
    /// as an Accessibility client's first query would. None for another
    /// line.
    pub(super) fn ax_control(&mut self, words: &[&str]) -> Option<Result<String, String>> {
        match words {
            ["ax"] => {
                let record = self.shell.ax.mirror.record();
                let v = serde_json::json!({
                    "window": record.window,
                    "elements": record.elements,
                    "host_elements": self.shell.windows.ax_len(),
                    "watched": record.watched,
                    "away": record.away,
                    "tree": self.shell.ax.mirror.nodes(),
                });
                Some(Ok(v.to_string()))
            }
            ["ax-watch", id] => {
                let Ok(id) = id.parse::<u32>() else {
                    return Some(Err(text::control_request_unknown(&words.join(" "))));
                };
                self.ax_wanted(id);
                Some(Ok("watching".into()))
            }
            _ => None,
        }
    }
}
