use super::*;

#[test]
fn egress_reaches_only_the_proxy_relay_and_allowed_host_ports() {
    let (env, mut s) = spec();
    s.add_host_port(8765, "manifest");
    set_proxy(&mut s, 41000);
    let sb = profile(&env, &s);
    let outbound: Vec<&str> = sb
        .lines()
        .filter(|l| l.starts_with("(allow network-outbound (remote ip"))
        .collect();
    assert_eq!(
        outbound,
        [
            "(allow network-outbound (remote ip \"localhost:8765\"))",
            "(allow network-outbound (remote ip \"localhost:41000\"))",
        ]
    );
    let vars = run_env(&env, &s, &private_tmp(&s));
    assert_eq!(vars["HTTPS_PROXY"], "http://127.0.0.1:41000");
    assert_eq!(vars["http_proxy"], "http://127.0.0.1:41000");
    assert!(vars["NO_PROXY"].contains("127.0.0.1"));
}

#[test]
fn api_reaches_only_the_proxy_relay() {
    let (env, mut s) = spec();
    s.network = Network::Api;
    s.add_host_port(8765, "manifest");
    set_proxy(&mut s, 41000);
    let sb = profile(&env, &s);
    assert!(sb.contains("(remote ip \"localhost:41000\")"));
    assert!(!sb.contains("localhost:8765"));
}

#[test]
fn allowlist_reaches_the_proxy_relay_and_allowed_host_ports() {
    let (env, mut s) = spec();
    s.network = Network::Allowlist;
    assert!(refuse(&s).is_ok());
    assert_eq!(net(Network::Allowlist).unwrap(), Net::Localhost);
    s.add_host_port(8765, "manifest");
    set_proxy(&mut s, 41000);
    let sb = profile(&env, &s);
    assert!(sb.contains("(remote ip \"localhost:41000\")"));
    assert!(sb.contains("(remote ip \"localhost:8765\")"));
    let vars = run_env(&env, &s, &private_tmp(&s));
    assert_eq!(vars["HTTPS_PROXY"], "http://127.0.0.1:41000");
}

#[test]
fn auto_picks_seatbelt_for_allowlist_on_macos_and_srt_on_linux() {
    use crate::backend::auto_order;
    assert_eq!(auto_order(Network::Allowlist, true), [NAME]);
    assert_eq!(auto_order(Network::Allowlist, false), ["srt"]);
    assert_eq!(auto_order(Network::Egress, true)[0], NAME);
    assert_eq!(auto_order(Network::Api, false)[0], "bwrap");
}

#[test]
fn the_proxy_enforces_the_list_in_modes_api_and_allowlist() {
    let (_, mut s) = spec();
    s.allow_domains = vec!["api.example.com".into(), "*.cdn.example.com".into()];
    for (mode, want) in [
        (Network::Egress, None),
        (Network::None, None),
        (Network::Api, Some("api")),
        (Network::Allowlist, Some("allowlist")),
    ] {
        s.network = mode;
        let p = crate::backend::listed_policy(&s, None);
        assert_eq!(p.as_ref().map(|p| p.mode()), want, "{mode:?}");
        if let Some(p) = p {
            assert!(p.list.check("api.example.com", 443).is_ok());
            assert!(p.list.check("a.cdn.example.com", 443).is_ok());
            assert!(p.list.check("other.example.com", 443).is_err());
        }
    }
}

#[test]
fn ip_entries_are_named_where_the_proxy_enforces_the_list() {
    use crate::backend::ignored_entries;
    let d = vec!["api.example.com".to_string(), "10.0.0.1".to_string()];
    let w = ignored_entries(Network::Allowlist, &d, true).unwrap();
    assert!(
        w.starts_with("network mode allowlist ignores these entries: 10.0.0.1"),
        "{w}"
    );
    assert!(ignored_entries(Network::Api, &d, false).is_some());
    // srt enforces the list on Linux and takes IP entries.
    assert!(ignored_entries(Network::Allowlist, &d, false).is_none());
    assert!(ignored_entries(Network::Egress, &d, true).is_none());
    assert!(ignored_entries(Network::Api, &d[..1], true).is_none());
}

/// Mode allowlist: the proxy behind the relay refuses a host outside the
/// list, and the boundary log names it. Only the profile needs a Mac, so
/// this runs on every platform.
#[test]
fn the_allowlist_proxy_refuses_a_host_outside_the_list() {
    use std::io::{Read, Write};
    let (_t, env, mut s) = temp_spec("");
    s.network = Network::Allowlist;
    s.allow_domains = vec!["api.example.com".into()];
    std::fs::create_dir_all(env.paddock_run().join(&s.id)).unwrap();
    start_proxy(&env, &mut s).unwrap();
    let relay = s
        .host_ports
        .iter()
        .find(|(_, o)| o == RELAY_ORIGIN)
        .unwrap()
        .0;
    assert_eq!(
        s.env["HTTPS_PROXY"].value,
        format!("http://127.0.0.1:{relay}")
    );
    let mut c = std::net::TcpStream::connect(("127.0.0.1", relay)).unwrap();
    c.write_all(b"CONNECT other.example.com:443 HTTP/1.1\r\nHost: other.example.com:443\r\n\r\n")
        .unwrap();
    let mut got = String::new();
    let _ = c.read_to_string(&mut got);
    assert!(got.starts_with("HTTP/1.1 403"), "{got}");
    let log = crate::boundary::read(&env, &s.id);
    assert!(
        log.iter().any(|e| e.target == "other.example.com:443"
            && e.rule == "network mode allowlist: not on the list"),
        "{log:?}"
    );
}

#[test]
fn network_none_has_no_ip_rule_and_no_proxy() {
    let (env, mut s) = spec();
    s.network = Network::None;
    s.set_env("HTTPS_PROXY", "http://127.0.0.1:7890", "host");
    s.add_host_port(7890, "host proxy");
    let sb = profile(&env, &s);
    assert!(!sb.contains("(remote ip"));
    assert!(!sb.contains("(local ip"));
    let vars = run_env(&env, &s, &private_tmp(&s));
    assert!(!vars.contains_key("HTTPS_PROXY"));
}

#[test]
fn network_host_reaches_everything_with_the_host_proxy() {
    let (env, mut s) = spec();
    s.network = Network::Host;
    s.set_env(
        "HTTPS_PROXY",
        "http://proxy.example:3128",
        "network: host proxy",
    );
    assert!(refuse(&s).is_ok());
    assert_eq!(net(Network::Host).unwrap(), Net::Host);
    let sb = profile(&env, &s);
    assert!(sb.contains("(allow network-outbound (remote ip \"*:*\"))"));
    assert!(sb.contains("(allow network-bind (local ip \"*:*\"))"));
    assert!(sb.contains("(allow network-inbound (local ip \"*:*\"))"));
    assert!(sb.contains("mDNSResponder"));
    let vars = run_env(&env, &s, &private_tmp(&s));
    assert_eq!(vars["HTTPS_PROXY"], "http://proxy.example:3128");
    assert!(HOST_WARNING.starts_with("network mode host"));
    assert!(!HOST_WARNING.contains('\n'));
}
