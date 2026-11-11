//! Seatbelt profiles (SBPL) for `sandbox-exec` on macOS.
//!
//! The generator is plain code with no macOS calls, so it is tested on
//! every platform. The macOS app backend (backend/seatbelt.rs) builds a
//! [`Policy`] from the sandbox spec and runs `sandbox-exec -f PROFILE`.
//!
//! The profile denies everything by default and then allows. Seatbelt uses
//! the last rule that matches, so the path rules come in order of depth:
//! a read-only folder inside a writable one, and a hidden file inside
//! either, come after the folder that contains them.
//!
//! Seatbelt sees real paths: `/var` is `/private/var` and `/tmp` is
//! `/private/tmp`. The caller passes resolved paths.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// What a path rule gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read and write, Unix sockets included.
    ReadWrite,
    /// Read only. Writes stay denied, also inside a writable folder.
    ReadOnly,
    /// Hidden: no read and no write, also inside an allowed folder. The name
    /// stays visible to a `stat`.
    Deny,
    /// Connect to the Unix socket (a file) or to the Unix sockets in a
    /// folder. No other right: the socket cannot be removed or replaced.
    Connect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRule {
    pub path: PathBuf,
    pub access: Access,
    /// A file (`literal`) instead of a folder and all below it (`subpath`).
    pub file: bool,
    /// Who asked for it, as a comment in the profile.
    pub why: String,
}

/// Network access of the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Net {
    /// No IP network. Unix sockets in the writable folders still work.
    None,
    /// Connections to localhost only. paddock's proxy listens there.
    Localhost,
    /// The network of the Mac: connect anywhere, listen on every address,
    /// resolve names (network mode `host`).
    Host,
    /// Connect to every IP address and resolve names, listen on localhost
    /// only: an approved command of the gateway executor. The caller adds
    /// the DNS service to [`Policy::services`].
    Any,
}

/// What runs in the sandbox. A desktop app needs the window server and
/// many macOS services. A terminal agent needs few, and gets only those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    App,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub paths: Vec<PathRule>,
    pub net: Net,
    pub scope: Scope,
    /// Terminal scope with [`Net::Localhost`]: the only localhost ports the
    /// sandbox can connect to (paddock's proxy and the allowed host ports).
    /// App scope connects to every localhost port.
    pub ports: Vec<u16>,
    /// Terminal scope with [`Net::Localhost`]: a range of localhost ports
    /// (first, last) that the sandbox can also connect to, for its own
    /// servers. The user configuration sets it (`[macos] localhost_ports`).
    pub localhost_range: Option<(u16, u16)>,
    /// Terminal scope: more macOS services (Mach names) besides
    /// [`TERMINAL_SERVICES`], each with its reason. The keychain of a login
    /// adds `com.apple.SecurityServer` (backend/seatbelt/keychain.rs).
    pub services: Vec<(String, String)>,
    /// Terminal scope: the run makes its own ptys with `/dev/ptmx`, as an
    /// App Sandbox app does ([`OWN_PTYS`]). Only for a harness that cannot
    /// load paddock's pty library (backend/seatbelt/run.rs `pty_refusal`).
    pub own_ptys: bool,
    /// App scope: `[apps.NAME] login_ports` is on. The app listens on
    /// localhost for the login page in the browser of the Mac. Only
    /// [`Net::None`] needs a rule for it: the other networks allow it.
    pub login_ports: bool,
}

impl Policy {
    pub fn new(net: Net) -> Self {
        Self {
            paths: Vec::new(),
            net,
            scope: Scope::App,
            ports: Vec::new(),
            localhost_range: None,
            services: Vec::new(),
            own_ptys: false,
            login_ports: false,
        }
    }

    /// The policy of a terminal run: the macOS services of [`TERMINAL_SERVICES`]
    /// only, and outbound connections to the localhost `ports` only.
    pub fn terminal(net: Net, ports: Vec<u16>) -> Self {
        Self {
            paths: Vec::new(),
            net,
            scope: Scope::Terminal,
            ports,
            localhost_range: None,
            services: Vec::new(),
            own_ptys: false,
            login_ports: false,
        }
    }

