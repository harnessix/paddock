use super::*;

fn sample() -> Policy {
    let mut p = Policy::new(Net::Localhost);
    p.add("/Users/u/src/project", Access::ReadWrite, "project");
    p.add(
        "/Users/u/.local/share/paddock/homes/vscode-1",
        Access::ReadWrite,
        "private home",
    );
    p.add(
        "/Applications/Visual Studio Code.app",
        Access::ReadOnly,
        "app bundle",
    );
    p.add_file("/Users/u/.gitconfig", Access::ReadOnly, "import");
    p.add(
        "/Users/u/src/project/.git/hooks",
        Access::ReadOnly,
        "git hooks",
    );
    p.add_file("/Users/u/src/project/.env", Access::Deny, "secrets");
    p
}

#[test]
fn denies_by_default_and_allows_the_project_and_home() {
    let s = render(&sample()).unwrap();
    let lines: Vec<&str> = s.lines().filter(|l| !l.starts_with(';')).collect();
    assert_eq!(lines[0], "(version 1)");
    assert_eq!(lines[1], "(deny default)");
    assert!(s.contains("(allow file-read* file-write* (subpath \"/Users/u/src/project\"))"));
    assert!(s.contains(
        "(allow file-read* file-write* (subpath \"/Users/u/.local/share/paddock/homes/vscode-1\"))"
    ));
    assert!(s.contains("(allow file-read* (subpath \"/Applications/Visual Studio Code.app\"))"));
    assert!(s.contains("(deny file-write* (subpath \"/Applications/Visual Studio Code.app\"))"));
    assert!(s.contains("(allow file-read* (literal \"/Users/u/.gitconfig\"))"));
}

#[test]
fn the_real_home_is_not_allowed() {
    let s = render(&sample()).unwrap();
    for l in s.lines().filter(|l| l.starts_with("(allow")) {
        assert!(
            !l.contains("(subpath \"/Users/u\")") && !l.contains("(subpath \"/Users\")"),
            "{l}"
        );
        // The only global file rules are metadata and the root folder.
        if l.starts_with("(allow file-read*") || l.starts_with("(allow file-write") {
            assert!(l.contains("(subpath ") || l.contains("(literal "), "{l}");
        }
    }
    assert!(!s.contains("(allow file-read*)"));
    assert!(!s.contains("(allow file-write*)"));
}

#[test]
fn deeper_rules_come_after_the_folders_that_hold_them() {
    let s = render(&sample()).unwrap();
    let project = s.find("(subpath \"/Users/u/src/project\")").unwrap();
    let hooks = s
        .find("(deny file-write* (subpath \"/Users/u/src/project/.git/hooks\"))")
        .unwrap();
    let env = s
        .find("(deny file-read* file-write* (literal \"/Users/u/src/project/.env\"))")
        .unwrap();
    assert!(project < hooks && project < env);
}

#[test]
fn network_is_localhost_only() {
    let s = render(&sample()).unwrap();
    let net: Vec<&str> = s
        .lines()
        .filter(|l| l.starts_with("(allow network"))
        .collect();
    assert!(net.contains(&"(allow network-outbound (remote ip \"localhost:*\"))"));
    for l in &net {
        assert!(
            l.contains("(remote ip \"localhost:*\")")
                || l.contains("(local ip \"*:*\")")
                || l.contains("unix-socket (subpath "),
            "{l}"
        );
        assert!(!l.contains("(remote ip \"*:"), "{l}");
    }
    assert!(!s.contains("(allow network*)"));
    let none = render(&Policy::new(Net::None)).unwrap();
    assert!(!none.contains("(allow network-outbound (remote ip"));
    assert!(!none.contains("(local ip"));
}

#[test]
fn network_host_allows_every_address_and_name_resolution() {
    let all = [
        "(allow network-outbound (remote ip \"*:*\"))",
        "(allow network-bind (local ip \"*:*\"))",
        "(allow network-inbound (local ip \"*:*\"))",
        "(allow network-outbound (remote unix-socket (path-literal \"/private/var/run/mDNSResponder\")))",
    ];
    let app = render(&Policy::new(Net::Host)).unwrap();
    let term = render(&Policy::terminal(Net::Host, vec![41000])).unwrap();
    for s in [&app, &term] {
        for r in all {
            assert!(s.contains(r), "{r}");
        }
        assert!(!s.contains("(allow network*)"));
    }
    for (name, _) in HOST_NET_SERVICES {
        let r = format!("(allow mach-lookup (global-name \"{name}\"))");
        assert!(term.contains(&r), "{r}");
    }
    // No other mode gets them.
    for net in [Net::None, Net::Localhost] {
        let s = render(&Policy::terminal(net, vec![41000])).unwrap();
        assert!(!s.contains("(remote ip \"*:*\")"));
        assert!(!s.contains("mDNSResponder"));
        assert!(!s.contains("com.apple.dnssd.service"));
        assert!(!render(&Policy::new(net)).unwrap().contains("mDNSResponder"));
    }
}

