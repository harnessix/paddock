//! The stub network of the macOS guest: the host end of the guest's network
//! device. It answers ARP for the gateway, one DHCP lease and its renew,
//! DNS with `REFUSED`, and ICMP echo to the gateway. Every other frame is
//! dropped and counted by protocol, address and port, one count line per
//! minute per target, so the boundary log names an app that ignores the
//! proxy and the target it tried.
//!
//! Pure Rust over Ethernet II frames, one frame per datagram, as the
//! `VZFileHandleNetworkDeviceAttachment` sends them. Each parser returns
//! `None` for a short or wrong frame, so a truncated frame is dropped and
//! never panics. Each builder round trips through its parser.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The MAC of the guest device, set in the VM configuration.
pub const GUEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xa0, 0x00, 0x15];
/// `GUEST_MAC` as the framework wants it.
pub const GUEST_MAC_TEXT: &str = "52:54:00:a0:00:15";
/// The MAC that answers for the gateway.
pub const GATEWAY_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xa0, 0x00, 0x02];
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
pub const GUEST_IP: [u8; 4] = [10, 0, 2, 15];
pub const NETMASK: [u8; 4] = [255, 255, 255, 0];
/// The lease length: one day. The guest renews at half of it.
pub const LEASE_SECS: u32 = 86_400;
/// The MTU of the device, and the longest frame with its header.
pub const MTU: usize = 1500;
pub const FRAME_MAX: usize = MTU + 14;
/// Dropped frames of one target within this window share a count line.
pub const MERGE: Duration = Duration::from_secs(60);
/// The rule text of the boundary entries.
pub const RULE: &str = "no route: the guest reaches the network only through paddock's proxy";
/// At most this many drop lines are kept. The oldest goes first.
const DROP_LINES: usize = 1000;

mod frames;
pub use frames::*;

/// The lease the stub gives: the guest's address, the gateway that also
/// answers DNS, the mask and the length. Only `mac` gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub gateway: [u8; 4],
    pub netmask: [u8; 4],
    pub seconds: u32,
}

impl Default for Lease {
    fn default() -> Self {
        Self {
            mac: GUEST_MAC,
            ip: GUEST_IP,
            gateway: GATEWAY_IP,
            netmask: NETMASK,
            seconds: LEASE_SECS,
        }
    }
}

/// The protocol of a dropped frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Proto {
    Tcp,
    Udp,
    Icmp,
    Ipv6,
    Arp,
    /// A DHCP message the stub does not answer: another MAC, or a type
    /// other than DISCOVER and REQUEST.
    Dhcp,
    /// Another IP protocol number.
    Ip(u8),
    /// Another Ethernet type.
    Ether(u16),
    /// A frame too short or wrong for its own header.
    Malformed,
}

impl fmt::Display for Proto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Proto::Tcp => f.write_str("tcp"),
            Proto::Udp => f.write_str("udp"),
            Proto::Icmp => f.write_str("icmp"),
            Proto::Ipv6 => f.write_str("ipv6"),
            Proto::Arp => f.write_str("arp"),
            Proto::Dhcp => f.write_str("dhcp"),
            Proto::Ip(n) => write!(f, "ip-{n}"),
            Proto::Ether(n) => write!(f, "ethertype-{n:#06x}"),
            Proto::Malformed => f.write_str("malformed"),
        }
    }
}

/// A dropped frame: its protocol and target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Dropped {
    pub proto: Proto,
    /// The IPv4 target, when the frame had one.
    pub dst: Option<[u8; 4]>,
    /// The target port, for TCP and UDP.
    pub port: Option<u16>,
}

impl Dropped {
    fn new(proto: Proto, dst: Option<[u8; 4]>, port: Option<u16>) -> Self {
        Self { proto, dst, port }
    }

    /// The target as the boundary log shows it: `tcp 1.1.1.1:443`,
    /// `udp 10.0.2.3:53`, `ipv6`.
    pub fn target(&self) -> String {
        let dst = self.dst.map(|ip| ip_text(&ip));
        super::text::drop_target(&self.proto.to_string(), dst.as_deref(), self.port)
    }
}

/// What the stub does with one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// A frame to send to the guest.
    Send(Vec<u8>),
    /// The frame was dropped and counted. `first` is true when it opened
    /// a new window for its target: one boundary entry per window.
    Dropped {
        what: Dropped,
        first: bool,
        count: u64,
    },
}

