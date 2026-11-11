use super::*;
use std::collections::HashSet;

#[test]
fn a_refused_sudo_password_is_told_apart_from_other_failures() {
    use crate::macvm::proto::StepResult;
    let r = |name: &str, ok: bool, output: &str| StepResult {
        name: name.into(),
        ok,
        output: output.into(),
        ..Default::default()
    };
    let root = STEPS.iter().find(|s| s.as_root).unwrap().name;
    let user = STEPS.iter().find(|s| !s.as_root).map(|s| s.name);
    let refused = "Sorry, try again.\nsudo: 1 incorrect password attempt";
    assert!(wrong_password(&[r(root, false, refused)]));
    assert!(wrong_password(&[
        r(root, true, ""),
        r(root, false, "sudo: 1 incorrect password attempt"),
    ]));
    assert!(!wrong_password(&[]));
    assert!(!wrong_password(&[r(root, true, refused)]), "it ran");
    assert!(!wrong_password(&[r(root, false, "pmset: bad value")]));
    assert!(!wrong_password(&[r("not-a-step", false, refused)]));
    if let Some(user) = user {
        assert!(!wrong_password(&[r(user, false, refused)]), "no sudo");
    }
}

#[test]
fn every_step_has_verify_and_absolute_paths() {
    let mut names = HashSet::new();
    for s in STEPS {
        assert!(names.insert(s.name), "{} is listed twice", s.name);
        assert!(!s.why.is_empty(), "{} has no reason", s.name);
        assert!(!s.verify.is_empty(), "{} has no verify command", s.name);
        assert!(!s.expect.is_empty(), "{} expects nothing", s.name);
        assert!(
            is_step_command(s.argv[0]),
            "{}: {} is not a system command",
            s.name,
            s.argv[0]
        );
        assert!(
            is_step_command(s.verify[0]),
            "{}: {} is not a system command",
            s.name,
            s.verify[0]
        );
        // No step may need the Automation permission: nobody is
        // there to click its prompt.
        for a in s.argv.iter().chain(s.verify.iter()) {
            assert!(!a.contains("System Events"), "{}", s.name);
            assert!(!a.contains("osascript"), "{}", s.name);
        }
    }
    assert!(is_system_command("/usr/bin/defaults"));
    assert!(!is_system_command("defaults"));
    assert!(!is_system_command("/usr/local/bin/x"));
    assert!(!is_system_command("/usr/bin/"));
}

#[test]
fn wire_form_and_proof_of_a_step() {
    use crate::macvm::proto::StepResult;
    let s = step("autologin").unwrap();
    let w = s.wire();
    assert_eq!(w.name, "autologin");
    assert!(w.as_root);
    assert_eq!(w.argv[0], s.argv[0]);
    assert_eq!(w.verify.len(), s.verify.len());
    let ok = StepResult {
        name: "autologin".into(),
        ok: true,
        output: String::new(),
        verify: format!("x {} y", s.expect), // not user text
        ..Default::default()
    };
    assert!(s.proven(&ok));
    let mut failed = ok.clone();
    failed.ok = false;
    assert!(!s.proven(&failed));
    let mut unproven = ok.clone();
    unproven.verify = "nothing".into();
    assert!(!s.proven(&unproven));
    assert_eq!(STEPS.iter().map(Step::wire).count(), STEPS.len());
}

#[test]
fn no_step_argv_contains_the_password() {
    // The agent gets the password apart from the list and writes it
    // on stdin. No argument carries it or a place holder for it.
    let pw = crate::macvm::golden::make_password().unwrap();
    for s in STEPS {
        for a in s.argv.iter().chain(s.verify.iter()) {
            assert!(!a.contains(&pw) && !a.contains("{password}"), "{}", s.name);
            assert!(!a.to_ascii_lowercase().contains("password="), "{}", s.name);
        }
        if let Some(i) = s.argv.iter().position(|a| *a == "-password") {
            assert_eq!(
                s.argv[i + 1],
                "-",
                "{} reads the password from stdin",
                s.name
            );
        }
    }
}