    pub fn add(&mut self, path: impl Into<PathBuf>, access: Access, why: &str) {
        self.paths.push(PathRule {
            path: path.into(),
            access,
            file: false,
            why: why.to_string(),
        });
    }

    pub fn add_file(&mut self, path: impl Into<PathBuf>, access: Access, why: &str) {
        self.paths.push(PathRule {
            path: path.into(),
            access,
            file: true,
            why: why.to_string(),
        });
    }
}

/// The system folders every app reads: the frameworks, libraries, fonts and
/// the system configuration. Never the user's home.
pub const SYSTEM_READ: &[&str] = &[
    "/System",
    "/Library",
    "/usr",
    "/bin",
    "/sbin",
    "/private/etc",
    "/private/var/db/timezone",
    "/dev",
];

/// More system folders for a terminal run: package managers (Homebrew,
/// MacPorts, Nix), the choice of `/bin/sh` and of the developer tools.
pub const TERMINAL_READ: &[&str] = &[
    "/opt",
    "/nix",
    "/private/var/select",
    "/private/var/db/xcode_select_link",
    "/Applications/Xcode.app",
];

/// The macOS services a terminal run can reach. Not the Keychain, the
/// pasteboard, the window server, preferences or Launch Services: each of
/// them reads or changes data of the real user outside the sandbox. A run
/// with a keychain of its own adds securityd ([`Policy::services`]).
pub const TERMINAL_SERVICES: &[(&str, &str)] = &[
    (
        "com.apple.system.opendirectoryd.libinfo",
        "user and group names (getpwuid)",
    ),
    (
        "com.apple.system.opendirectoryd.membership",
        "group membership",
    ),
    (
        "com.apple.system.notification_center",
        "system notifications such as a time zone change",
    ),
    ("com.apple.system.logger", "the system log"),
    ("com.apple.logd", "the system log"),
    ("com.apple.diagnosticd", "the system log"),
    ("com.apple.trustd", "TLS certificate checks"),
    ("com.apple.trustd.agent", "TLS certificate checks"),
    ("com.apple.ocspd", "TLS certificate revocation checks"),
    (
        "com.apple.SystemConfiguration.configd",
        "network configuration (read)",
    ),
    ("com.apple.FSEvents", "file change events for watchers"),
];

/// The macOS services of network mode `host` in a terminal run: name
/// resolution and the network state. The app scope reaches every service.
pub const HOST_NET_SERVICES: &[(&str, &str)] = &[
    ("com.apple.dnssd.service", "name resolution (mDNSResponder)"),
    (
        "com.apple.SystemConfiguration.DNSConfiguration",
        "the DNS configuration",
    ),
    (
        "com.apple.SystemConfiguration.SCNetworkReachability",
        "network reachability",
    ),
    ("com.apple.networkd", "Network.framework connections"),
];

/// The socket of mDNSResponder, which resolves names for every program.
pub const MDNS_SOCKET: &str = "/private/var/run/mDNSResponder";

/// Device files the app can write.
pub const DEV_WRITE: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/tty",
    "/dev/dtracehelper",
    "/dev/random",
    "/dev/urandom",
];