/// One count line of the drops: a target, its first time in the current
/// window, and the counts.
#[derive(Debug, Clone, Serialize)]
pub struct DropLine {
    pub proto: Proto,
    pub dst: Option<String>,
    pub port: Option<u16>,
    /// Since the stub started.
    pub count: u64,
    /// In the current window.
    pub recent: u64,
    #[serde(skip)]
    window: Instant,
}

/// The drop counts, merged per minute per target.
#[derive(Debug, Default)]
pub struct Drops {
    lines: BTreeMap<Dropped, DropLine>,
}

impl Drops {
    /// Count one drop. True when it opens a new window for its target:
    /// the caller writes one boundary entry then, and none for the rest
    /// of the window.
    pub fn note(&mut self, d: &Dropped, now: Instant) -> bool {
        if let Some(line) = self.lines.get_mut(d) {
            line.count += 1;
            if now.saturating_duration_since(line.window) < MERGE {
                line.recent += 1;
                return false;
            }
            line.window = now;
            line.recent = 1;
            return true;
        }
        if self.lines.len() >= DROP_LINES {
            let oldest = self
                .lines
                .iter()
                .min_by_key(|(_, l)| l.window)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.lines.remove(&k);
            }
        }
        self.lines.insert(
            d.clone(),
            DropLine {
                proto: d.proto,
                dst: d.dst.map(|ip| ip_text(&ip)),
                port: d.port,
                count: 1,
                recent: 1,
                window: now,
            },
        );
        true
    }

    /// Every line, for the status.
    pub fn lines(&self) -> Vec<&DropLine> {
        self.lines.values().collect()
    }

    /// The count of one target since the start.
    pub fn count(&self, d: &Dropped) -> u64 {
        self.lines.get(d).map(|l| l.count).unwrap_or(0)
    }

    /// Every drop since the start.
    pub fn total(&self) -> u64 {
        self.lines.values().map(|l| l.count).sum()
    }

    /// One line for the log: `dropped 3: tcp 1.1.1.1:443 x2, ipv6 x1`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self
            .lines
            .iter()
            .map(|(d, l)| super::text::drop_count(&d.target(), l.count))
            .collect();
        super::text::drop_summary(self.total(), &parts)
    }
}

/// What the stub answered, for the status.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Answered {
    pub arp: u64,
    pub dhcp_offers: u64,
    pub dhcp_acks: u64,
    pub dns_refused: u64,
    pub icmp: u64,
}

/// The stub: the gateway's MAC, the one lease, and the counts.
#[derive(Debug)]
pub struct Stub {
    gateway_mac: [u8; 6],
    lease: Lease,
    /// When the lease was last given or renewed.
    lease_given: Option<Instant>,
    pub answered: Answered,
    pub drops: Drops,
}

impl Stub {
    pub fn new(gateway_mac: [u8; 6], lease: Lease) -> Self {
        Self {
            gateway_mac,
            lease,
            lease_given: None,
            answered: Answered::default(),
            drops: Drops::default(),
        }
    }

    pub fn lease(&self) -> &Lease {
        &self.lease
    }

    /// When the lease was given, or renewed, last.
    pub fn lease_given(&self) -> Option<Instant> {
        self.lease_given
    }

    /// Handle one frame from the guest at `now`. Every drop is counted in
    /// `drops` and returned as `Reply::Dropped`.
    pub fn handle(&mut self, frame: &[u8], now: Instant) -> Vec<Reply> {
        let reply = match self.answer(frame, now) {
            Ok(frame) => Reply::Send(frame),
            Err(d) => {
                let first = self.drops.note(&d, now);
                let count = self.drops.count(&d);
                Reply::Dropped {
                    what: d,
                    first,
                    count,
                }
            }
        };
        vec![reply]
    }

    fn answer(&mut self, frame: &[u8], now: Instant) -> Result<Vec<u8>, Dropped> {
        let malformed = || Dropped::new(Proto::Malformed, None, None);
        let eth = Ethernet::parse(frame).ok_or_else(malformed)?;
        match eth.ethertype {
            ETH_ARP => {
                let arp = Arp::parse(&eth.payload).ok_or_else(malformed)?;
                if arp.op != 1 || arp.tpa != self.lease.gateway {
                    return Err(Dropped::new(Proto::Arp, Some(arp.tpa), None));
                }
                self.answered.arp += 1;
                let reply = Arp {
                    op: 2,
                    sha: self.gateway_mac,
                    spa: self.lease.gateway,
                    tha: arp.sha,
                    tpa: arp.spa,
                };
                Ok(self.eth(eth.src, ETH_ARP, &reply.build()))
            }
            ETH_IPV4 => self.ipv4(&eth, now),
            ETH_IPV6 => Err(Dropped::new(Proto::Ipv6, None, None)),
            other => Err(Dropped::new(Proto::Ether(other), None, None)),
        }
    }

