//! The texts of the links and the login callbacks of the guest: the link
//! helper and the agent (`guest/link.rs`), the session (`login_route.rs`,
//! `run_login.rs`). No text holds the path or the query of a link, which
//! carry the state and the code of a login, except the approval card,
//! which shows the whole link as the card of the Linux route does.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// A link longer than the limit.
pub fn link_too_long(len: usize, max: usize) -> String {
    format!("the link has {len} bytes, more than the {max} bytes that paddock takes")
}

/// A link that is not http or https.
pub fn link_not_web() -> String {
    "only http and https links open in the host browser".into()
}

/// The helper's line did not arrive whole.
pub fn link_unreadable(detail: &str) -> String {
    format!("cannot read the link from the link helper: {detail}")
}

/// Something that is not a socket stands where the agent's socket goes.
pub fn link_socket_taken(path: &Path) -> String {
    format!(
        "{} is not a socket: remove it, and the agent makes its link socket at the next start",
        path.display()
    )
}

/// The helper cannot reach the agent.
pub fn link_no_agent(path: &Path, detail: &str) -> String {
    format!(
        "cannot reach the paddock agent at {}: {detail}; the link stays closed",
        path.display()
    )
}

/// The agent closed the helper's connection without an answer.
pub fn link_no_answer() -> String {
    "the paddock agent did not answer the link helper".into()
}

/// The agent has no connection to the host now.
pub fn link_no_host_connection() -> String {
    "the guest has no connection to the host now, so the link stays closed; open it again in a moment".into()
}

/// A `login.forward` with port 0 or a ticket that is not hex.
pub fn login_forward_bad() -> String {
    "the login forward needs a port and a ticket of hex digits".into()
}

/// A `login.forward` before the session token.
pub fn login_forward_no_token() -> String {
    "the login forward came before the session token".into()
}

/// No guest process listens on the callback port.
pub fn login_nothing_listens(port: u16, detail: &str) -> String {
    format!("no guest program listens on 127.0.0.1:{port} or [::1]:{port} ({detail}), so the callback is not forwarded")
}

/// The agent cannot dial the `guest-login` port of the host.
pub fn login_no_host(port: u16, detail: &str) -> String {
    format!("cannot dial the host for the callback of port {port}: {detail}")
}

/// `open-url = "deny"` in `[host]`.
pub fn link_mode_deny() -> String {
    "open-url is deny in [host] of ~/.config/paddock/config.toml".into()
}

/// The user chose "deny for this session" for an earlier link.
pub fn link_denied_for_session() -> String {
    "you denied links of this session for the rest of it".into()
}

/// The summary line of the card.
pub fn link_card_summary(url: &str) -> String {
    format!("open {url}")
}

/// The detail of the card.
pub fn link_card_detail(url: &str) -> String {
    format!("Open in the host browser:\n{url}")
}

/// The reason of the card: who asks.
pub fn link_card_reason(label: &str) -> String {
    format!("{label} in paddock's macOS guest opened this link")
}

/// `config.toml` cannot be read, so no link opens.
pub fn link_policy_unreadable(kind: &str, detail: &str) -> String {
    format!("the link {kind} stays closed: {detail}. Fix ~/.config/paddock/config.toml and open the link again")
}

/// A link that the rules refuse.
pub fn link_refused(kind: &str, why: &str) -> String {
    format!("the link {kind} stays closed: {why}")
}

/// Too many links wait for an answer.
pub fn link_too_many_asks(kind: &str, max: usize) -> String {
    format!("the link {kind} stays closed: {max} links of this session wait for an answer already")
}

/// The user denied the link.
pub fn link_denied(kind: &str) -> String {
    format!("you denied the link {kind}")
}

/// Nobody answered the card.
pub fn link_unanswered(kind: &str, secs: u64) -> String {
    format!("the link {kind} stays closed: nobody answered within {secs} seconds")
}

/// The host browser did not start.
pub fn link_open_failed(kind: &str, detail: &str) -> String {
    format!("cannot open the link {kind} in the host browser: {detail}")
}

/// The link opened in the host browser.
pub fn link_opened(kind: &str, ports: &[u16]) -> String {
    if ports.is_empty() {
        format!("opened the link {kind} in the host browser")
    } else {
        let list: Vec<String> = ports.iter().map(u16::to_string).collect();
        format!(
            "opened the link {kind} in the host browser; its login callback port {} listens on the host loopback",
            list.join(", ")
        )
    }
}

/// The check mode: the session took the link but did not start a browser.
pub fn link_dry(kind: &str) -> String {
    format!(
        "took the link {kind} without a browser (PADDOCK_MACVM_LINK_DRY=1 in a --bench session)"
    )
}

/// A callback port listens.
pub fn login_listening(port: u16, addrs: &[SocketAddr]) -> String {
    let list: Vec<String> = addrs.iter().map(SocketAddr::to_string).collect();
    format!(
        "the login callback port {port} listens on {} for {} minutes",
        list.join(" and "),
        crate::login::WINDOW.as_secs() / 60
    )
}

/// A callback port cannot listen.
pub fn login_bind_failed(port: u16, why: &str) -> String {
    format!("cannot listen on the login callback port {port} of the host loopback ({why}): a host program holds it, so the login cannot finish in the guest")
}

/// A host connection waited for the guest in vain.
pub fn login_no_dial(port: u16, wait: Duration) -> String {
    format!(
        "a connection to the login callback port {port} closed: the guest did not take it within {} s",
        wait.as_secs()
    )
}

/// One forwarded connection ended.
pub fn login_forwarded(port: u16, up: u64, down: u64) -> String {
    format!("forwarded a connection of the login callback port {port} into the guest: {up} bytes in, {down} bytes back")
}

/// The first callback completed.
pub fn login_completed(port: u16, grace: Duration) -> String {
    format!(
        "the login callback on port {port} completed; the port closes in {} s",
        grace.as_secs()
    )
}

/// A callback port stopped listening.
pub fn login_closed(port: u16, completed: bool) -> String {
    if completed {
        format!("the login callback port {port} closed after its callback")
    } else {
        format!("the login callback port {port} closed after its window, without a callback")
    }
}

/// An address that is not loopback.
pub fn login_not_loopback() -> String {
    "a login callback listens only on a loopback address".into()
}

/// The link helper of the guest failed.
pub fn link_helper_failed(detail: &str) -> String {
    format!("the paddock link helper could not hand the link to the agent: {detail}")
}

/// The LaunchServices file has no root dict.
pub fn launch_services_no_dict() -> String {
    "the property list has no root dict".into()
}

/// `LSHandlers` is not an array.
pub fn launch_services_no_array() -> String {
    "LSHandlers of the property list is not an array".into()
}

/// The file cannot be read as a property list.
pub fn launch_services_unreadable(path: &Path) -> String {
    format!("cannot read {} as a property list", path.display())
}

/// The merge refused the file.
pub fn launch_services_bad(path: &Path, why: &str) -> String {
    format!("cannot add the link helper to {}: {why}", path.display())
}

/// The default browser of the guest is the link helper now.
pub fn launch_services_written(merged: bool) -> String {
    if merged {
        "the link helper is the default browser of the guest account; the other handlers stayed"
            .into()
    } else {
        "the link helper is the default browser of the guest account".into()
    }
}

/// The default browser could not be set.
pub fn launch_services_failed(detail: &str) -> String {
    format!("the link helper is not the default browser of the guest ({detail}): links in the guest open in its own Safari, and a login callback of a guest app cannot finish")
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
