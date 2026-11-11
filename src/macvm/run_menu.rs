//! The menu bar replica in the shim `paddock-vm app`, macOS only: the
//! `menu` event of the guest goes through the fallback state of `menu.rs`,
//! whose changes `dock.rs` runs on the `NSMenu`s; a click on an item
//! becomes `menu.press`; the `menu` request goes out after a `hello` with
//! the app started and after a press, and a request without an answer keeps
//! the last tree. The `window menu` lines of the control socket and the
//! `menu` field of the session record serve the Mac checks and `paddock app
//! status`. `run.rs` holds the session.

use super::menu::{Change, Fallback, MenuItem, CAP};
use super::proto::Request;
use super::run::{Session, Why};
use super::text;
use std::time::Instant;

/// The menu side of the shell.
#[derive(Default)]
pub(super) struct MenuShell {
    pub(super) fallback: Fallback,
    /// When the pending `menu` request went out, for the ms of the event.
    requested: Option<Instant>,
    /// The cut line went into the event log once.
    cut_noted: bool,
}

impl Session<'_> {
    /// A `menu` event: the tree replaces the last one, the host menus
    /// change where the diff says, the record learns the state.
    pub(super) fn on_menu(&mut self, items: Vec<MenuItem>, cut: bool) {
        let count = MenuItem::count(&items);
        let ms = self
            .shell
            .menu
            .requested
            .take()
            .map(|t| t.elapsed().as_secs_f64() * 1000.0);
        let changes = self.shell.menu.fallback.tree(items);
        let mut numbers = vec![("items", count as f64), ("changes", changes.len() as f64)];
        if let Some(ms) = ms {
            numbers.push(("ms", ms));
        }
        self.events
            .record("menu", &text::menu_received(count), &numbers);
        if cut && !self.shell.menu.cut_noted {
            self.shell.menu.cut_noted = true;
            let line = text::menu_cut(CAP);
            log::warn!("macvm: {line}");
            self.events.record("menu", &line, &[]);
        }
        self.apply_menu(&changes);
    }

    /// The changes on the host menus, and the record.
    fn apply_menu(&mut self, changes: &[Change]) {
        for c in changes {
            log::debug!("macvm: menu change {}", c.kind());
        }
        if !changes.is_empty() {
            self.shell
                .dock
                .apply_menu(self.shell.menu.fallback.items(), changes);
        }
        let menu = self.shell.menu.fallback.record();
        let value = serde_json::to_value(&menu).unwrap_or_default();
        self.update_record(|v| v["menu"] = value);
    }

    /// Ask the guest for the tree, one request at a time.
    pub(super) fn menu_request(&mut self) {
        if self.shell.menu.requested.is_some() {
            return;
        }
        let app = self.plan.app.clone();
        if self.send(Request::Menu { app }, Why::Menu) {
            self.shell.menu.requested = Some(Instant::now());
        }
    }

    /// The reply of a `menu` request: the event came before it.
    pub(super) fn menu_answered(&mut self) {
        self.shell.menu.requested = None;
    }

    /// A new `hello`: presses go again, the last tree stays until the
    /// next event, and the host asks for one when the app runs.
    pub(super) fn menu_hello(&mut self) {
        self.shell.menu.requested = None;
        let changes = self.shell.menu.fallback.agent_back();
        self.apply_menu(&changes);
        if self.app_started {
            self.menu_request();
        }
    }

    /// The control connection closed: the last tree stays, presses are
    /// dropped until the next `hello`.
    pub(super) fn menu_agent_gone(&mut self) {
        self.shell.menu.requested = None;
        let changes = self.shell.menu.fallback.agent_gone();
        self.apply_menu(&changes);
    }

    /// The app exited: the paddock menu again.
    pub(super) fn menu_app_exited(&mut self) {
        let changes = self.shell.menu.fallback.app_exited();
        if !changes.is_empty() {
            self.events.record("menu", &text::menu_first_pass(), &[]);
        }
        self.apply_menu(&changes);
    }

    /// The `menu` request got no reply in 2 s, or an error: the last
    /// tree stays, one line says so.
    pub(super) fn menu_timeout(&mut self) {
        self.shell.menu.requested = None;
        let line = text::menu_timeout_kept();
        log::warn!("macvm: {line}");
        self.events.record("menu", &line, &[]);
        let changes = self.shell.menu.fallback.timeout();
        self.apply_menu(&changes);
    }

    /// A click on an item of the replica, by its Accessibility path: the
    /// press goes to the guest when the last tree has the item, enabled,
    /// and the agent is there; else one log line and nothing is sent.
    pub(super) fn menu_press(&mut self, path: Vec<usize>) {
        let pressed = self
            .shell
            .menu
            .fallback
            .press(&path)
            .map(|_| MenuItem::titles(self.shell.menu.fallback.items(), &path));
        match pressed {
            Ok(titles) => {
                self.events
                    .record("menu", &text::menu_pressed(&titles), &[]);
                self.send(Request::MenuPress { path }, Why::MenuPress);
            }
            Err(why) => {
                let line = text::menu_press_dropped(text::menu_drop_reason(why), &path);
                log::warn!("macvm: {line}");
                self.events.record("menu", &line, &[]);
            }
        }
    }

    /// The `window menu` lines of the control socket: `menu` gives the
    /// state and the tree as JSON, `menu-titles` the titles of the host
    /// main menu after the app menu, `menu-press 2.0` presses the item
    /// with that Accessibility path as a click would. None for another
    /// line.
    pub(super) fn menu_control(&mut self, words: &[&str]) -> Option<Result<String, String>> {
        match words {
            ["menu"] => {
                let record = self.shell.menu.fallback.record();
                let v = serde_json::json!({
                    "shown": record.shown,
                    "items": record.items,
                    "away": record.away,
                    "tree": self.shell.menu.fallback.items(),
                });
                Some(Ok(v.to_string()))
            }
            ["menu-titles"] => {
                let titles = self.shell.dock.menu_titles();
                Some(Ok(serde_json::Value::from(titles).to_string()))
            }
            ["menu-press", path] => {
                let parsed: Result<Vec<usize>, _> = path.split('.').map(str::parse).collect();
                let Ok(path) = parsed else {
                    return Some(Err(text::control_request_unknown(&words.join(" "))));
                };
                self.menu_press(path);
                Some(Ok("pressed".into()))
            }
            _ => None,
        }
    }
}