    fn ipv4(&mut self, eth: &Ethernet, now: Instant) -> Result<Vec<u8>, Dropped> {
        let malformed = || Dropped::new(Proto::Malformed, None, None);
        let ip = Ipv4::parse(&eth.payload).ok_or_else(malformed)?;
        match ip.proto {
            IP_UDP => {
                let udp = Udp::parse(&ip.payload).ok_or_else(malformed)?;
                if udp.dst_port == 67 {
                    return self.dhcp(eth, &udp, now);
                }
                if udp.dst_port == 53 && ip.dst == self.lease.gateway {
                    let q = Dns::parse(&udp.payload).ok_or_else(malformed)?;
                    self.answered.dns_refused += 1;
                    let answer = Udp {
                        src_port: 53,
                        dst_port: udp.src_port,
                        payload: q.refused().build(),
                    };
                    let packet = Ipv4 {
                        src: self.lease.gateway,
                        dst: ip.src,
                        proto: IP_UDP,
                        payload: answer.build(self.lease.gateway, ip.src),
                    };
                    return Ok(self.eth(eth.src, ETH_IPV4, &packet.build()));
                }
                Err(Dropped::new(Proto::Udp, Some(ip.dst), Some(udp.dst_port)))
            }
            IP_TCP => {
                if ip.payload.len() < 4 {
                    return Err(malformed());
                }
                let port = u16::from_be_bytes([ip.payload[2], ip.payload[3]]);
                Err(Dropped::new(Proto::Tcp, Some(ip.dst), Some(port)))
            }
            IP_ICMP => {
                let echo = IcmpEcho::parse(&ip.payload).ok_or_else(malformed)?;
                if echo.kind != 8 || ip.dst != self.lease.gateway {
                    return Err(Dropped::new(Proto::Icmp, Some(ip.dst), None));
                }
                self.answered.icmp += 1;
                let reply = IcmpEcho { kind: 0, ..echo };
                let packet = Ipv4 {
                    src: self.lease.gateway,
                    dst: ip.src,
                    proto: IP_ICMP,
                    payload: reply.build(),
                };
                Ok(self.eth(eth.src, ETH_IPV4, &packet.build()))
            }
            other => Err(Dropped::new(Proto::Ip(other), Some(ip.dst), None)),
        }
    }

    fn dhcp(&mut self, eth: &Ethernet, udp: &Udp, now: Instant) -> Result<Vec<u8>, Dropped> {
        let d =
            Dhcp::parse(&udp.payload).ok_or_else(|| Dropped::new(Proto::Malformed, None, None))?;
        let other = || Dropped::new(Proto::Dhcp, None, None);
        if d.op != 1 || d.chaddr != self.lease.mac {
            return Err(other());
        }
        let kind = match d.msg_type() {
            Some(1) => 2, // DISCOVER gets OFFER
            Some(3) => {
                // REQUEST gets ACK for the lease, NAK for another address.
                let wanted = d.requested_ip().unwrap_or(d.ciaddr);
                if wanted == [0, 0, 0, 0] || wanted == self.lease.ip {
                    5
                } else {
                    6
                }
            }
            _ => return Err(other()),
        };
        let mut options = vec![(53, vec![kind]), (54, self.lease.gateway.to_vec())];
        if kind != 6 {
            options.push((51, self.lease.seconds.to_be_bytes().to_vec()));
            options.push((58, (self.lease.seconds / 2).to_be_bytes().to_vec()));
            options.push((1, self.lease.netmask.to_vec()));
            options.push((3, self.lease.gateway.to_vec()));
            options.push((6, self.lease.gateway.to_vec()));
        }
        let reply = Dhcp {
            op: 2,
            xid: d.xid,
            flags: d.flags,
            ciaddr: [0; 4],
            yiaddr: if kind == 6 { [0; 4] } else { self.lease.ip },
            siaddr: self.lease.gateway,
            chaddr: d.chaddr,
            options,
        };
        match kind {
            2 => self.answered.dhcp_offers += 1,
            5 => {
                self.answered.dhcp_acks += 1;
                self.lease_given = Some(now);
            }
            _ => {}
        }
        // A renew comes unicast from the leased address and gets a unicast
        // answer. The first exchange is broadcast.
        let unicast = d.ciaddr == self.lease.ip && kind == 5;
        let (dst_mac, dst_ip) = if unicast || ((d.flags & 0x8000) == 0 && kind != 6) {
            (eth.src, if unicast { d.ciaddr } else { self.lease.ip })
        } else {
            ([0xff; 6], [255, 255, 255, 255])
        };
        let packet = Ipv4 {
            src: self.lease.gateway,
            dst: dst_ip,
            proto: IP_UDP,
            payload: Udp {
                src_port: 67,
                dst_port: 68,
                payload: reply.build(),
            }
            .build(self.lease.gateway, dst_ip),
        };
        Ok(self.eth(dst_mac, ETH_IPV4, &packet.build()))
    }