#[test]
fn the_host_waits_for_every_step_to_time_out_twice() {
    assert_eq!(request_secs(0), 300);
    assert_eq!(
        request_secs(STEPS.len()),
        (STEPS.len() as u64 * 2 * STEP_TIMEOUT_SECS + 60).max(300)
    );
    assert!(request_secs(18) >= 18 * 2 * STEP_TIMEOUT_SECS);
    let r = crate::macvm::proto::Request::Configure {
        password: String::new(),
        steps: STEPS.iter().map(Step::wire).collect(),
    };
    assert_eq!(
        crate::macvm::proto::timeout(&r).as_secs(),
        request_secs(STEPS.len())
    );
}

#[test]
fn report_names_each_step_and_the_failed_ones() {
    use crate::macvm::proto::StepResult;
    let mut results: Vec<StepResult> = STEPS
        .iter()
        .map(|s| StepResult {
            name: s.name.into(),
            ok: true,
            verify: s.expect.into(),
            code: Some(0),
            ms: 120,
            ..Default::default()
        })
        .collect();
    let (lines, failed) = report(&results);
    assert_eq!(lines.len(), STEPS.len());
    assert!(failed.is_empty(), "{failed:?}");
    assert!(
        lines[0].contains("autologin") && lines[0].contains("exit 0"),
        "{}",
        lines[0]
    );
    results[1].ok = false;
    results[1].code = None;
    results[1].timed_out = true;
    results[1].output = "killed after 30 s".into();
    results.remove(2);
    let (lines, failed) = report(&results);
    assert_eq!(failed, [STEPS[1].name, STEPS[2].name]);
    assert!(lines[1].contains("timed out"), "{}", lines[1]);
    assert!(lines[2].contains("no result"), "{}", lines[2]);
}

#[test]
fn the_shutdown_is_a_root_step_of_a_system_command() {
    let s = shutdown_step();
    assert!(s.as_root && is_system_command(&s.argv[0]));
    assert_eq!(s.argv, ["/sbin/shutdown", "-h", "now"]);
}

#[test]
fn the_dropped_steps_are_gone_and_no_step_needs_a_permission() {
    for name in DROPPED {
        assert!(step(name).is_none(), "{name} is back");
    }
    // No step runs tmutil (Full Disk Access) or an agent op that
    // reads the old desktop picture API.
    for s in STEPS {
        assert!(!s.argv.contains(&"/usr/bin/tmutil"), "{}", s.name);
        assert_ne!(s.argv[0], AGENT_OP, "{}", s.name);
    }
    assert_eq!(STEPS.len(), 20);
}

#[test]
fn the_downloads_step_only_reads_the_link_that_the_trust_made() {
    let s = step("downloads-link").unwrap();
    assert!(!s.as_root);
    assert_eq!(s.expect, DOWNLOADS_TARGET);
    // Nothing of the step removes, moves or writes: only `test -L`
    // and `readlink`, which macOS does not guard with a prompt.
    let all = s.argv.join(" ") + " " + &s.verify.join(" ");
    for word in ["rm ", "ln ", "mv ", "Library"] {
        assert!(!all.contains(word), "{word} in {all}");
    }
    assert!(s.argv[2].starts_with("test -L "), "{:?}", s.argv);
    assert!(s.verify[2].starts_with("readlink "), "{:?}", s.verify);
    assert_eq!(
        crate::guest::downloads::share_path(),
        std::path::Path::new(DOWNLOADS_TARGET)
    );
}

#[test]
fn proxy_steps_name_the_loopback_proxy() {
    for name in ["proxy-http", "proxy-https"] {
        let s = step(name).unwrap();
        assert!(
            s.argv.contains(&"127.0.0.1") && s.argv.contains(&"3128"),
            "{name}"
        );
        assert!(s.as_root);
        assert!(s.argv.contains(&SERVICE) && s.verify.contains(&SERVICE));
    }
    assert!(step("proxy-no-bypass").is_some());
}

