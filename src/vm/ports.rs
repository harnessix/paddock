//! The vsock ports of a paddock VM. The VM has no network
//! device, so all traffic between the guest and the host uses these ports.
//! The numbers are fixed: the guest image, the helper and paddock agree on
//! them.

/// The guest agent.
pub const CONTROL: u32 = 1024;
/// paddock's host request broker.
pub const BROKER: u32 = 1025;
/// paddock's network proxy.
pub const PROXY: u32 = 1026;
/// The pty or the stdio of the run.
pub const PTY: u32 = 1027;
/// The control line of the macOS guest agent (`paddock-guest`): hello,
/// health, window events, requests and replies.
pub const GUEST_CONTROL: u32 = 1030;
/// One CONNECT tunnel per TCP connection of the macOS guest.
pub const GUEST_PROXY: u32 = 1031;
/// One file transfer of the macOS guest: a header line, then bytes.
pub const GUEST_FILES: u32 = 1032;
/// One forwarded login callback of the macOS guest: the guest agent dials
/// it after the host asked (`login.forward`), with the token line and the
/// ticket line, then carries the bytes of one host loopback connection.
pub const GUEST_LOGIN: u32 = 1033;

/// Every port with its name, in port order.
pub const ALL: [(u32, &str); 8] = [
    (CONTROL, "control"),
    (BROKER, "broker"),
    (PROXY, "proxy"),
    (PTY, "pty"),
    (GUEST_CONTROL, "guest-control"),
    (GUEST_PROXY, "guest-proxy"),
    (GUEST_FILES, "guest-files"),
    (GUEST_LOGIN, "guest-login"),
];

/// The name of a known port.
pub fn name(port: u32) -> Option<&'static str> {
    ALL.iter().find(|(p, _)| *p == port).map(|(_, n)| *n)
}

/// A known port from its number (`1025`) or its name (`broker`).
pub fn parse(text: &str) -> Option<u32> {
    if let Some((p, _)) = ALL.iter().find(|(_, n)| *n == text) {
        return Some(*p);
    }
    let port: u32 = text.parse().ok()?;
    name(port).map(|_| port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_fixed() {
        assert_eq!((CONTROL, BROKER, PROXY, PTY), (1024, 1025, 1026, 1027));
        assert_eq!(
            (GUEST_CONTROL, GUEST_PROXY, GUEST_FILES, GUEST_LOGIN),
            (1030, 1031, 1032, 1033)
        );
    }

    #[test]
    fn guest_ports_have_names_and_all_is_in_port_order() {
        assert_eq!(parse("guest-control"), Some(GUEST_CONTROL));
        assert_eq!(parse("1032"), Some(GUEST_FILES));
        assert_eq!(name(GUEST_PROXY), Some("guest-proxy"));
        assert!(ALL.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn parse_takes_a_number_or_a_name() {
        assert_eq!(parse("1025"), Some(BROKER));
        assert_eq!(parse("proxy"), Some(PROXY));
        assert_eq!(parse("pty"), Some(PTY));
        assert_eq!(name(CONTROL), Some("control"));
    }

    #[test]
    fn parse_refuses_other_ports() {
        for text in [
            "22", "1023", "1028", "1029", "1034", "-1", "", "Broker", "0x401",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