/// An SBPL string literal for a path. Backslash and double quote are
/// escaped. A path that is not absolute, or has a control character such as
/// a newline or NUL, is refused: it cannot be quoted safely.
pub fn quote(p: &Path) -> Result<String> {
    let Some(s) = p.to_str() else {
        bail!(
            "{} is not valid UTF-8, so a Seatbelt profile cannot name it",
            p.display()
        );
    };
    if !s.starts_with('/') {
        bail!("{s:?} is not an absolute path, so a Seatbelt profile cannot name it");
    }
    if s.chars().any(|c| c.is_control()) {
        bail!("{s:?} has a control character (such as a newline or NUL), so a Seatbelt profile cannot name it");
    }
    // sandbox-exec refuses string literals over 1025 bytes.
    if s.len() > 900 {
        bail!("{s:?} is too long for a Seatbelt profile");
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Ok(out)
}

/// A comment for the profile, on one line.
fn comment(s: &str) -> String {
    let one: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("; {one}")
}

/// The fixed rules of every profile: the process and the system calls. One
/// rule per line, each with its reason.
const BASE: &[(&str, &str)] = &[
    ("(version 1)", "Seatbelt profile format 1"),
    ("(deny default)", "everything not allowed below is denied"),
    (
        "(allow process-exec)",
        "start programs (each still needs read access to its file)",
    ),
    ("(allow process-fork)", "start child processes"),
    (
        "(allow process-info* (target same-sandbox))",
        "see its own processes, not others",
    ),
    (
        "(allow signal (target same-sandbox))",
        "signal its own processes, not others",
    ),
    (
        "(allow syscall*)",
        "system calls: the file and network rules do the confinement",
    ),
    (
        "(allow sysctl-read)",
        "read system information such as the CPU count",
    ),
    (
        "(allow ipc-posix-shm*)",
        "shared memory between the sandbox's processes",
    ),
    (
        "(allow ipc-posix-sem)",
        "semaphores between the sandbox's processes",
    ),
    (
        "(allow system-socket (socket-domain AF_UNIX))",
        "create Unix sockets (the paths are limited below)",
    ),
    (
        "(allow file-read-metadata)",
        "stat every path: names stay visible, contents do not",
    ),
    ("(allow file-read* (literal \"/\"))", "list the root folder"),
];

/// The system services of a desktop app.
const APP_BASE: &[(&str, &str)] = &[
    (
        "(allow mach-priv-task-port (target same-sandbox))",
        "Chromium helpers attach to their own processes",
    ),
    (
        "(allow mach-lookup)",
        "macOS services: window server, fonts, pasteboard, Keychain (a desktop app needs them)",
    ),
    (
        "(allow mach-register)",
        "Chromium registers its own service ports",
    ),
    (
        "(allow iokit-open)",
        "graphics and power drivers (IOSurface, GPU)",
    ),
    ("(allow iokit-get-properties)", "read driver properties"),
    (
        "(allow user-preference-read)",
        "read preferences (under the private home)",
    ),
    (
        "(allow user-preference-write)",
        "write preferences (under the private home)",
    ),
    (
        "(allow distributed-notification-post)",
        "post system notifications such as theme changes",
    ),
    (
        "(allow system-socket (require-all (socket-domain AF_SYSTEM) (socket-protocol 2)))",
        "system event sockets, no network",
    ),
];

/// The terminal rules besides the services: the terminal of the run, and
/// the ptys that paddock opens on the host and sends in (ptybroker.rs).
const TERMINAL_BASE: &[(&str, &str)] = &[
    (
        "(allow file-ioctl (regex #\"^/dev/ttys[0-9]+$\"))",
        "terminal settings (window size, raw mode) on a terminal that the sandbox already holds open; opening another terminal stays denied",
    ),
    (
        "(allow file-ioctl (literal \"/dev/ptmx\") (regex #\"^/dev/pty[p-wP-W][0-9a-zA-Z]$\"))",
        "the window size on the master side of a pty from paddock; opening /dev/ptmx stays denied",
    ),
];

/// The pty rules of Apple's App Sandbox profile (`application.sb`): a
/// process opens `/dev/ptmx` and gets a new pty. The kernel gives it the
/// sandbox extension `com.apple.sandbox.pty` for the new `/dev/ttysNNN`
/// only, so it opens its own ptys and no terminal that it did not create.
pub const OWN_PTYS: &[(&str, &str)] = &[
    (
        "(allow pseudo-tty)",
        "new ptys for a harness that cannot load paddock's pty library",
    ),
    (
        "(allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\"))",
        "open /dev/ptmx to make a new pty",
    ),
    (
        "(allow file-read* file-write* file-ioctl (require-all (regex #\"^/dev/ttys[0-9]+$\") (extension \"com.apple.sandbox.pty\")))",
        "open the terminal side of a pty that the sandbox made. The terminals of others have no extension and stay closed",
    ),
];

/// The last rule of every profile: no process in the sandbox pushes input
/// into a terminal (TIOCSTI), also not into the terminal of the run.
pub const LAST: (&str, &str) = (
    "(deny file-ioctl (ioctl-command TIOCSTI))",
    "no input pushed into a terminal (TIOCSTI)",
);

/// The most ports of a localhost range. Seatbelt has no port ranges, so the
/// profile gets one rule per port.
pub const MAX_RANGE: u32 = 1024;

/// The Seatbelt profile for a policy.
pub fn render(policy: &Policy) -> Result<String> {
    let mut out: Vec<String> = Vec::new();
    let scoped = match policy.scope {
        Scope::App => APP_BASE,
        Scope::Terminal => TERMINAL_BASE,
    };
    let own: &[(&str, &str)] = if policy.scope == Scope::Terminal && policy.own_ptys {
        OWN_PTYS
    } else {
        &[]
    };
    for (rule, why) in BASE.iter().chain(scoped).chain(own) {
        out.push(comment(why));
        out.push((*rule).to_string());
    }
    if policy.scope == Scope::Terminal {
        out.push(comment(
            "macOS services of a terminal run, no others (no Keychain, pasteboard or preferences)",
        ));
        for (name, why) in TERMINAL_SERVICES {
            out.push(comment(why));
            out.push(format!("(allow mach-lookup (global-name \"{name}\"))"));
        }
        for (name, why) in &policy.services {
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
                bail!("{name:?} is not a Mach service name");
            }
            out.push(comment(why));
            out.push(format!("(allow mach-lookup (global-name \"{name}\"))"));
        }
    }

    out.push(comment("system folders, read-only"));
    let extra: &[&str] = match policy.scope {
        Scope::App => &[],
        Scope::Terminal => TERMINAL_READ,
    };
    // A terminal run does not read all of /dev: another terminal of the
    // user (/dev/ttysNNN) would give its keystrokes away. It gets the
    // device files below and its own open files (/dev/fd).
    let all_dev = policy.scope == Scope::App;
    for p in SYSTEM_READ
        .iter()
        .chain(extra)
        .filter(|p| all_dev || **p != "/dev")
    {
        out.push(format!(
            "(allow file-read* (subpath {}))",
            quote(Path::new(p))?
        ));
    }
    if !all_dev {
        out.push(comment(
            "its own open files as /dev/fd/N (and /dev/stdin, /dev/stdout, /dev/stderr)",
        ));
        out.push("(allow file-read* file-write-data (subpath \"/dev/fd\"))".into());
    }
    out.push(comment("device files the app writes"));
    for p in DEV_WRITE {
        out.push(format!(
            "(allow file-read* file-write-data file-ioctl (literal {}))",
            quote(Path::new(p))?
        ));
    }

    match policy.net {
        Net::None => {
            out.push(comment("network: none (no IP rule allows a connection)"));
            if policy.scope == Scope::App && policy.login_ports {
                out.push(comment(
                    "login_ports: listen on localhost for the login page in the browser of this Mac",
                ));
                out.push("(allow network-bind (local ip \"localhost:*\"))".into());
                out.push("(allow network-inbound (local ip \"localhost:*\"))".into());
            }
        }
        Net::Localhost if policy.scope == Scope::Terminal => {
            out.push(comment(
                "network: connect only to these localhost ports: paddock's proxy and the allowed host ports",
            ));
            for port in &policy.ports {
                out.push(format!(
                    "(allow network-outbound (remote ip \"localhost:{port}\"))"
                ));
            }
            if let Some((first, last)) = policy.localhost_range {
                if first == 0 || first > last || u32::from(last - first) >= MAX_RANGE {
                    bail!("the localhost port range {first}-{last} is not a range of 1 to {MAX_RANGE} ports");
                }
                out.push(comment(&format!(
                    "the localhost ports {first}-{last} of the user configuration: the sandbox's own servers, and every service of the Mac on these ports"
                )));
                for port in first..=last {
                    if !policy.ports.contains(&port) {
                        out.push(format!(
                            "(allow network-outbound (remote ip \"localhost:{port}\"))"
                        ));
                    }
                }
            }
            out.push(comment(
                "listen on localhost only, for login callbacks from the host browser",
            ));
            out.push("(allow network-bind (local ip \"localhost:*\"))".into());
            out.push("(allow network-inbound (local ip \"localhost:*\"))".into());
        }
        Net::Host => {
            out.push(comment(
                "network: host. The network of this Mac: connect anywhere, listen on every address",
            ));
            out.push("(allow network-outbound (remote ip \"*:*\"))".into());
            out.push("(allow network-bind (local ip \"*:*\"))".into());
            out.push("(allow network-inbound (local ip \"*:*\"))".into());
            out.push(comment("name resolution through mDNSResponder"));
            out.push(format!(
                "(allow network-outbound (remote unix-socket (path-literal {})))",
                quote(Path::new(MDNS_SOCKET))?
            ));
            if policy.scope == Scope::Terminal {
                for (name, why) in HOST_NET_SERVICES {
                    out.push(comment(why));
                    out.push(format!("(allow mach-lookup (global-name \"{name}\"))"));
                }
            }
        }
        Net::Any => {
            out.push(comment(
                "network: connect to every address, listen on localhost only",
            ));
            out.push("(allow network-outbound (remote ip \"*:*\"))".into());
            out.push("(allow network-bind (local ip \"localhost:*\"))".into());
            out.push("(allow network-inbound (local ip \"localhost:*\"))".into());
            out.push(comment("name resolution through mDNSResponder"));
            out.push(format!(
                "(allow network-outbound (remote unix-socket (path-literal {})))",
                quote(Path::new(MDNS_SOCKET))?
            ));
        }
        Net::Localhost => {
            out.push(comment(
                "network: connect to localhost only, where paddock's proxy listens",
            ));
            out.push("(allow network-outbound (remote ip \"localhost:*\"))".into());
            out.push(comment(
                "listen on local ports for the app's own processes (no remote endpoint, so no egress)",
            ));
            out.push("(allow network-bind (local ip \"*:*\"))".into());
            out.push("(allow network-inbound (local ip \"*:*\"))".into());
        }
    }

    // Deeper paths last, so the most specific rule wins.
    let mut rules: Vec<&PathRule> = policy.paths.iter().collect();
    rules.sort_by_key(|r| r.path.components().count());
    for r in rules {
        let q = quote(&r.path)?;
        let filter = if r.file {
            format!("(literal {q})")
        } else {
            format!("(subpath {q})")
        };
        out.push(comment(&r.why));
        match r.access {
            Access::ReadWrite => {
                out.push(format!("(allow file-read* file-write* {filter})"));
                if !r.file {
                    out.push(format!(
                        "(allow network-bind (local unix-socket (subpath {q})))"
                    ));
                    out.push(format!(
                        "(allow network-outbound (remote unix-socket (subpath {q})))"
                    ));
                }
            }
            Access::ReadOnly => {
                out.push(format!("(allow file-read* {filter})"));
                out.push(format!("(deny file-write* {filter})"));
            }
            Access::Deny => {
                out.push(format!("(deny file-read* file-write* {filter})"));
            }
            Access::Connect => {
                let target = if r.file {
                    format!("(path-literal {q})")
                } else {
                    format!("(subpath {q})")
                };
                out.push(format!(
                    "(allow network-outbound (remote unix-socket {target}))"
                ));
            }
        }
    }
    out.push(comment(LAST.1));
    out.push(LAST.0.to_string());
    out.push(String::new());
    Ok(out.join("\n"))
}

#[cfg(test)]
#[path = "seatbelt_tests.rs"]
mod tests;
