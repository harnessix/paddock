use super::*;

#[test]
fn a_connect_target_keeps_port_80() {
    assert_eq!(connect_target("h:80"), Some(("h".into(), 80)));
    assert_eq!(
        connect_target("127.0.0.1:80"),
        Some(("127.0.0.1".into(), 80))
    );
    assert_eq!(
        connect_target("example.com:443"),
        Some(("example.com".into(), 443))
    );
    assert_eq!(connect_target("example.com"), None);
    assert_eq!(connect_target("example.com:x"), None);
}

#[test]
fn registry_http_policy_only_forwards_exact_listed_sources() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (head, _) = read_head(&mut stream).unwrap();
        assert!(String::from_utf8_lossy(&head).starts_with("GET /crate HTTP/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .unwrap();
    });
    let (registry, bad) = allow::RegistryHttpPolicy::parse(&[format!("localhost:{port}")]);
    assert!(bad.is_empty());
    let (api_list, _) = allow::Allowlist::parse(&[format!("localhost:{port}")]);
    let (_dir, sock) = start(HostProxy {
        allow_loopback: vec![port],
        api: Some(allow::ApiPolicy {
            list: api_list,
            refusals: None,
        }),
        registry_http: Some(registry),
        ..Default::default()
    });
    assert_eq!(
        request(
            &sock,
            &format!(
                "GET http://localhost:{}/crate HTTP/1.1\r\nHost: localhost\r\n\r\n",
                if port == 1 { 2 } else { 1 }
            )
        )
        .0,
        403
    );
    assert_eq!(
        request(
            &sock,
            &format!("GET http://127.0.0.1:{port}/crate HTTP/1.1\r\nHost: localhost\r\n\r\n")
        )
        .0,
        403
    );
    assert_eq!(
        request(
            &sock,
            &format!("GET http://localhost:{port}/crate HTTP/1.1\r\nHost: localhost\r\n\r\n")
        )
        .0,
        200
    );
    server.join().unwrap();
}

#[test]
fn registry_policy_requires_exact_names_and_explicit_ports() {
    let (policy, bad) = allow::RegistryHttpPolicy::parse(&[
        "localhost:8080",
        "example.com",
        "*.example.com:80",
        "127.0.0.1:80",
    ]);
    assert_eq!(bad.len(), 3);
    assert!(policy.check("localhost", 8080).is_ok());
    assert!(policy.check("localhost", 80).is_err());
    assert!(policy.check("other.localhost", 8080).is_err());
    let cfg = HostProxy {
        registry_http: Some(policy),
        allow_lan: true,
        ..Default::default()
    };
    assert!(check_target(&cfg, "192.168.1.1", 80).is_err());
    assert!(check_target(&cfg, "127.0.0.1", 80).is_err());
}

#[test]
fn private_registry_exception_is_exact_and_keeps_loopback_blocked() {
    let (private, bad) = allow::RegistryHttpPolicy::parse(&["mirror.example:443"]);
    assert!(bad.is_empty());
    assert!(private.check("mirror.example", 443).is_ok());
    assert!(private.check("mirror.example", 80).is_err());
    assert!(private.check("other.example", 443).is_err());
    let cfg = HostProxy {
        registry_http: Some(allow::RegistryHttpPolicy::default()),
        registry_private: Some(private),
        ..Default::default()
    };
    let private_addr = ["192.168.1.10:443".parse().unwrap()];
    assert!(target::check_resolved(&cfg, "mirror.example", 443, &private_addr, &[]).is_ok());
    assert!(target::check_resolved(&cfg, "mirror.example", 80, &private_addr, &[]).is_err());
    assert!(target::check_resolved(&cfg, "other.example", 443, &private_addr, &[]).is_err());
    assert!(target::check_resolved(
        &cfg,
        "mirror.example",
        443,
        &private_addr,
        &["192.168.1.10".parse().unwrap()]
    )
    .is_err());
    assert!(check_target(&cfg, "127.0.0.1", 443).is_err());
    assert!(check_target(&cfg, "192.168.1.1", 443).is_err());
}
