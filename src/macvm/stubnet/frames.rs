//! The frames of the stub network: parsers and builders for Ethernet II,
//! ARP, IPv4, UDP, DHCP, DNS and ICMP echo. Each parser returns `None`
//! for a short or wrong input, and each builder round trips through its
//! parser.

/// The IP protocol numbers and Ethernet types the stub knows.
pub const ETH_IPV4: u16 = 0x0800;
pub const ETH_ARP: u16 = 0x0806;
pub const ETH_IPV6: u16 = 0x86dd;
pub const IP_ICMP: u8 = 1;
pub const IP_TCP: u8 = 6;
pub const IP_UDP: u8 = 17;
const DHCP_COOKIE: [u8; 4] = [99, 130, 83, 99];

pub fn ip_text(ip: &[u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// The ones' complement sum of the IP family.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = u16::from_be_bytes([chunk[0], *chunk.get(1).unwrap_or(&0)]);
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn take<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at + N)?.try_into().ok()
}

/// An Ethernet II frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ethernet {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
    pub payload: Vec<u8>,
}

impl Ethernet {
    pub fn parse(b: &[u8]) -> Option<Self> {
        Some(Self {
            dst: take(b, 0)?,
            src: take(b, 6)?,
            ethertype: u16::from_be_bytes(take(b, 12)?),
            payload: b.get(14..)?.to_vec(),
        })
    }

    pub fn build(&self) -> Vec<u8> {
        let mut f = Vec::with_capacity(14 + self.payload.len());
        f.extend_from_slice(&self.dst);
        f.extend_from_slice(&self.src);
        f.extend_from_slice(&self.ethertype.to_be_bytes());
        f.extend_from_slice(&self.payload);
        f
    }
}

/// An ARP message for IPv4 over Ethernet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arp {
    /// 1 request, 2 reply.
    pub op: u16,
    pub sha: [u8; 6],
    pub spa: [u8; 4],
    pub tha: [u8; 6],
    pub tpa: [u8; 4],
}

impl Arp {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if take::<6>(b, 0)? != [0, 1, 8, 0, 6, 4] {
            return None;
        }
        Some(Self {
            op: u16::from_be_bytes(take(b, 6)?),
            sha: take(b, 8)?,
            spa: take(b, 14)?,
            tha: take(b, 18)?,
            tpa: take(b, 24)?,
        })
    }

    pub fn build(&self) -> Vec<u8> {
        let mut r = Vec::with_capacity(28);
        r.extend_from_slice(&[0, 1, 8, 0, 6, 4]);
        r.extend_from_slice(&self.op.to_be_bytes());
        r.extend_from_slice(&self.sha);
        r.extend_from_slice(&self.spa);
        r.extend_from_slice(&self.tha);
        r.extend_from_slice(&self.tpa);
        r
    }
}

/// An IPv4 packet without options. The builder sets TTL 64, DF, id 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv4 {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub proto: u8,
    pub payload: Vec<u8>,
}

impl Ipv4 {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let first = *b.first()?;
        if first >> 4 != 4 {
            return None;
        }
        let ihl = usize::from(first & 0x0f) * 4;
        if ihl < 20 || b.len() < ihl {
            return None;
        }
        let total = usize::from(u16::from_be_bytes(take(b, 2)?));
        if total < ihl || total > b.len() {
            return None;
        }
        Some(Self {
            src: take(b, 12)?,
            dst: take(b, 16)?,
            proto: b[9],
            payload: b[ihl..total].to_vec(),
        })
    }

    pub fn build(&self) -> Vec<u8> {
        let total = (20 + self.payload.len()) as u16;
        let mut h = vec![0x45, 0];
        h.extend_from_slice(&total.to_be_bytes());
        h.extend_from_slice(&[0, 0, 0x40, 0, 64, self.proto, 0, 0]);
        h.extend_from_slice(&self.src);
        h.extend_from_slice(&self.dst);
        let c = checksum(&h).to_be_bytes();
        h[10] = c[0];
        h[11] = c[1];
        h.extend_from_slice(&self.payload);
        h
    }
}

/// A UDP datagram. The builder needs both addresses for the checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Udp {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: Vec<u8>,
}

impl Udp {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let len = usize::from(u16::from_be_bytes(take(b, 4)?));
        if len < 8 || len > b.len() {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes(take(b, 0)?),
            dst_port: u16::from_be_bytes(take(b, 2)?),
            payload: b[8..len].to_vec(),
        })
    }

    pub fn build(&self, src_ip: [u8; 4], dst_ip: [u8; 4]) -> Vec<u8> {
        let len = (8 + self.payload.len()) as u16;
        let mut u = Vec::with_capacity(usize::from(len));
        u.extend_from_slice(&self.src_port.to_be_bytes());
        u.extend_from_slice(&self.dst_port.to_be_bytes());
        u.extend_from_slice(&len.to_be_bytes());
        u.extend_from_slice(&[0, 0]);
        u.extend_from_slice(&self.payload);
        let mut pseudo = Vec::with_capacity(12 + u.len());
        pseudo.extend_from_slice(&src_ip);
        pseudo.extend_from_slice(&dst_ip);
        pseudo.extend_from_slice(&[0, IP_UDP]);
        pseudo.extend_from_slice(&len.to_be_bytes());
        pseudo.extend_from_slice(&u);
        let mut c = checksum(&pseudo);
        if c == 0 {
            c = 0xffff;
        }
        u[6..8].copy_from_slice(&c.to_be_bytes());
        u
    }
}

