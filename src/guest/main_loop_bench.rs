//! The bench ops of the agent's main loop, which `bench::op_gate` in
//! `main_loop.rs` lets through only in a session that started with `--bench`.

use super::Session;
use crate::guest::platform::{Platform, WindowOp};
use crate::macvm::axmirror::Node;
use crate::macvm::clipboard::{self, Item};
use crate::macvm::proto::{ErrorCode, Reply, ReplyData};
use crate::macvm::text;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One frame of the counter: 60 Hz.
const FRAME: Duration = Duration::from_micros(16_667);
/// The longest `bench.animate`.
const ANIMATE_MAX: u32 = 30;

/// The path and the role of the first text area or text field of a
/// tree, depth first.
pub(super) fn text_node(nodes: &[Node]) -> Option<(Vec<usize>, String)> {
    for n in nodes {
        if n.role == "AXTextArea" || n.role == "AXTextField" {
            return Some((n.path.clone(), n.role.clone()));
        }
        if let Some(found) = text_node(&n.children) {
            return Some(found);
        }
    }
    None
}

impl<P: Platform> Session<'_, P> {
    /// The window and its text node, or the reply that says why not.
    fn bench_text_node(
        &mut self,
        req: u64,
        window: u32,
    ) -> Result<(crate::macvm::proto::Window, Vec<usize>, String), Reply> {
        let Some(w) = self.st.snapshot.get(window).cloned() else {
            return Err(Reply::error(
                req,
                ErrorCode::NoSuchWindow,
                text::no_window(window),
            ));
        };
        match self.p.ax_tree(&w) {
            Ok((nodes, _)) => match text_node(&nodes) {
                Some((path, role)) => Ok((w, path, role)),
                None => Err(Reply::error(
                    req,
                    ErrorCode::Unsupported,
                    text::bench_no_text_area(window),
                )),
            },
            Err((code, detail)) => Err(Reply::error(req, code, detail)),
        }
    }

    /// `bench.animate`: the counter in the text area of the window, one
    /// value per frame for `secs` seconds. The loop blocks that long, so
    /// the health events pause; the host's timeout of the request allows
    /// it. The reply carries the number of frames.
    pub(super) fn bench_animate(&mut self, req: u64, window: u32, secs: u32) -> Reply {
        let (w, path, role) = match self.bench_text_node(req, window) {
            Ok(x) => x,
            Err(r) => return r,
        };
        let end = Instant::now() + Duration::from_secs(u64::from(secs.min(ANIMATE_MAX)));
        let mut frame: u64 = 0;
        let mut next = Instant::now();
        while Instant::now() < end {
            frame += 1;
            if let Err((code, detail)) = self.p.ax_tree_set(&w, &path, &role, &frame.to_string()) {
                return Reply::error(req, code, detail);
            }
            next += FRAME;
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        Reply::data(
            req,
            ReplyData::Text {
                text: frame.to_string(),
            },
        )
    }

    /// `bench.text`: focus the window and make sure that it has a text
    /// area. The reply names the role and the path of the text node.
    pub(super) fn bench_text(&mut self, req: u64, window: u32) -> Reply {
        let (w, path, role) = match self.bench_text_node(req, window) {
            Ok(x) => x,
            Err(r) => return r,
        };
        if let Err((code, detail)) = self.p.window_op(&w, WindowOp::Focus) {
            return Reply::error(req, code, detail);
        }
        Reply::data(
            req,
            ReplyData::Text {
                text: format!("{role} at {path:?}"), // not user text
            },
        )
    }

    /// `bench.pasteboard get`: the plain text of the guest pasteboard;
    /// `set`: the pasteboard is this text.
    pub(super) fn bench_pasteboard(&mut self, req: u64, action: &str, text: &str) -> Reply {
        match action {
            "get" => {
                let found = self
                    .p
                    .clipboard_read()
                    .iter()
                    .flat_map(|i| i.types.iter())
                    .find(|(uti, _)| uti == clipboard::TEXT)
                    .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                Reply::data(req, ReplyData::Text { text: found })
            }
            "set" => match self.p.clipboard_write(&[Item::text(text)]) {
                Ok(count) => {
                    self.st.clip_seen = Some(count);
                    Reply::ok(req)
                }
                Err((code, detail)) => Reply::error(req, code, detail),
            },
            other => Reply::error(
                req,
                ErrorCode::BadRequest,
                text::bench_pasteboard_action(other),
            ),
        }
    }
}

/// `bench.hang`: the agent stops answering for good, for the hang row of the
/// failure matrix. The host's `agent.restart` through launchd ends it.
pub(super) fn hang() -> Reply {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// `bench.spawn_child`: a child of the agent, detached from its streams,
/// for the checklist. The reply carries the pid.
pub(super) fn spawn_child(req: u64, argv: &[String]) -> Reply {
    let Some((cmd, args)) = argv.split_first() else {
        return Reply::error(req, ErrorCode::BadRequest, text::bench_no_argv());
    };
    match Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Reply::data(req, ReplyData::Launched { pid: child.id() }),
        Err(e) => Reply::error(req, ErrorCode::Io, e.to_string()),
    }
}

/// `bench.env`: the environment of the agent, `NAME=VALUE` per line,
/// sorted, for the checklist that proves no host secret reaches it.
pub(super) fn env(req: u64) -> Reply {
    let mut lines: Vec<String> = std::env::vars()
        .map(|(k, v)| format!("{k}={v}")) // not user text
        .collect();
    lines.sort();
    Reply::data(
        req,
        ReplyData::Text {
            text: lines.join("\n"),
        },
    )
}

/// The AppleScript that opens the Dock menu of the app with this bundle
/// id, reads the names of its items, closes it, and prints one per line.
pub(super) fn dock_menu_script(bundle_id: &str) -> String {
    format!(
        // not user text
        r#"tell application "System Events"
    set appName to name of first application process whose bundle identifier is "{bundle_id}"
    tell process "Dock"
        set theItem to UI element appName of list 1
        perform action "AXShowMenu" of theItem
        delay 0.3
        set names to name of every menu item of menu 1 of theItem
        key code 53
    end tell
end tell
set AppleScript's text item delimiters to linefeed
return names as text
"#
    )
}

/// `bench.dock_menu`: the items of the Dock menu of the app, one per
/// line, read through System Events in one `osascript` process.
pub(super) fn dock_menu(req: u64, app: Option<&str>) -> Reply {
    let Some(app) = app else {
        return Reply::error(req, ErrorCode::NoSuchApp, text::bench_no_app());
    };
    if app.contains(['"', '\\']) {
        return Reply::error(req, ErrorCode::BadRequest, text::bench_no_app());
    }
    let script = dock_menu_script(app);
    let out = Command::new("/usr/bin/osascript")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(script.as_bytes())?;
            }
            child.wait_with_output()
        });
    match out {
        Ok(o) if o.status.success() => Reply::data(
            req,
            ReplyData::Text {
                text: String::from_utf8_lossy(&o.stdout).trim().to_string(),
            },
        ),
        Ok(o) => Reply::error(
            req,
            ErrorCode::Io,
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        ),
        Err(e) => Reply::error(req, ErrorCode::Io, e.to_string()),
    }
}
