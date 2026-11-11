//! The texts of the network exit of the guest: the stub device, its drop
//! lines and the proxy identity in the boundary log.

/// The stub network device ended: the VM runs on without a network.
pub fn no_network() -> String {
    "no network: the guest device ended".into()
}

/// A target the stub network dropped, as the boundary log shows it:
/// `tcp 1.1.1.1:443`, `icmp 10.0.2.9`, `ipv6`.
pub fn drop_target(proto: &str, dst: Option<&str>, port: Option<u16>) -> String {
    match (dst, port) {
        (Some(ip), Some(port)) => format!("{proto} {ip}:{port}"),
        (Some(ip), None) => format!("{proto} {ip}"),
        _ => proto.to_string(),
    }
}

/// One target of the drop summary: `tcp 1.1.1.1:443 x3`.
pub fn drop_count(target: &str, count: u64) -> String {
    format!("{target} x{count}")
}

/// The drop summary of the stub network for the log.
pub fn drop_summary(total: u64, parts: &[String]) -> String {
    format!("dropped {total}: {}", parts.join(", "))
}

/// A boundary rule with the guest process that hit it.
pub fn rule_with_identity(rule: &str, id: &str) -> String {
    format!("{rule} ({id})")
}

/// The log line of the same, with the reason.
pub fn network_ended(why: &str) -> String {
    format!("the network device of the guest ended ({why}): the session runs on without a network, the proxy of the app is not affected")
}

/// The header line of the guest proxy is longer than `max` bytes.
pub fn header_too_long(max: usize) -> String {
    format!("the header line is longer than {max} bytes")
}

/// The first line started with `{` but is no header line.
pub fn not_a_header(error: &str) -> String {
    format!("not a header line: {error}")
}

/// The header line has another version than this build.
pub fn header_version_mismatch(got: u32, want: u32) -> String {
    format!("header version {got} is not {want}")
}

/// The CONNECT request the guest proxy writes to the host proxy.
pub fn connect_line(target: &str) -> String {
    format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n")
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