/// A DHCP message: the BOOTP fields the stub uses and the options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dhcp {
    /// 1 request, 2 reply.
    pub op: u8,
    pub xid: [u8; 4],
    pub flags: u16,
    pub ciaddr: [u8; 4],
    pub yiaddr: [u8; 4],
    pub siaddr: [u8; 4],
    pub chaddr: [u8; 6],
    pub options: Vec<(u8, Vec<u8>)>,
}

impl Dhcp {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 240 || take::<4>(b, 236)? != DHCP_COOKIE || b[1] != 1 || b[2] != 6 {
            return None;
        }
        let mut options = Vec::new();
        let mut i = 240;
        while i < b.len() {
            let code = b[i];
            if code == 255 {
                break;
            }
            if code == 0 {
                i += 1;
                continue;
            }
            let len = usize::from(*b.get(i + 1)?);
            let value = b.get(i + 2..i + 2 + len)?;
            options.push((code, value.to_vec()));
            i += 2 + len;
        }
        Some(Self {
            op: b[0],
            xid: take(b, 4)?,
            flags: u16::from_be_bytes(take(b, 10)?),
            ciaddr: take(b, 12)?,
            yiaddr: take(b, 16)?,
            siaddr: take(b, 20)?,
            chaddr: take(b, 28)?,
            options,
        })
    }

    pub fn build(&self) -> Vec<u8> {
        let mut r = vec![0u8; 240];
        r[0] = self.op;
        r[1] = 1;
        r[2] = 6;
        r[4..8].copy_from_slice(&self.xid);
        r[10..12].copy_from_slice(&self.flags.to_be_bytes());
        r[12..16].copy_from_slice(&self.ciaddr);
        r[16..20].copy_from_slice(&self.yiaddr);
        r[20..24].copy_from_slice(&self.siaddr);
        r[28..34].copy_from_slice(&self.chaddr);
        r[236..240].copy_from_slice(&DHCP_COOKIE);
        for (code, value) in &self.options {
            r.push(*code);
            r.push(value.len().min(255) as u8);
            r.extend_from_slice(&value[..value.len().min(255)]);
        }
        r.push(255);
        r.resize(r.len().max(300), 0);
        r
    }

    fn option(&self, code: u8) -> Option<&[u8]> {
        self.options
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| v.as_slice())
    }

    /// Option 53: 1 DISCOVER, 2 OFFER, 3 REQUEST, 5 ACK, 6 NAK.
    pub fn msg_type(&self) -> Option<u8> {
        self.option(53)?.first().copied()
    }

    /// Option 50, the address a REQUEST asks for.
    pub fn requested_ip(&self) -> Option<[u8; 4]> {
        self.option(50)?.try_into().ok()
    }

    /// Option 51, the lease length of an OFFER or ACK.
    pub fn lease_secs(&self) -> Option<u32> {
        Some(u32::from_be_bytes(self.option(51)?.try_into().ok()?))
    }
}

/// A DNS message: the id, the flags, the counts and the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dns {
    pub id: u16,
    pub flags: u16,
    pub qdcount: u16,
    pub ancount: u16,
    pub nscount: u16,
    pub arcount: u16,
    /// The question and record sections, as they came.
    pub rest: Vec<u8>,
}

impl Dns {
    pub fn parse(b: &[u8]) -> Option<Self> {
        Some(Self {
            id: u16::from_be_bytes(take(b, 0)?),
            flags: u16::from_be_bytes(take(b, 2)?),
            qdcount: u16::from_be_bytes(take(b, 4)?),
            ancount: u16::from_be_bytes(take(b, 6)?),
            nscount: u16::from_be_bytes(take(b, 8)?),
            arcount: u16::from_be_bytes(take(b, 10)?),
            rest: b.get(12..)?.to_vec(),
        })
    }

    pub fn build(&self) -> Vec<u8> {
        let mut r = Vec::with_capacity(12 + self.rest.len());
        for v in [
            self.id,
            self.flags,
            self.qdcount,
            self.ancount,
            self.nscount,
            self.arcount,
        ] {
            r.extend_from_slice(&v.to_be_bytes());
        }
        r.extend_from_slice(&self.rest);
        r
    }

    /// The `REFUSED` answer to this question: the same id and question,
    /// the response bit, the RD bit as asked, rcode 5, no records.
    pub fn refused(&self) -> Dns {
        Dns {
            id: self.id,
            flags: 0x8000 | (self.flags & 0x0100) | 5,
            qdcount: self.qdcount,
            ancount: 0,
            nscount: 0,
            arcount: 0,
            rest: self.rest.clone(),
        }
    }

    /// The rcode of an answer.
    pub fn rcode(&self) -> u8 {
        (self.flags & 0x0f) as u8
    }
}

/// An ICMP echo request (type 8) or reply (type 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcmpEcho {
    pub kind: u8,
    pub id: u16,
    pub seq: u16,
    pub data: Vec<u8>,
}

impl IcmpEcho {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 8 || b[1] != 0 {
            return None;
        }
        Some(Self {
            kind: b[0],
            id: u16::from_be_bytes(take(b, 4)?),
            seq: u16::from_be_bytes(take(b, 6)?),
            data: b[8..].to_vec(),
        })
    }

    pub fn build(&self) -> Vec<u8> {
        let mut r = vec![self.kind, 0, 0, 0];
        r.extend_from_slice(&self.id.to_be_bytes());
        r.extend_from_slice(&self.seq.to_be_bytes());
        r.extend_from_slice(&self.data);
        let c = checksum(&r).to_be_bytes();
        r[2] = c[0];
        r[3] = c[1];
        r
    }
}
