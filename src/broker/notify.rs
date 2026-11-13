//! Desktop notifications through the org.freedesktop.Notifications D-Bus API.
//!
//! Paddock does not run `notify-send`: the arguments of a host process are
//! visible to every local user in /proc, and a notification can hold request
//! text such as clipboard content. The session bus belongs to the user, so
//! the text stays with the user.
//!
//! Every notice also goes into the launcher event stream as a `notice`
//! line ([`announce`]), on every platform and also when no launcher runs.
//! On macOS, when paddock for Mac follows the stream with `--notify` and
//! took the line, paddock does not post its own notification: the
//! launcher shows it with paddock's name and icon.

use crate::env::HostEnv;
#[cfg(target_os = "linux")]
use anyhow::Context as _;
use anyhow::Result;
#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use zbus::blocking::{connection::Builder, Connection, Proxy};
#[cfg(target_os = "linux")]
use zbus::zvariant::Value;

#[cfg(target_os = "linux")]
const DEST: &str = "org.freedesktop.Notifications";
#[cfg(target_os = "linux")]
const PATH: &str = "/org/freedesktop/Notifications";

/// One notification. `actions` are (key, label) pairs.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Note {
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub critical: bool,
    /// What the notice is about, the `what` of its `notice` line:
    /// `approval`, `frozen`, `storage`. Empty means `info`.
    #[serde(default)]
    pub what: String,
    /// What answers it, for example the id of a request.
    #[serde(default)]
    pub action: Option<String>,
}

/// paddock for Mac shows the notices that it takes. Only macOS has it.
const LAUNCHER_SHOWS: bool = cfg!(target_os = "macos");

/// The `notice` line of `n`.
pub fn notice(n: &Note) -> crate::eventstream::schema::Notice {
    crate::eventstream::schema::Notice::new(
        &n.what,
        &n.summary,
        &n.body,
        n.critical,
        n.action.as_deref(),
    )
}

/// Publish `n` on the event stream of `env`. True when a launcher that
/// shows notices took it on macOS: then paddock does not post it too.
pub fn announce(env: &HostEnv, n: &Note) -> bool {
    use crate::eventstream::{schema::Event, Bus, Line};
    let line = Line::now(Event::Notice(notice(n)));
    Bus::of(env).send_notice(&line.to_text()) && LAUNCHER_SHOWS
}

/// Show a notification and return at once: publish it, and post it
/// unless paddock for Mac shows it.
pub fn show(env: &HostEnv, n: &Note) -> Result<()> {
    if announce(env, n) {
        return Ok(());
    }
    show_here(env, n)
}

/// The session bus of the host user. The address comes from the host
/// environment that paddock read at start, not from the current process, so
/// tests with a fake environment never reach the real desktop.
#[cfg(target_os = "linux")]
pub fn session(env: &HostEnv) -> Result<Connection> {
    let addr = match env.var("DBUS_SESSION_BUS_ADDRESS") {
        Some(a) if !a.is_empty() => a.to_string(),
        _ => {
            let bus = env
                .runtime_dir
                .as_ref()
                .map(|d| d.join("bus"))
                .filter(|b| b.exists())
                .context("no session bus for notifications")?;
            format!("unix:path={}", bus.display())
        }
    };
    Ok(Builder::address(addr.as_str())?.build()?)
}

#[cfg(target_os = "linux")]
fn proxy(conn: &Connection) -> Result<Proxy<'static>> {
    Ok(Proxy::new(conn, DEST, PATH, DEST)?)
}

/// The body as plain text for a server that reads markup in bodies (the
/// `body-markup` capability, on by default in mako and dunst): `&`, `<` and
/// `>` are escaped, so text from a sandbox cannot add links, images or
/// formatting.
pub fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(target_os = "linux")]
fn reads_markup(p: &Proxy<'_>) -> bool {
    p.call::<_, _, Vec<String>>("GetCapabilities", &())
        .map(|caps| caps.iter().any(|c| c == "body-markup"))
        // When the server does not say, escaping is the safe choice.
        .unwrap_or(true)
}

#[cfg(target_os = "linux")]
fn send(p: &Proxy<'_>, n: &Note) -> Result<u32> {
    let body = if reads_markup(p) {
        escape_markup(&n.body)
    } else {
        n.body.clone()
    };
    let actions: Vec<&str> = n
        .actions
        .iter()
        .flat_map(|(k, l)| [k.as_str(), l.as_str()])
        .collect();
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("urgency", Value::U8(if n.critical { 2 } else { 1 }));
    let id: u32 = p.call(
        "Notify",
        &(
            "paddock",
            0u32,
            "",
            n.summary.as_str(),
            body.as_str(),
            actions,
            hints,
            -1i32,
        ),
    )?;
    Ok(id)
}

