//! Tests of the stub network over fixed frames. Every vector is written
//! out in hex, with its fields in the comment, so a change of a parser
//! shows up as a byte difference and not as a round trip of itself.

use super::*;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex"))
        .collect()
}

/// ARP request, broadcast, from 52:54:00:a0:00:15 10.0.2.15: who has
/// 10.0.2.2.
const ARP_REQ: &str = "ff ff ff ff ff ff 52 54 00 a0 00 15 08 06 \
    00 01 08 00 06 04 00 01 52 54 00 a0 00 15 0a 00 02 0f \
    00 00 00 00 00 00 0a 00 02 02";

/// TCP SYN from 10.0.2.15:49152 to 1.1.1.1:443, to the gateway MAC. The
/// IP header: 45 00, length 40, id 0, DF, TTL 64, proto 6, checksum
/// 0x2cc0 (over the 20 header bytes), src, dst. TCP: ports, seq 1, ack 0,
/// data offset 5, SYN, window 65535, checksum 0 (the stub does not check
/// it), urgent 0.
const TCP_SYN: &str = "52 54 00 a0 00 02 52 54 00 a0 00 15 08 00 \
    45 00 00 28 00 00 40 00 40 06 2c c0 0a 00 02 0f 01 01 01 01 \
    c0 00 01 bb 00 00 00 01 00 00 00 00 50 02 ff ff 00 00 00 00";

/// UDP from 10.0.2.15:5353 to 10.0.2.3:53 (another host, not the
/// gateway) with a 4-byte payload. UDP length 12, checksum 0 (none).
const UDP_OTHER: &str = "52 54 00 a0 00 02 52 54 00 a0 00 15 08 00 \
    45 00 00 20 00 00 40 00 40 11 22 bc 0a 00 02 0f 0a 00 02 03 \
    14 e9 00 35 00 0c 00 00 de ad be ef";

/// DNS query id 0x1234, RD, one question: example.com A IN, from
/// 10.0.2.15:5353 to 10.0.2.2:53. UDP length 37, checksum 0.
const DNS_QUERY: &str = "52 54 00 a0 00 02 52 54 00 a0 00 15 08 00 \
    45 00 00 39 00 00 40 00 40 11 22 a4 0a 00 02 0f 0a 00 02 02 \
    14 e9 00 35 00 25 00 00 \
    12 34 01 00 00 01 00 00 00 00 00 00 \
    07 65 78 61 6d 70 6c 65 03 63 6f 6d 00 00 01 00 01";

/// ICMP echo request id 1 seq 1 with data "abcd" from 10.0.2.15 to the
/// gateway. ICMP checksum 0x3337.
const ICMP_ECHO: &str = "52 54 00 a0 00 02 52 54 00 a0 00 15 08 00 \
    45 00 00 20 00 00 40 00 40 01 22 cd 0a 00 02 0f 0a 00 02 02 \
    08 00 33 37 00 01 00 01 61 62 63 64";

/// An IPv6 frame: a 40-byte header (version 6, next header UDP, hop
/// limit 64, fe80::1 to ff02::1) and no payload.
const IPV6: &str = "33 33 00 00 00 01 52 54 00 a0 00 15 86 dd \
    60 00 00 00 00 00 11 40 \
    fe 80 00 00 00 00 00 00 00 00 00 00 00 00 00 01 \
    ff 02 00 00 00 00 00 00 00 00 00 00 00 00 00 01";

/// A DHCP frame from `mac` to the broadcast address: the BOOTP header
/// with xid 01020304, `flags`, `ciaddr`, `chaddr` = mac, the cookie and
/// `options`, padded to 300 bytes.
fn dhcp_frame(mac: [u8; 6], flags: u16, ciaddr: [u8; 4], options: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let d = Dhcp {
        op: 1,
        xid: [1, 2, 3, 4],
        flags,
        ciaddr,
        yiaddr: [0; 4],
        siaddr: [0; 4],
        chaddr: mac,
        options: options.to_vec(),
    };
    let (src_ip, dst_ip, dst_mac) = if ciaddr == [0; 4] {
        ([0; 4], [255; 4], [0xff; 6])
    } else {
        (ciaddr, GATEWAY_IP, GATEWAY_MAC)
    };
    Ethernet {
        dst: dst_mac,
        src: mac,
        ethertype: ETH_IPV4,
        payload: Ipv4 {
            src: src_ip,
            dst: dst_ip,
            proto: IP_UDP,
            payload: Udp {
                src_port: 68,
                dst_port: 67,
                payload: d.build(),
            }
            .build(src_ip, dst_ip),
        }
        .build(),
    }
    .build()
}

