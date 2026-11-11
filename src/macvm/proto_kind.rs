//! The log text of the events of the wire protocol (`proto.rs`).

use super::{AxNode, Event, MenuItem, Request};

impl Event {
    /// A short text for the logs: no token, no clipboard or typed text.
    pub fn kind(&self) -> String {
        match self {
            Event::Hello(h) => format!(
                // not user text
                "hello v{} agent {} macOS {} ({}) {}x{} @{}",
                h.v, h.agent, h.macos, h.build, h.display.w, h.display.h, h.scale
            ),
            Event::Health(h) => format!(
                // not user text
                "health up {}s load {:.2} app {}",
                h.uptime,
                h.load,
                if h.app_running { "running" } else { "none" }
            ),
            Event::Windows(w) => format!(
                // not user text
                "windows {} in order, {} changed, {} removed",
                w.order.len(),
                w.changed.len(),
                w.removed.len()
            ),
            Event::Focus { app, window } => match window {
                Some(w) => format!("focus {app} window {w}"), // not user text
                None => format!("focus {app}"),               // not user text
            },
            Event::Clipboard { count, items, .. } => {
                format!("clipboard change {count} with {} items", items.len()) // not user text
            }
            Event::Menu { app, items, cut } => format!(
                // not user text
                "menu of {app} with {} items{}",
                MenuItem::count(items),
                if *cut { " (cut)" } else { "" }
            ),
            Event::AppExited {
                app,
                code,
                crash_report,
                relaunch,
            } => format!(
                // not user text
                "app_exited {app} code {code}{}{}",
                match crash_report {
                    Some(p) => format!(" crash report {}", p.display()), // not user text
                    None => String::new(),
                },
                if *relaunch { "" } else { " (no relaunch)" }
            ),
            Event::Pong { .. } => "pong".into(),
            Event::AxValue { window, .. } => format!("ax.value of window {window}"), // not user text
            Event::AxTree { window, nodes, cut } => format!(
                // not user text
                "ax.tree of window {window} with {} nodes{}",
                AxNode::count(nodes),
                if *cut { " (cut)" } else { "" }
            ),
            Event::Disk { data_free, warn } => {
                format!(
                    // not user text
                    "disk {data_free} free{}",
                    if *warn { " (warn)" } else { "" }
                )
            }
            Event::DataUnmountable { .. } => "data.unmountable".into(),
            Event::DataProgress { wait_secs, .. } => format!("data.progress wait {wait_secs} s"), // not user text
            Event::Dialog {
                owner, dismissed, ..
            } => format!(
                // not user text
                "dialog of {owner}{}",
                if *dismissed { " (dismissed)" } else { "" }
            ),
            Event::ScreenLock { locked, uptime, .. } => format!(
                // not user text
                "screen.lock {} up {uptime}s",
                if *locked { "locked" } else { "unlocked" }
            ),
            Event::LinkOpen { url } => format!("link.open {}", link_kind(url)), // not user text
            Event::Reply(r) => r.kind(),
        }
    }
}

/// A link for the logs: the scheme and the host and the length, never
/// the path or the query, which carry the state and the code of a login.
pub fn link_kind(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => format!(
            // not user text
            "{}://{} ({} characters)",
            u.scheme(),
            u.host_str().unwrap_or(""),
            url.chars().count()
        ),
        Err(_) => format!(
            // not user text
            "a link that does not parse ({} characters)",
            url.chars().count()
        ),
    }
}

impl Request {
    /// A short text for the logs: no token, no password, no clipboard
    /// text, no environment values.
    pub fn kind(&self) -> String {
        match self {
            Request::ClockSet { .. } => "clock.set".into(),
            Request::Token { v, .. } => format!("token from host v{v}"), // not user text
            Request::WindowMove { id, frame } => format!(
                // not user text
                "window.move {id} to {},{} {}x{}",
                frame.x, frame.y, frame.w, frame.h
            ),
            Request::WindowRaise { id } => format!("window.raise {id}"), // not user text
            Request::WindowMinimize { id } => format!("window.minimize {id}"), // not user text
            Request::WindowClose { id } => format!("window.close {id}"), // not user text
            Request::WindowFocus { id } => format!("window.focus {id}"), // not user text
            Request::AppLaunch { path, args, env } => format!(
                // not user text
                "app.launch {} with {} args and {} variables",
                path.display(),
                args.len(),
                env.len()
            ),
            Request::AppQuit { app, force } => {
                let force = if *force { " (force)" } else { "" };
                format!("app.quit {app}{force}") // not user text
            }
            Request::AppCopy {
                from,
                to,
                version,
                unquarantine,
            } => format!(
                // not user text
                "app.copy {} to {} ({version}){}",
                from.display(),
                to.display(),
                if *unquarantine { " unquarantine" } else { "" }
            ),
            Request::ClipboardSet { items, .. } => format!("clipboard.set {} items", items.len()), // not user text
            Request::ClipboardWatch { on } => {
                format!("clipboard.watch {}", if *on { "on" } else { "off" }) // not user text
            }
            Request::Drop { window, paths, .. } => {
                format!("drop {} paths on window {window}", paths.len()) // not user text
            }
            Request::DisplaySet { w, h, scale } => format!("display.set {w}x{h} @{scale}"), // not user text
            Request::DataPrepare { data_id, .. } => format!("data.prepare {data_id}"), // not user text
            Request::Configure { steps, .. } => format!("configure {} steps", steps.len()), // not user text
            Request::Ping { .. } => "ping".into(),
            Request::AgentUpdate { version, path, .. } => {
                format!("agent.update to {version} from {}", path.display()) // not user text
            }
            Request::AgentRestart => "agent.restart".into(),
            Request::InputSet {
                sources, current, ..
            } => format!("input.set {current} of {} sources", sources.len()), // not user text
            Request::LogFetch => "log.fetch".into(),
            Request::DiskLow => "disk.low".into(),
            Request::Menu { app } => format!("menu of {app}"), // not user text
            Request::MenuPress { path } => format!("menu.press {path:?}"), // not user text
            Request::AxWatch { on } => format!("ax.watch {}", if *on { "on" } else { "off" }), // not user text
            Request::AxTree { window } => format!("ax.tree of window {window}"), // not user text
            Request::AxPress {
                window: w, path, ..
            } => format!("ax.press {path:?} of window {w}"), // not user text
            Request::AxSet { window, path, .. } => format!("ax.set {path:?} of window {window}"), // not user text
            Request::Shutdown { .. } => "shutdown".into(),
            Request::KeychainUnlock { .. } => "keychain.unlock".into(),
            Request::BenchAnimate { window, secs } => {
                format!("bench.animate window {window} for {secs}s") // not user text
            }
            Request::BenchText { window } => format!("bench.text window {window}"), // not user text
            Request::BenchHang => "bench.hang".into(),
            Request::BenchSpawnChild { argv } => {
                format!("bench.spawn_child {} args", argv.len()) // not user text
            }
            Request::BenchPasteboard { action, .. } => format!("bench.pasteboard {action}"), // not user text
            Request::BenchEnv => "bench.env".into(),
            Request::BenchDockMenu => "bench.dock_menu".into(),
            Request::SharesOpen => "shares.open".into(),
            Request::BenchLock => "bench.lock".into(),
            Request::BenchKeychain => "bench.keychain".into(),
            Request::LoginForward { port, .. } => format!("login.forward port {port}"), // not user text
        }
    }
}