/// Only the guest loopback goes around the proxy: a login callback to
/// `localhost` stays in the guest, every host name goes through the proxy.
#[test]
fn the_bypass_list_is_the_guest_loopback_and_nothing_else() {
    use crate::macvm::proto::StepResult;
    let s = step("proxy-no-bypass").unwrap();
    assert!(s.as_root);
    assert_eq!(
        s.argv,
        [
            "/usr/sbin/networksetup",
            "-setproxybypassdomains",
            SERVICE,
            "localhost",
            "127.0.0.1",
            "::1"
        ]
    );
    assert!(!s.argv.contains(&"Empty"));
    assert!(!s.argv.iter().any(|a| a.contains('*') || a.starts_with('.')));
    let result = |verify: &str| StepResult {
        name: s.name.into(),
        ok: true,
        verify: verify.into(),
        ..Default::default()
    };
    assert!(s.proven(&result("localhost\n127.0.0.1\n::1\n")));
    assert!(s.proven(&result("::1\nlocalhost\n127.0.0.1")), "any order");
    assert!(
        !s.proven(&result("localhost\n127.0.0.1\n::1\n*.local\n")),
        "one more name"
    );
    assert!(!s.proven(&result("localhost\n127.0.0.1\n")), "::1 missing");
    assert!(!s.proven(&result(
        "There aren't any bypass domains set on com.redhat.spice.0."
    )));
    // The verify output of the quiet setup check is the expected text.
    assert!(s.proven(&result(s.expect)));
}

#[test]
fn service_is_resolved_from_the_listing() {
    let listing =
        "An asterisk (*) denotes that a network service is disabled.\ncom.redhat.spice.0\n";
    assert_eq!(pick_service(listing).as_deref(), Some("com.redhat.spice.0"));
    assert_eq!(
        pick_service("header\n*Disabled One\nEthernet\n").as_deref(),
        Some("Ethernet")
    );
    assert_eq!(pick_service("header only\n"), None);
    assert_eq!(pick_service(""), None);
    let argv = step("proxy-https").unwrap().wire().argv;
    let r = resolve_service(&argv, "com.redhat.spice.0");
    assert_eq!(
        r,
        [
            "/usr/sbin/networksetup",
            "-setsecurewebproxy",
            "com.redhat.spice.0",
            "127.0.0.1",
            "3128"
        ]
    );
    assert!(!r.iter().any(|a| a == SERVICE));
}

#[test]
fn launch_env_points_every_proxy_variable_at_the_guest_proxy() {
    let env = launch_env();
    for var in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        assert_eq!(env.get(var).map(String::as_str), Some(PROXY_URL), "{var}");
    }
    // The guest loopback only, as the bypass list of the system proxy.
    for var in ["NO_PROXY", "no_proxy"] {
        assert_eq!(
            env.get(var).map(String::as_str),
            Some("localhost,127.0.0.1,::1"),
            "{var}"
        );
    }
    assert_eq!(env.len(), crate::proxy::PROXY_VARS.len());
}

#[test]
fn autologin_names_the_paddock_account() {
    let s = step("autologin").unwrap();
    assert!(s.argv.contains(&"paddock"));
    assert_eq!(s.expect, "paddock");
    assert!(s.as_root);
}

#[test]
fn the_list_covers_every_configure_step() {
    for n in [
        "autologin",
        "no-sleep",
        "no-screen-saver",
        "no-screen-lock",
        "no-software-update-check",
        "no-spotlight",
        "proxy-http",
        "proxy-https",
        "dock-hidden",
        "menu-bar-hidden",
        "no-banners",
        "downloads-link",
    ] {
        assert!(step(n).is_some(), "{n} is missing");
    }
    assert_eq!(names().len(), STEPS.len());
}

#[test]
fn the_lock_settings_are_also_set_where_no_machine_id_counts() {
    // Each profile is a new machine to macOS: the screen saver and the
    // lock must also be off in the domains of every host.
    for name in [
        "no-screen-saver-any-host",
        "no-saver-password",
        "no-saver-password-delay",
        "no-login-window-saver",
        "no-auto-logout",
    ] {
        let s = step(name).unwrap_or_else(|| panic!("{name} is missing"));
        assert!(!s.argv.contains(&"-currentHost"), "{name}");
        assert_eq!(s.expect, "0", "{name}");
    }
}