fn discover(mac: [u8; 6]) -> Vec<u8> {
    dhcp_frame(mac, 0x8000, [0; 4], &[(53, vec![1])])
}

fn request(mac: [u8; 6], ip: [u8; 4]) -> Vec<u8> {
    dhcp_frame(
        mac,
        0x8000,
        [0; 4],
        &[(53, vec![3]), (50, ip.to_vec()), (54, GATEWAY_IP.to_vec())],
    )
}

/// The renew: unicast from the leased address, no option 50.
fn renew(mac: [u8; 6]) -> Vec<u8> {
    dhcp_frame(mac, 0, GUEST_IP, &[(53, vec![3])])
}

fn vectors() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("arp", hex(ARP_REQ)),
        ("tcp", hex(TCP_SYN)),
        ("udp", hex(UDP_OTHER)),
        ("dns", hex(DNS_QUERY)),
        ("icmp", hex(ICMP_ECHO)),
        ("ipv6", hex(IPV6)),
        ("dhcp", discover(GUEST_MAC)),
    ]
}

fn one(stub: &mut Stub, frame: &[u8], now: Instant) -> Reply {
    let mut r = stub.handle(frame, now);
    assert_eq!(r.len(), 1);
    r.remove(0)
}

fn sent(stub: &mut Stub, frame: &[u8], now: Instant) -> Vec<u8> {
    match one(stub, frame, now) {
        Reply::Send(f) => f,
        other => panic!("expected a frame, got {other:?}"),
    }
}

fn dropped(stub: &mut Stub, frame: &[u8], now: Instant) -> (Dropped, bool, u64) {
    match one(stub, frame, now) {
        Reply::Dropped { what, first, count } => (what, first, count),
        other => panic!("expected a drop, got {other:?}"),
    }
}

fn udp_of(frame: &[u8]) -> (Ipv4, Udp) {
    let e = Ethernet::parse(frame).unwrap();
    assert_eq!(e.ethertype, ETH_IPV4);
    let ip = Ipv4::parse(&e.payload).unwrap();
    assert_eq!(ip.proto, IP_UDP);
    let udp = Udp::parse(&ip.payload).unwrap();
    (ip, udp)
}

#[test]
fn vectors_round_trip() {
    // Ethernet: every vector parses and builds back to the same bytes.
    for (name, v) in vectors() {
        let e = Ethernet::parse(&v).unwrap_or_else(|| panic!("{name} parses"));
        assert_eq!(e.build(), v, "{name} ethernet round trip");
    }
    let arp = Arp::parse(&Ethernet::parse(&hex(ARP_REQ)).unwrap().payload).unwrap();
    assert_eq!(arp.op, 1);
    assert_eq!(arp.sha, GUEST_MAC);
    assert_eq!(arp.spa, GUEST_IP);
    assert_eq!(arp.tpa, GATEWAY_IP);
    assert_eq!(arp.build(), hex(ARP_REQ)[14..]);

    let ip = Ipv4::parse(&Ethernet::parse(&hex(TCP_SYN)).unwrap().payload).unwrap();
    assert_eq!((ip.src, ip.dst, ip.proto), (GUEST_IP, [1, 1, 1, 1], IP_TCP));
    assert_eq!(ip.payload.len(), 20);
    assert_eq!(
        ip.build(),
        hex(TCP_SYN)[14..],
        "the IP builder makes the same header"
    );

    let (ip, udp) = udp_of(&hex(DNS_QUERY));
    assert_eq!((udp.src_port, udp.dst_port), (5353, 53));
    let q = Dns::parse(&udp.payload).unwrap();
    assert_eq!((q.id, q.flags, q.qdcount), (0x1234, 0x0100, 1));
    assert_eq!(q.rest, hex(DNS_QUERY)[54..]);
    assert_eq!(q.build(), udp.payload);
    // The UDP builder fills the checksum in; the vector has none. The
    // parser reads both.
    let built = udp.build(ip.src, ip.dst);
    assert_eq!(Udp::parse(&built).unwrap(), udp);
    assert_ne!(built[6..8], [0, 0]);

    let e = Ethernet::parse(&hex(ICMP_ECHO)).unwrap();
    let ip = Ipv4::parse(&e.payload).unwrap();
    let echo = IcmpEcho::parse(&ip.payload).unwrap();
    assert_eq!((echo.kind, echo.id, echo.seq), (8, 1, 1));
    assert_eq!(echo.data, b"abcd");
    assert_eq!(echo.build(), ip.payload, "the ICMP checksum of the vector");

    let (_, udp) = udp_of(&discover(GUEST_MAC));
    let d = Dhcp::parse(&udp.payload).unwrap();
    assert_eq!(
        (d.op, d.xid, d.flags, d.chaddr),
        (1, [1, 2, 3, 4], 0x8000, GUEST_MAC)
    );
    assert_eq!(d.msg_type(), Some(1));
    assert_eq!(d.build(), udp.payload);
    assert_eq!(d.build().len(), 300, "padded to the BOOTP minimum");

    // The frames the stub makes parse too.
    let mut stub = Stub::default();
    let now = Instant::now();
    let offer = sent(&mut stub, &discover(GUEST_MAC), now);
    let (_, udp) = udp_of(&offer);
    let o = Dhcp::parse(&udp.payload).unwrap();
    assert_eq!(o.build(), udp.payload);
    assert_eq!(hex("0a 00 02 0f"), GUEST_IP);
}

