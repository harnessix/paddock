//! One test per function of text/login.rs, named as the function.

use super::super::login as t;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

#[test]
fn link_too_long() {
    assert!(t::link_too_long(40000, 32768).contains("40000 bytes"));
}

#[test]
fn link_not_web() {
    assert!(t::link_not_web().contains("http and https"));
}

#[test]
fn link_unreadable() {
    assert!(t::link_unreadable("closed").contains("closed"));
}

#[test]
fn link_socket_taken() {
    assert!(
        t::link_socket_taken(Path::new("/x/link.sock")).contains("/x/link.sock is not a socket")
    );
}

#[test]
fn link_no_agent() {
    let s = t::link_no_agent(Path::new("/x/link.sock"), "refused");
    assert!(s.contains("/x/link.sock") && s.contains("refused"), "{s}");
}

#[test]
fn link_no_answer() {
    assert!(t::link_no_answer().contains("did not answer"));
}

#[test]
fn link_no_host_connection() {
    assert!(t::link_no_host_connection().contains("no connection to the host"));
}

#[test]
fn login_forward_bad() {
    assert!(t::login_forward_bad().contains("ticket"));
}

#[test]
fn login_forward_no_token() {
    assert!(t::login_forward_no_token().contains("token"));
}

#[test]
fn login_nothing_listens() {
    let s = t::login_nothing_listens(1455, "refused");
    assert!(
        s.contains("127.0.0.1:1455") && s.contains("[::1]:1455"),
        "{s}"
    );
}

#[test]
fn login_no_host() {
    assert!(t::login_no_host(1455, "timed out").contains("port 1455"));
}

#[test]
fn link_mode_deny() {
    assert!(t::link_mode_deny().contains("open-url is deny"));
}

#[test]
fn link_denied_for_session() {
    assert!(t::link_denied_for_session().contains("this session"));
}

#[test]
fn link_card_summary() {
    assert_eq!(
        t::link_card_summary("https://a.example/x"),
        "open https://a.example/x"
    );
}

#[test]
fn link_card_detail() {
    assert!(t::link_card_detail("https://a.example/x").ends_with("\nhttps://a.example/x"));
}

#[test]
fn link_card_reason() {
    assert!(
        t::link_card_reason("Sample Notes").starts_with("Sample Notes in paddock's macOS guest")
    );
}

#[test]
fn link_policy_unreadable() {
    let s = t::link_policy_unreadable("https://a.example (19 characters)", "bad toml");
    assert!(s.contains("bad toml") && s.contains("config.toml"), "{s}");
}

#[test]
fn link_refused() {
    assert!(t::link_refused("k", "why").contains("stays closed: why"));
}

#[test]
fn link_too_many_asks() {
    assert!(t::link_too_many_asks("k", 4).contains("4 links"));
}

#[test]
fn link_denied() {
    assert_eq!(t::link_denied("k"), "you denied the link k");
}

#[test]
fn link_unanswered() {
    assert!(t::link_unanswered("k", 120).contains("120 seconds"));
}

#[test]
fn link_open_failed() {
    assert!(t::link_open_failed("k", "no open").contains("no open"));
}

#[test]
fn link_opened() {
    assert_eq!(
        t::link_opened("k", &[]),
        "opened the link k in the host browser"
    );
    assert!(t::link_opened("k", &[1455]).contains("port 1455 listens"));
}

#[test]
fn link_dry() {
    assert!(t::link_dry("k").contains("PADDOCK_MACVM_LINK_DRY=1"));
}

#[test]
fn login_listening() {
    let a: SocketAddr = "127.0.0.1:1455".parse().unwrap();
    let b: SocketAddr = "[::1]:1455".parse().unwrap();
    let s = t::login_listening(1455, &[a, b]);
    assert!(s.contains("127.0.0.1:1455 and [::1]:1455"), "{s}");
    assert!(s.contains("for 10 minutes"), "{s}");
}

#[test]
fn login_bind_failed() {
    assert!(t::login_bind_failed(1455, "in use").contains("port 1455"));
}

#[test]
fn login_no_dial() {
    assert!(t::login_no_dial(1455, Duration::from_secs(10)).contains("within 10 s"));
}

#[test]
fn login_forwarded() {
    assert!(t::login_forwarded(1455, 300, 120).contains("300 bytes in, 120 bytes back"));
}

#[test]
fn login_completed() {
    assert!(t::login_completed(1455, Duration::from_secs(10)).contains("closes in 10 s"));
}

#[test]
fn login_closed() {
    assert!(t::login_closed(1455, true).contains("after its callback"));
    assert!(t::login_closed(1455, false).contains("without a callback"));
}

#[test]
fn login_not_loopback() {
    assert!(t::login_not_loopback().contains("loopback"));
}

#[test]
fn link_helper_failed() {
    assert!(t::link_helper_failed("x").contains("link helper"));
}

#[test]
fn launch_services_no_dict() {
    assert!(t::launch_services_no_dict().contains("root dict"));
}

#[test]
fn launch_services_no_array() {
    assert!(t::launch_services_no_array().contains("LSHandlers"));
}

#[test]
fn launch_services_unreadable() {
    assert!(t::launch_services_unreadable(Path::new("/x.plist")).contains("/x.plist"));
}

#[test]
fn launch_services_bad() {
    assert!(t::launch_services_bad(Path::new("/x.plist"), "why").ends_with(": why"));
}

#[test]
fn launch_services_written() {
    assert!(t::launch_services_written(true).contains("other handlers stayed"));
    assert!(!t::launch_services_written(false).contains("other"));
}

#[test]
fn launch_services_failed() {
    assert!(t::launch_services_failed("x").contains("own Safari"));
}