/// Post a notification on this desktop, without the event stream.
#[cfg(target_os = "linux")]
fn show_here(env: &HostEnv, n: &Note) -> Result<()> {
    let conn = session(env)?;
    send(&proxy(&conn)?, n)?;
    Ok(())
}

/// Show a notification with actions. `on_action` gets the key of the action
/// that the user picks. The call returns when the notification is shown; a
/// thread waits for the answer until the notification closes. The returned
/// handle closes the notification early.
#[cfg(target_os = "linux")]
pub fn ask(
    env: &HostEnv,
    n: &Note,
    on_action: impl FnOnce(&str) + Send + 'static,
) -> Result<Handle> {
    // No launcher shows notices on Linux: the line is for the stream only.
    let _ = announce(env, n);
    let conn = session(env)?;
    let p = proxy(&conn)?;
    // Subscribe before the notification exists, so no answer is lost.
    let signals = p.receive_all_signals()?;
    let id = send(&p, n)?;
    let conn2 = conn.clone();
    std::thread::spawn(move || {
        let _keep = conn2;
        for msg in signals {
            let h = msg.header();
            let member = h.member().map(|m| m.as_str().to_string());
            match member.as_deref() {
                Some("ActionInvoked") => {
                    if let Ok((nid, key)) = msg.body().deserialize::<(u32, String)>() {
                        if nid == id {
                            on_action(&key);
                            return;
                        }
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((nid, _)) = msg.body().deserialize::<(u32, u32)>() {
                        if nid == id {
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
    });
    Ok(Handle { conn, id })
}

/// `ask`, and where a notification cannot have buttons (macOS), a plain
/// one: its text must name the command that answers.
pub fn ask_or_show(
    env: &HostEnv,
    n: &Note,
    on_action: impl FnOnce(&str) + Send + 'static,
) -> Option<Handle> {
    match ask(env, n, on_action) {
        Ok(h) => Some(h),
        Err(e) => {
            log::debug!("cannot show the question with buttons: {e:#}");
            // `ask` published it already.
            if let Err(e) = show_here(env, n) {
                log::debug!("cannot show the question as a notification: {e:#}");
            }
            None
        }
    }
}

/// A shown notification.
#[cfg(target_os = "linux")]
pub struct Handle {
    conn: Connection,
    id: u32,
}

#[cfg(target_os = "linux")]
impl Handle {
    pub fn close(&self) {
        if let Ok(p) = proxy(&self.conn) {
            let _: Result<(), _> = p.call("CloseNotification", &(self.id,));
        }
    }
}

/// macOS without paddock for Mac: `terminal-notifier` or, as the last
/// way, `osascript` (`onboard::mac_notifier`), with the text as plain
/// arguments, so no shell and no AppleScript parses it. The text is in
/// the arguments of that program while it runs. As on Linux, a run with a
/// home that is not the user's own (a test) never reaches the real
/// desktop.
#[cfg(not(target_os = "linux"))]
fn show_here(env: &HostEnv, n: &Note) -> Result<()> {
    if !crate::launchers::home_is_real(env) {
        anyhow::bail!("no desktop notification: HOME is not the home of this user");
    }
    crate::onboard::mac_notify(env, &n.summary, &n.body)
}

/// macOS: paddock's own notifications have no buttons. When paddock for
/// Mac took the notice, it shows it and the answer goes through `paddock
/// approvals answer`: the returned handle never calls `on_action`. Else
/// [`ask`] refuses and [`ask_or_show`] posts the plain text.
#[cfg(not(target_os = "linux"))]
pub fn ask(
    env: &HostEnv,
    n: &Note,
    on_action: impl FnOnce(&str) + Send + 'static,
) -> Result<Handle> {
    let _ = on_action;
    if announce(env, n) {
        return Ok(Handle);
    }
    Err(crate::platform::unavailable("a desktop notification"))
}

#[cfg(not(target_os = "linux"))]
pub struct Handle;

#[cfg(not(target_os = "linux"))]
impl Handle {
    pub fn close(&self) {}
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;

#[cfg(test)]
mod markup_tests {
    #[test]
    fn markup_from_a_sandbox_is_shown_as_text() {
        assert_eq!(
            super::escape_markup("<b>x</b> & <a href=\"u\">y</a>"),
            "&lt;b&gt;x&lt;/b&gt; &amp; &lt;a href=\"u\"&gt;y&lt;/a&gt;"
        );
    }
}