#[test]
fn login_ports_listen_on_localhost_without_network() {
    let bind = "(allow network-bind (local ip \"localhost:*\"))";
    let inbound = "(allow network-inbound (local ip \"localhost:*\"))";
    let mut p = Policy::new(Net::None);
    assert!(!render(&p).unwrap().contains(bind));
    p.login_ports = true;
    let s = render(&p).unwrap();
    assert!(s.contains(bind) && s.contains(inbound));
    assert!(!s.contains("(allow network-outbound (remote ip"));
    // The other networks of the app scope listen on every local
    // address already, localhost too.
    for net in [Net::Localhost, Net::Host] {
        let s = render(&Policy::new(net)).unwrap();
        assert!(s.contains("(allow network-bind (local ip \"*:*\"))"));
        assert!(s.contains("(allow network-inbound (local ip \"*:*\"))"));
    }
}

#[test]
fn quotes_spaces_quotes_and_backslashes() {
    assert_eq!(
        quote(Path::new("/a b/c\"d\\e")).unwrap(),
        "\"/a b/c\\\"d\\\\e\""
    );
    let mut p = Policy::new(Net::None);
    p.add("/Users/u/my \"odd\" project", Access::ReadWrite, "project");
    let s = render(&p).unwrap();
    assert!(s.contains("(subpath \"/Users/u/my \\\"odd\\\" project\")"));
}

#[test]
fn refuses_paths_it_cannot_quote() {
    assert!(quote(Path::new("/a\nb")).is_err());
    assert!(quote(Path::new("/a\0b")).is_err());
    assert!(quote(Path::new("relative")).is_err());
    let mut p = Policy::new(Net::None);
    p.add(
        "/Users/u/evil\n(allow default)",
        Access::ReadWrite,
        "project",
    );
    assert!(render(&p).is_err());
}

#[test]
fn comments_stay_on_one_line() {
    let mut p = Policy::new(Net::None);
    p.add("/x", Access::ReadOnly, "why\n(allow default)");
    let s = render(&p).unwrap();
    assert!(!s.lines().any(|l| l == "(allow default)"));
}

#[test]
fn a_terminal_run_reaches_only_its_ports_and_services() {
    let mut p = Policy::terminal(Net::Localhost, vec![41000, 8765]);
    p.add_file(
        "/private/tmp/paddock-501/ab/id/host.sock",
        Access::Connect,
        "broker",
    );
    let s = render(&p).unwrap();
    assert!(s.contains("(allow network-outbound (remote ip \"localhost:41000\"))"));
    assert!(s.contains("(allow network-outbound (remote ip \"localhost:8765\"))"));
    assert!(!s.contains("(remote ip \"localhost:*\")"));
    assert!(s.contains("(allow network-bind (local ip \"localhost:*\"))"));
    assert!(!s.contains("(local ip \"*:*\")"));
    // Named services only: no Keychain, pasteboard or preferences.
    assert!(!s.lines().any(|l| l == "(allow mach-lookup)"));
    assert!(s.contains("(allow mach-lookup (global-name \"com.apple.trustd\"))"));
    for l in s.lines().filter(|l| l.starts_with("(allow mach-lookup")) {
        assert!(
            !l.contains("SecurityServer") && !l.contains("pasteboard"),
            "{l}"
        );
    }
    for app_only in [
        "(allow user-preference-read)",
        "(allow user-preference-write)",
        "(allow iokit-open)",
        "(allow mach-register)",
    ] {
        assert!(!s.contains(app_only), "{app_only}");
    }
    assert!(s.contains(
        "(allow network-outbound (remote unix-socket (path-literal \"/private/tmp/paddock-501/ab/id/host.sock\")))"
    ));
    // A socket gets no file write rule: it cannot be replaced.
    assert!(!s.contains("file-write* (literal \"/private/tmp/paddock-501/ab/id/host.sock\")"));
    assert!(s.contains("(allow file-read* (subpath \"/private/var/select\"))"));
    assert!(s.contains("(allow file-ioctl (regex #\"^/dev/ttys[0-9]+$\"))"));
    // Not all of /dev: another terminal stays closed.
    assert!(!s.contains("(subpath \"/dev\")"));
    assert!(s.contains("(allow file-read* file-write-data file-ioctl (literal \"/dev/tty\"))"));
    assert!(s.contains("(subpath \"/dev/fd\")"));
}

