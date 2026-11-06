//! The Seatbelt view of an approved command on macOS.
//!
//! The executor runs the command with `sandbox-exec` and a profile from the
//! same [`ExecCfg`] that systemd and bwrap get on Linux. The command sees the
//! system read-only, the project read-write, an empty home, its own
//! `TMPDIR` and the network. Seatbelt denies the rest of the user's files
//! by default. The policy is plain code, so its tests run on every platform.

use super::{ExecCfg, Home};
use crate::backend::seatbelt::keychain;
use crate::seatbelt::{Access, Net, Policy};
use std::path::{Path, PathBuf};

/// The service that resolves names, for a command with network.
pub const DNS_SERVICE: &str = "com.apple.dnssd.service";

/// The folders of one command. Every path is a real path (`/private/var`,
/// not `/var`), as Seatbelt sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The real home of the user.
    pub home: PathBuf,
    pub project: PathBuf,
    /// The empty folder that is `HOME` when the home is hidden.
    pub empty_home: PathBuf,
    /// The command's own `TMPDIR`.
    pub tmp: PathBuf,
    /// The user's cache folder (`DARWIN_USER_CACHE_DIR`), for the module
    /// directory of the Security framework.
    pub user_cache: Option<PathBuf>,
    /// Agent sockets that the command gets from its variables
    /// ([`AGENT_SOCKETS`]): it can connect to them, nothing more.
    pub sockets: Vec<PathBuf>,
}

/// Variables that name an agent socket. When one passes (`git-push`, or
/// `[exec] env`), the command can connect to that socket.
pub const AGENT_SOCKETS: &[&str] = &["SSH_AUTH_SOCK"];

impl View {
    /// `HOME` of the command.
    pub fn home_var(&self, cfg: &ExecCfg) -> &Path {
        match cfg.home {
            Home::Hidden => &self.empty_home,
            Home::ReadOnly | Home::Full => &self.home,
        }
    }
}