#[test]
fn truncated_never_panics() {
    let mut stub = Stub::default();
    let now = Instant::now();
    let mut all = vectors();
    all.push(("request", request(GUEST_MAC, GUEST_IP)));
    all.push(("renew", renew(GUEST_MAC)));
    for (name, v) in &all {
        for n in 0..v.len() {
            let r = stub.handle(&v[..n], now);
            assert!(
                matches!(r.as_slice(), [Reply::Dropped { .. }]),
                "{name} cut at {n} bytes is dropped, got {r:?}"
            );
            let e = Ethernet::parse(&v[..n]);
            if let Some(e) = e {
                let _ = Arp::parse(&e.payload);
                if let Some(ip) = Ipv4::parse(&e.payload) {
                    let _ = IcmpEcho::parse(&ip.payload);
                    if let Some(u) = Udp::parse(&ip.payload) {
                        let _ = Dhcp::parse(&u.payload);
                        let _ = Dns::parse(&u.payload);
                    }
                }
            }
        }
    }
    // Cut inside the IP header: the total length says more than there is.
    assert!(Ipv4::parse(&hex(
        "45 00 00 28 00 00 40 00 40 06 00 00 0a 00 02 0f 01 01 01 01"
    ))
    .is_none());
    // A DHCP option whose length runs past the end.
    let mut bad = discover(GUEST_MAC);
    let opt = bad.len() - 300 + 240;
    bad.truncate(opt);
    bad.extend_from_slice(&[53, 200]);
    assert!(matches!(
        stub.handle(&bad, now).as_slice(),
        [Reply::Dropped { .. }]
    ));
    assert_eq!(stub.answered.dhcp_offers, 0);
    assert!(stub.drops.total() > 0);
}