#[test]
fn a_terminal_run_without_network_has_no_ip_rule() {
    let s = render(&Policy::terminal(Net::None, vec![41000])).unwrap();
    assert!(!s.contains("(remote ip"));
    assert!(!s.contains("(local ip"));
}

#[test]
fn a_localhost_range_adds_one_rule_per_port() {
    let mut p = Policy::terminal(Net::Localhost, vec![41000, 3001]);
    p.localhost_range = Some((3000, 3003));
    let s = render(&p).unwrap();
    for port in [3000, 3001, 3002, 3003, 41000] {
        let rule = format!("(allow network-outbound (remote ip \"localhost:{port}\"))");
        assert_eq!(s.matches(&rule).count(), 1, "{port}");
    }
    assert!(!s.contains("localhost:3004"));
    assert!(!s.contains("localhost:2999"));
    assert!(!s.contains("(remote ip \"localhost:*\")"));
    p.localhost_range = Some((3000, 3000 + MAX_RANGE as u16));
    assert!(render(&p).is_err());
    p.localhost_range = Some((0, 10));
    assert!(render(&p).is_err());
    // Without network, the range gives nothing.
    let mut none = Policy::terminal(Net::None, vec![]);
    none.localhost_range = Some((3000, 3003));
    assert!(!render(&none).unwrap().contains("localhost:3000"));
}

#[test]
fn tiocsti_is_denied_last_and_ptys_are_not_opened_by_path() {
    for p in [Policy::terminal(Net::None, vec![]), sample()] {
        let s = render(&p).unwrap();
        let rules: Vec<&str> = s.lines().filter(|l| l.starts_with('(')).collect();
        assert_eq!(
            *rules.last().unwrap(),
            "(deny file-ioctl (ioctl-command TIOCSTI))"
        );
    }
    let s = render(&Policy::terminal(Net::None, vec![])).unwrap();
    for l in s
        .lines()
        .filter(|l| l.contains("ptmx") || l.contains("ttys"))
    {
        if l.starts_with("(allow") {
            assert!(l.starts_with("(allow file-ioctl "), "{l}");
        }
    }
}

#[test]
fn own_ptys_open_ptmx_and_only_ttys_with_the_pty_extension() {
    let mut p = Policy::terminal(Net::None, vec![]);
    p.own_ptys = true;
    let s = render(&p).unwrap();
    for (rule, _) in OWN_PTYS {
        assert_eq!(s.lines().filter(|l| l == rule).count(), 1, "{rule}");
    }
    // Every rule that opens a ttys needs the extension of its creator.
    for l in s
        .lines()
        .filter(|l| l.starts_with("(allow") && l.contains("ttys"))
    {
        assert!(
            l.starts_with("(allow file-ioctl ")
                || l.contains("(extension \"com.apple.sandbox.pty\")"),
            "{l}"
        );
    }
    assert!(!s.contains("(subpath \"/dev\")"));
    let rules: Vec<&str> = s.lines().filter(|l| l.starts_with('(')).collect();
    assert_eq!(
        *rules.last().unwrap(),
        "(deny file-ioctl (ioctl-command TIOCSTI))"
    );
    // Without the flag, and in app scope, the rules stay out.
    let mut app = sample();
    app.own_ptys = true;
    for s in [
        render(&Policy::terminal(Net::None, vec![])).unwrap(),
        render(&app).unwrap(),
    ] {
        assert!(!s.contains("pseudo-tty"));
        assert!(!s.contains("com.apple.sandbox.pty"));
        assert!(!s.contains("file-write* file-ioctl (literal \"/dev/ptmx\")"));
    }
}

#[test]
fn app_mode_keeps_its_services() {
    let s = render(&sample()).unwrap();
    assert!(s.lines().any(|l| l == "(allow mach-lookup)"));
    assert!(!s.contains("/private/var/select"));
    assert!(s.contains("(allow file-read* (subpath \"/dev\"))"));
}
