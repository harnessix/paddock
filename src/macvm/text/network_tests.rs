use super::super::tests::badge;
use crate::macvm::text as t;

#[test]
fn no_network() {
    assert_eq!(t::no_network(), "no network: the guest device ended");
    badge(&t::no_network());
}

#[test]
fn network_ended() {
    let line = t::network_ended("the device closed");
    assert!(
        line.starts_with("the network device of the guest ended (the device closed)"),
        "{line}"
    );
    assert!(line.contains("without a network"));
}

#[test]
fn drop_target() {
    assert_eq!(
        t::drop_target("tcp", Some("1.1.1.1"), Some(443)),
        "tcp 1.1.1.1:443"
    );
    assert_eq!(
        t::drop_target("icmp", Some("10.0.2.9"), None),
        "icmp 10.0.2.9"
    );
    assert_eq!(t::drop_target("ipv6", None, None), "ipv6");
}

#[test]
fn drop_count() {
    assert_eq!(t::drop_count("tcp 1.1.1.1:443", 3), "tcp 1.1.1.1:443 x3");
}

#[test]
fn drop_summary() {
    assert_eq!(
        t::drop_summary(4, &["tcp 1.1.1.1:443 x3".into(), "ipv6 x1".into()]),
        "dropped 4: tcp 1.1.1.1:443 x3, ipv6 x1"
    );
}

#[test]
fn rule_with_identity() {
    assert_eq!(
        t::rule_with_identity("an address of this host", "app = curl"),
        "an address of this host (app = curl)"
    );
}

#[test]
fn header_too_long() {
    assert_eq!(
        t::header_too_long(7),
        "the header line is longer than 7 bytes"
    );
}

#[test]
fn not_a_header() {
    assert_eq!(t::not_a_header("x"), "not a header line: x");
}

#[test]
fn header_version_mismatch() {
    assert_eq!(
        t::header_version_mismatch(2, 1),
        "header version 2 is not 1"
    );
}

#[test]
fn connect_line() {
    assert_eq!(
        t::connect_line("a:443"),
        "CONNECT a:443 HTTP/1.1\r\nHost: a:443\r\n\r\n"
    );
}