#[test]
fn dhcp_lease_and_renew() {
    let mut stub = Stub::default();
    let t0 = Instant::now();
    let offer = sent(&mut stub, &discover(GUEST_MAC), t0);
    let e = Ethernet::parse(&offer).unwrap();
    assert_eq!(
        (e.dst, e.src),
        ([0xff; 6], GATEWAY_MAC),
        "broadcast flag: a broadcast offer"
    );
    let (ip, udp) = udp_of(&offer);
    assert_eq!((ip.src, ip.dst), (GATEWAY_IP, [255; 4]));
    assert_eq!((udp.src_port, udp.dst_port), (67, 68));
    let o = Dhcp::parse(&udp.payload).unwrap();
    assert_eq!(
        (o.op, o.xid, o.yiaddr, o.siaddr),
        (2, [1, 2, 3, 4], GUEST_IP, GATEWAY_IP)
    );
    assert_eq!(o.msg_type(), Some(2));
    assert_eq!(o.lease_secs(), Some(LEASE_SECS));
    let opt = |c: u8| {
        o.options
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(opt(1), Some(NETMASK.to_vec()), "netmask");
    assert_eq!(opt(3), Some(GATEWAY_IP.to_vec()), "router");
    assert_eq!(opt(6), Some(GATEWAY_IP.to_vec()), "dns");
    assert_eq!(opt(54), Some(GATEWAY_IP.to_vec()), "server id");
    assert_eq!(opt(252), None, "no WPAD");
    assert_eq!(stub.lease_given(), None, "an offer is not the lease yet");

    let ack = sent(&mut stub, &request(GUEST_MAC, GUEST_IP), t0);
    let a = Dhcp::parse(&udp_of(&ack).1.payload).unwrap();
    assert_eq!((a.msg_type(), a.yiaddr), (Some(5), GUEST_IP));
    assert_eq!(stub.lease_given(), Some(t0));

    // A second DISCOVER gets the same lease.
    let again = sent(&mut stub, &discover(GUEST_MAC), t0 + Duration::from_secs(5));
    let o2 = Dhcp::parse(&udp_of(&again).1.payload).unwrap();
    assert_eq!(o2.yiaddr, GUEST_IP);

    // Another MAC gets no answer, and it counts.
    let other = [0x52, 0x54, 0x00, 0xa0, 0x00, 0x99];
    let (d, first, count) = dropped(&mut stub, &discover(other), t0);
    assert_eq!((d.proto, first, count), (Proto::Dhcp, true, 1));
    assert!(matches!(
        one(&mut stub, &request(other, GUEST_IP), t0),
        Reply::Dropped { .. }
    ));

    // A REQUEST for another address gets a NAK.
    let nak = sent(&mut stub, &request(GUEST_MAC, [10, 0, 2, 99]), t0);
    let n = Dhcp::parse(&udp_of(&nak).1.payload).unwrap();
    assert_eq!((n.msg_type(), n.yiaddr), (Some(6), [0; 4]));

    // The renew at half the lease: unicast, answered unicast, lease time
    // moves.
    let half = t0 + Duration::from_secs(u64::from(LEASE_SECS / 2));
    let ack2 = sent(&mut stub, &renew(GUEST_MAC), half);
    let e = Ethernet::parse(&ack2).unwrap();
    assert_eq!(e.dst, GUEST_MAC, "a renew is answered to the guest MAC");
    let (ip, udp) = udp_of(&ack2);
    assert_eq!(ip.dst, GUEST_IP);
    let a2 = Dhcp::parse(&udp.payload).unwrap();
    assert_eq!(
        (a2.msg_type(), a2.yiaddr, a2.lease_secs()),
        (Some(5), GUEST_IP, Some(LEASE_SECS))
    );
    assert_eq!(stub.lease_given(), Some(half));
    assert_eq!(stub.answered.dhcp_acks, 2);
    assert_eq!(stub.answered.dhcp_offers, 2);

    // RELEASE and INFORM are not answered.
    let inform = dhcp_frame(GUEST_MAC, 0, GUEST_IP, &[(53, vec![8])]);
    assert_eq!(dropped(&mut stub, &inform, half).0.proto, Proto::Dhcp);
}

#[test]
fn drops_syn_udp_ipv6_and_counts() {
    let mut stub = Stub::default();
    let t0 = Instant::now();
    let (d, first, count) = dropped(&mut stub, &hex(TCP_SYN), t0);
    assert_eq!(
        d,
        Dropped {
            proto: Proto::Tcp,
            dst: Some([1, 1, 1, 1]),
            port: Some(443)
        }
    );
    assert_eq!(d.target(), "tcp 1.1.1.1:443");
    assert!(first);
    assert_eq!(count, 1);

    let (_, first, count) = dropped(&mut stub, &hex(TCP_SYN), t0 + Duration::from_secs(30));
    assert!(!first, "a second within the minute joins the line");
    assert_eq!(count, 2);
    let (_, first, count) = dropped(&mut stub, &hex(TCP_SYN), t0 + Duration::from_secs(61));
    assert!(first, "after the window a new entry");
    assert_eq!(count, 3);

    let (d, first, _) = dropped(&mut stub, &hex(UDP_OTHER), t0);
    assert_eq!(d.target(), "udp 10.0.2.3:53");
    assert!(first);
    let (d, first, _) = dropped(&mut stub, &hex(IPV6), t0);
    assert_eq!((d.proto, d.dst, d.port), (Proto::Ipv6, None, None));
    assert_eq!(d.target(), "ipv6");
    assert!(first);

    // ARP for another address, an ICMP to another host, another IP
    // protocol and another Ethernet type: all dropped with their kind.
    let mut arp_other = hex(ARP_REQ);
    arp_other[41] = 7;
    assert_eq!(
        dropped(&mut stub, &arp_other, t0).0.target(),
        "arp 10.0.2.7"
    );
    let mut icmp_other = hex(ICMP_ECHO);
    icmp_other[33] = 9;
    assert_eq!(
        dropped(&mut stub, &icmp_other, t0).0.target(),
        "icmp 10.0.2.9"
    );
    let mut proto_47 = hex(ICMP_ECHO);
    proto_47[23] = 47;
    assert_eq!(
        dropped(&mut stub, &proto_47, t0).0.target(),
        "ip-47 10.0.2.2"
    );
    let mut ether = hex(ARP_REQ);
    ether[13] = 0x42;
    assert_eq!(
        dropped(&mut stub, &ether, t0).0.target(),
        "ethertype-0x0842"
    );
    assert_eq!(dropped(&mut stub, b"short", t0).0.proto, Proto::Malformed);

    assert_eq!(stub.drops.total(), 10);
    assert_eq!(stub.drops.lines().len(), 8);
    assert_eq!(
        stub.drops.count(&Dropped {
            proto: Proto::Tcp,
            dst: Some([1, 1, 1, 1]),
            port: Some(443)
        }),
        3
    );
    let s = stub.drops.summary();
    assert!(s.starts_with("dropped 10: "), "{s}");
    assert!(s.contains("tcp 1.1.1.1:443 x3"), "{s}");
    assert!(s.contains("ipv6 x1"), "{s}");
    assert_eq!(
        stub.answered.arp + stub.answered.icmp + stub.answered.dns_refused,
        0
    );
    let json = serde_json::to_value(stub.drops.lines()).unwrap();
    assert!(json.to_string().contains("\"proto\":\"tcp\""), "{json}");
}

#[test]
fn drop_lines_are_capped() {
    let mut stub = Stub::default();
    let t0 = Instant::now();
    let mut syn = hex(TCP_SYN);
    for i in 0..1100u32 {
        syn[30..34].copy_from_slice(&(0x0100_0000u32 + i).to_be_bytes());
        stub.handle(&syn, t0 + Duration::from_millis(u64::from(i)));
    }
    assert_eq!(stub.drops.lines().len(), 1000);
    assert_eq!(
        stub.drops.count(&Dropped {
            proto: Proto::Tcp,
            dst: Some([1, 0, 0, 0]),
            port: Some(443)
        }),
        0,
        "the oldest line went first"
    );
}

#[test]
fn dns_refused_same_id() {
    let mut stub = Stub::default();
    let now = Instant::now();
    let answer = sent(&mut stub, &hex(DNS_QUERY), now);
    let e = Ethernet::parse(&answer).unwrap();
    assert_eq!((e.dst, e.src), (GUEST_MAC, GATEWAY_MAC));
    let (ip, udp) = udp_of(&answer);
    assert_eq!((ip.src, ip.dst), (GATEWAY_IP, GUEST_IP));
    assert_eq!((udp.src_port, udp.dst_port), (53, 5353));
    let a = Dns::parse(&udp.payload).unwrap();
    assert_eq!(a.id, 0x1234);
    assert_eq!(a.rcode(), 5, "REFUSED");
    assert_eq!(a.flags & 0x8000, 0x8000, "a response");
    assert_eq!(a.flags & 0x0100, 0x0100, "RD as asked");
    assert_eq!((a.qdcount, a.ancount, a.nscount, a.arcount), (1, 0, 0, 0));
    assert_eq!(a.rest, hex(DNS_QUERY)[54..], "the question comes back");
    assert_eq!(stub.answered.dns_refused, 1);

    // A DNS to another address is not the gateway's to answer.
    let mut other = hex(DNS_QUERY);
    other[33] = 3;
    assert_eq!(
        dropped(&mut stub, &other, now).0.target(),
        "udp 10.0.2.3:53"
    );
    // A short DNS to the gateway is malformed.
    let mut short = hex(DNS_QUERY);
    short.truncate(14 + 20 + 8 + 6);
    short[14 + 2..14 + 4].copy_from_slice(&34u16.to_be_bytes());
    short[14 + 20 + 4..14 + 20 + 6].copy_from_slice(&14u16.to_be_bytes());
    assert_eq!(dropped(&mut stub, &short, now).0.proto, Proto::Malformed);
}

#[test]
fn arp_and_icmp_echo_to_the_gateway() {
    let mut stub = Stub::default();
    let now = Instant::now();
    let reply = sent(&mut stub, &hex(ARP_REQ), now);
    let e = Ethernet::parse(&reply).unwrap();
    assert_eq!(
        (e.dst, e.src, e.ethertype),
        (GUEST_MAC, GATEWAY_MAC, ETH_ARP)
    );
    let a = Arp::parse(&e.payload).unwrap();
    assert_eq!(
        a,
        Arp {
            op: 2,
            sha: GATEWAY_MAC,
            spa: GATEWAY_IP,
            tha: GUEST_MAC,
            tpa: GUEST_IP
        }
    );

    let reply = sent(&mut stub, &hex(ICMP_ECHO), now);
    let e = Ethernet::parse(&reply).unwrap();
    let ip = Ipv4::parse(&e.payload).unwrap();
    assert_eq!((ip.src, ip.dst, ip.proto), (GATEWAY_IP, GUEST_IP, IP_ICMP));
    let echo = IcmpEcho::parse(&ip.payload).unwrap();
    assert_eq!(
        echo,
        IcmpEcho {
            kind: 0,
            id: 1,
            seq: 1,
            data: b"abcd".to_vec()
        }
    );
    assert_eq!(checksum(&ip.payload), 0, "the reply checksum is right");
    assert_eq!((stub.answered.arp, stub.answered.icmp), (1, 1));
}

#[test]
fn pump_over_a_socket_pair() {
    let (host, guest) = UnixDatagram::pair().unwrap();
    host.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let stub = std::sync::Arc::new(Mutex::new(Stub::default()));
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(Dropped, u64)>();
    let t = {
        let (stub, stop) = (stub.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut sock = Sock(host);
            pump(&mut sock, &stub, &stop, |d, n| {
                let _ = tx.send((d.clone(), n));
            })
        })
    };
    guest.send(&discover(GUEST_MAC)).unwrap();
    let mut buf = vec![0u8; FRAME_MAX];
    let n = guest.recv(&mut buf).unwrap();
    let o = Dhcp::parse(&udp_of(&buf[..n]).1.payload).unwrap();
    assert_eq!(o.yiaddr, GUEST_IP);
    guest.send(&hex(TCP_SYN)).unwrap();
    guest.send(&hex(TCP_SYN)).unwrap();
    let (d, count) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((d.target(), count), ("tcp 1.1.1.1:443".to_string(), 1));
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "the second SYN in the minute makes no new entry"
    );
    assert_eq!(stub.lock().unwrap().drops.total(), 2);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(t.join().unwrap().is_ok(), "stop ends the pump with Ok");
}