    fn eth(&self, dst: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
        Ethernet {
            dst,
            src: self.gateway_mac,
            ethertype,
            payload: payload.to_vec(),
        }
        .build()
    }
}

impl Default for Stub {
    fn default() -> Self {
        Self::new(GATEWAY_MAC, Lease::default())
    }
}

/// Run the stub over the host end of the device: one datagram per frame
/// in, one per reply out. `on_drop` gets each dropped frame that opens a
/// new window of its target, with the count so far. A datagram socket
/// gives no end of file when the other end closes, so the caller sets a
/// read timeout on it and raises `stop` to end the loop: a timeout with
/// `stop` set returns `Ok`. A read or write error returns it: the thread
/// ends, and the VM keeps running without a network.
pub fn pump(
    mut sock: impl Read + Write,
    stub: &Mutex<Stub>,
    stop: &AtomicBool,
    mut on_drop: impl FnMut(&Dropped, u64),
) -> std::io::Result<()> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match sock.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        let now = Instant::now();
        let replies = stub
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(&buf[..n], now);
        for r in replies {
            match r {
                Reply::Send(frame) => sock.write_all(&frame)?,
                Reply::Dropped {
                    what,
                    first: true,
                    count,
                } => on_drop(&what, count),
                Reply::Dropped { .. } => {}
            }
        }
    }
}

/// The stub of a session: the pump thread over the host end of the
/// device, the boundary entries (one per minute per target, kind
/// `network`, rule [`RULE`]) and the counters for the status.
pub struct Network {
    pub stub: Arc<Mutex<Stub>>,
    stop: Arc<AtomicBool>,
    ended: Arc<Mutex<Option<String>>>,
}

impl Network {
    /// Start the pump over `sock`. Drops go to the boundary log at
    /// `boundary`. When the pump ends, `on_end` gets the reason once: the
    /// VM keeps running without a network.
    pub fn start(
        sock: std::os::unix::net::UnixDatagram,
        boundary: std::path::PathBuf,
        on_end: Box<dyn FnOnce(&str) + Send>,
    ) -> Self {
        let stub = Arc::new(Mutex::new(Stub::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(Mutex::new(None));
        let _ = sock.set_read_timeout(Some(Duration::from_secs(1)));
        let (stub2, stop2, ended2) = (stub.clone(), stop.clone(), ended.clone());
        let _ = std::thread::Builder::new()
            .name("stubnet".into())
            .spawn(move || {
                let mut sock = Datagram(sock);
                let result = pump(&mut sock, &stub2, &stop2, |d, count| {
                    let e =
                        crate::boundary::Entry::new(crate::boundary::NETWORK, &d.target(), RULE);
                    if let Err(err) = crate::boundary::record_at(&boundary, e) {
                        log::debug!("stubnet: cannot write the boundary log: {err}");
                    }
                    log::info!("stubnet: dropped {} (count {count})", d.target());
                });
                let why = match result {
                    Ok(()) if stop2.load(Ordering::SeqCst) => return,
                    Ok(()) => "the device closed".to_string(),
                    Err(e) => e.to_string(),
                };
                if let Ok(mut g) = ended2.lock() {
                    *g = Some(why.clone());
                }
                on_end(&why);
            });
        Self { stub, stop, ended }
    }

    /// Why the pump ended, when it did.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().ok().and_then(|g| g.clone())
    }

    /// End the pump thread.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// The counters for `paddock app status --json`.
    pub fn status(&self) -> serde_json::Value {
        let s = self.stub.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({
            "answered": s.answered,
            "dropped": s.drops.lines(),
            "dropped_total": s.drops.total(),
            "lease": s.lease_given().is_some(),
            "ended": self.ended(),
        })
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A datagram socket as a `Read + Write` of frames.
struct Datagram(std::os::unix::net::UnixDatagram);

impl Read for Datagram {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Write for Datagram {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "stubnet_tests.rs"]
mod tests;