/// The Seatbelt policy of a command with `cfg`.
pub fn policy(cfg: &ExecCfg, v: &View) -> Policy {
    let net = if cfg.network { Net::Any } else { Net::None };
    let mut p = Policy::terminal(net, Vec::new());
    if cfg.network {
        p.services.push((
            DNS_SERVICE.into(),
            "DNS names, for a command with network".into(),
        ));
    }
    match cfg.home {
        // The real home has no rule: Seatbelt denies it by default.
        Home::Hidden => p.add(
            &v.empty_home,
            Access::ReadWrite,
            "the empty home of this command",
        ),
        Home::ReadOnly => p.add(
            &v.home,
            Access::ReadOnly,
            "the home of the user, read-only ([exec] home)",
        ),
        Home::Full => p.add(
            &v.home,
            Access::ReadWrite,
            "the home of the user, read-write ([exec] home = \"full\")",
        ),
    }
    for d in &cfg.hide {
        p.add(d, Access::Deny, "hidden from the command");
    }
    p.add(&v.project, Access::ReadWrite, "the project");
    for d in &cfg.paths {
        p.add(d, Access::ReadWrite, "[exec] paths");
    }
    for d in &cfg.ro_paths {
        p.add(d, Access::ReadOnly, "[exec] ro_paths");
    }
    p.add(&v.tmp, Access::ReadWrite, "the command's own TMPDIR");
    for s in &v.sockets {
        p.add_file(
            s,
            Access::Connect,
            "an agent socket that the command got from its variables",
        );
    }
    if cfg.keychain {
        keychain::securityd(
            &mut p,
            v.user_cache.as_deref(),
            "securityd: the login keychain of the user, for gh and git credential helpers. The keychain file is in the read-only home",
        );
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            home: "/Users/demo".into(),
            project: "/Users/demo/src/app".into(),
            empty_home: "/Users/demo/.local/state/paddock/run/exec/u/home".into(),
            tmp: "/Users/demo/.local/state/paddock/run/exec/u/tmp".into(),
            user_cache: Some("/private/var/folders/xy/abc/C".into()),
            sockets: Vec::new(),
        }
    }

    fn render(cfg: &ExecCfg) -> String {
        crate::seatbelt::render(&policy(cfg, &view())).unwrap()
    }

    fn cfg() -> ExecCfg {
        ExecCfg {
            hide: Vec::new(),
            ..ExecCfg::default()
        }
    }

    #[test]
    fn a_hidden_home_is_an_empty_folder_and_the_network_is_on() {
        let c = cfg();
        let s = render(&c);
        assert!(s.contains("(allow file-read* file-write* (subpath \"/Users/demo/src/app\"))"));
        assert!(s.contains(
            "(allow file-read* file-write* (subpath \"/Users/demo/.local/state/paddock/run/exec/u/home\"))"
        ));
        assert!(s.contains(
            "(allow file-read* file-write* (subpath \"/Users/demo/.local/state/paddock/run/exec/u/tmp\"))"
        ));
        assert!(!s.contains("(subpath \"/Users/demo\")"), "{s}");
        assert!(s.contains("(allow network-outbound (remote ip \"*:*\"))"));
        assert!(s.contains("(allow network-bind (local ip \"localhost:*\"))"));
        assert!(s.contains("(global-name \"com.apple.dnssd.service\")"));
        assert!(!s.contains("SecurityServer"), "{s}");
        assert_eq!(
            view().home_var(&c),
            Path::new("/Users/demo/.local/state/paddock/run/exec/u/home")
        );
    }

    #[test]
    fn without_network_there_is_no_ip_rule() {
        let s = render(&ExecCfg {
            network: false,
            ..cfg()
        });
        assert!(!s.contains("remote ip"), "{s}");
        assert!(!s.contains("local ip"), "{s}");
        assert!(!s.contains("dnssd"), "{s}");
    }

    #[test]
    fn a_read_only_home_with_the_keychain() {
        let c = ExecCfg {
            home: Home::ReadOnly,
            keychain: true,
            ..cfg()
        };
        let s = render(&c);
        assert!(s.contains("(allow file-read* (subpath \"/Users/demo\"))"));
        assert!(s.contains("(deny file-write* (subpath \"/Users/demo\"))"));
        assert!(s.contains("(global-name \"com.apple.SecurityServer\")"));
        assert!(s.contains("(allow file-read* (subpath \"/private/var/db/mds\"))"));
        assert!(s.contains("(subpath \"/private/var/folders/xy/abc/C/mds\")"));
        assert!(s.contains("(deny file-read* file-write* (subpath \"/Library/Keychains\"))"));
        assert_eq!(view().home_var(&c), Path::new("/Users/demo"));
        // The project stays writable inside the read-only home: its rule
        // comes later.
        let ro = s
            .find("(deny file-write* (subpath \"/Users/demo\"))")
            .unwrap();
        let rw = s
            .find("(allow file-read* file-write* (subpath \"/Users/demo/src/app\"))")
            .unwrap();
        assert!(ro < rw, "{s}");
        // A full home is writable.
        let s = render(&ExecCfg {
            home: Home::Full,
            ..cfg()
        });
        assert!(s.contains("(allow file-read* file-write* (subpath \"/Users/demo\"))"));
    }

    #[test]
    fn hidden_folders_are_denied_inside_the_home() {
        let s = render(&ExecCfg {
            home: Home::ReadOnly,
            hide: vec!["/Users/demo/.ssh".into()],
            paths: vec!["/Users/demo/lib".into()],
            ro_paths: vec!["/srv/data".into()],
            ..cfg()
        });
        let home = s
            .find("(allow file-read* (subpath \"/Users/demo\"))")
            .unwrap();
        let deny = s
            .find("(deny file-read* file-write* (subpath \"/Users/demo/.ssh\"))")
            .unwrap();
        assert!(home < deny, "{s}");
        assert!(s.contains("(allow file-read* file-write* (subpath \"/Users/demo/lib\"))"));
        assert!(s.contains("(allow file-read* (subpath \"/srv/data\"))"));
    }

    #[test]
    fn an_agent_socket_is_reachable_and_nothing_else_of_its_folder() {
        let v = View {
            sockets: vec!["/private/tmp/com.apple.launchd.x/Listeners".into()],
            ..view()
        };
        let s = crate::seatbelt::render(&policy(&cfg(), &v)).unwrap();
        assert!(s.contains(
            "(allow network-outbound (remote unix-socket (path-literal \"/private/tmp/com.apple.launchd.x/Listeners\")))"
        ));
        assert!(!s.contains("(subpath \"/private/tmp"), "{s}");
    }
}

/// Tests that run `sandbox-exec`. They run only on macOS.
#[cfg(all(test, target_os = "macos"))]
#[path = "seatbelt_tests.rs"]
mod macos_tests;