#[test]
fn network_writes_the_boundary_log_and_reports_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("logs").join("boundary.jsonl");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let (host, guest) = UnixDatagram::pair().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let net = Network::start(
        host,
        log.clone(),
        Box::new(move |why| {
            let _ = tx.send(why.to_string());
        }),
    );
    guest.send(&hex(TCP_SYN)).unwrap();
    guest.send(&hex(TCP_SYN)).unwrap();
    guest.send(&hex(IPV6)).unwrap();
    guest.send(&discover(GUEST_MAC)).unwrap();
    let mut buf = vec![0u8; FRAME_MAX];
    guest.recv(&mut buf).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let entries = loop {
        let e = crate::boundary::read_at(&log);
        if e.len() >= 2 || Instant::now() > deadline {
            break e;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert_eq!(entries[0].kind, "network");
    assert_eq!(entries[0].target, "tcp 1.1.1.1:443");
    assert_eq!(entries[0].rule, RULE);
    assert_eq!(entries[0].count, 1, "one entry per minute per target");
    assert_eq!(entries[1].target, "ipv6");
    let st = net.status();
    assert_eq!(st["dropped_total"], 3);
    assert_eq!(st["ended"], serde_json::Value::Null);
    assert_eq!(st["answered"]["dhcp_offers"], 1);
    assert!(st.to_string().contains("\"proto\":\"tcp\""), "{st}");
    // The guest end goes away while a reply is due: the send fails, the
    // pump ends and says so. The stub lock holds the pump between its
    // read and its reply, so the close comes first for sure.
    {
        let held = net.stub.lock().unwrap();
        guest.send(&hex(ARP_REQ)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(guest);
        drop(held);
    }
    let why = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!why.is_empty());
    assert_eq!(net.ended(), Some(why));
    assert_ne!(net.status()["ended"], serde_json::Value::Null);
}

/// `UnixDatagram` as a `Read + Write`.
struct Sock(UnixDatagram);

impl std::io::Read for Sock {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv(buf)
    }
}

impl std::io::Write for Sock {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
