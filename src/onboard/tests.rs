use super::*;

#[test]
fn linux_refuses_nothing_and_macos_refuses_the_linux_features() {
    for f in FEATURES {
        assert!(available(Os::Linux, *f), "{f:?}");
        assert!(refuse_on(Os::Linux, *f).is_ok());
        let line = refusal(Os::Macos, *f).expect("macOS refuses it");
        assert!(!line.contains('\n'), "one line: {line}");
        assert!(refuse_on(Os::Macos, *f).is_err());
    }
    assert!(refusal(Os::Macos, Feature::Launchers)
        .unwrap()
        .contains("Linux only"));
    assert!(refusal(Os::Macos, Feature::UpdateBinary)
        .unwrap()
        .contains("cargo install --path ."));
    assert_eq!(refused_lines(Os::Linux), Vec::<String>::new());
    assert_eq!(refused_lines(Os::Macos).len(), FEATURES.len());
}

#[test]
fn each_platform_offers_its_own_backends() {
    assert!(backends(Os::Linux).contains(&"bwrap"));
    assert!(!backends(Os::Linux).contains(&"seatbelt"));
    assert!(backends(Os::Macos).contains(&"seatbelt"));
    assert!(!backends(Os::Macos).contains(&"bwrap"));
    assert_eq!(backends(Os::Macos)[0], "auto");
    assert_eq!(doctor_backends(Os::Macos)[0], "seatbelt");
    assert!(auto_backend_text(Os::Macos).contains("seatbelt"));
}

#[test]
fn macos_adds_a_new_zshrc_and_linux_only_existing_files() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    assert!(rc_targets(home, Os::Linux).is_empty());
    assert_eq!(rc_targets(home, Os::Macos), vec![home.join(".zshrc")]);
    std::fs::write(home.join(".bashrc"), "").unwrap();
    assert_eq!(rc_targets(home, Os::Linux), vec![home.join(".bashrc")]);
    assert_eq!(
        rc_targets(home, Os::Macos),
        vec![home.join(".bashrc"), home.join(".zshrc")]
    );
    std::fs::write(home.join(".zshrc"), "").unwrap();
    assert_eq!(rc_targets(home, Os::Macos).len(), 2, "no second .zshrc");
    // A dangling symlink is not followed and not replaced.
    std::fs::remove_file(home.join(".zshrc")).unwrap();
    std::os::unix::fs::symlink(home.join("missing"), home.join(".zshrc")).unwrap();
    assert_eq!(rc_targets(home, Os::Macos), vec![home.join(".bashrc")]);
}

#[test]
fn only_an_empty_zshrc_on_macos_goes_away() {
    let z = Path::new("/h/.zshrc");
    assert!(removes_empty_rc(z, "\n", Os::Macos));
    assert!(!removes_empty_rc(z, "alias ll='ls -l'\n", Os::Macos));
    assert!(!removes_empty_rc(z, "", Os::Linux));
    assert!(!removes_empty_rc(Path::new("/h/.bashrc"), "", Os::Macos));
}

#[test]
fn the_mac_checks_name_what_is_missing() {
    let all = MacProbe {
        version: Some("27.0.1".into()),
        sandbox_exec: true,
        developer_dir: Some("/Library/Developer/CommandLineTools".into()),
    };
    let (lines, ok) = mac_system_lines(&all);
    assert!(ok, "{lines:?}");
    assert!(lines[0].contains("ok       macOS 27.0.1"), "{lines:?}");

    let (lines, ok) = mac_system_lines(&MacProbe {
        version: Some("15.3".into()),
        ..Default::default()
    });
    assert!(!ok);
    assert!(lines[0].contains("note") && lines[0].contains("tested on macOS 27"));
    assert!(lines.iter().any(|l| l.contains("MISSING  sandbox-exec")));
    assert!(lines.iter().any(|l| l.contains("xcode-select --install")));
    assert_eq!(macos_major("27.1"), Some(27));
    assert_eq!(macos_major("x"), None);
}

#[test]
fn the_mac_dependencies_say_how_to_install_them() {
    let (lines, req, opt) = mac_dep_lines(|_| None);
    assert_eq!(lines.len(), MAC_DEPS.len());
    assert_eq!(req, vec!["xcode-select --install"]);
    assert!(opt.contains(&"brew install tmux"));
    let (_, req, opt) = mac_dep_lines(|c| Some(PathBuf::from("/usr/bin").join(c)));
    assert!(req.is_empty() && opt.is_empty());
    // Nothing of Linux: no package manager of a Linux distribution.
    for d in MAC_DEPS {
        for word in ["pacman", "apt", "dnf", "sudo"] {
            assert!(!d.install.contains(word), "{}", d.install);
        }
    }
}

#[test]
fn notification_text_is_an_argument_not_code() {
    let argv = notification_argv("-rf \"x\"", "a\nb\" & do shell script \"id\"");
    // The script is fixed. The text is the last two arguments.
    let n = argv.len();
    assert_eq!(argv[n - 2], "rf \"x\"");
    assert_eq!(argv[n - 1], "a b\" & do shell script \"id\"");
    assert!(argv[..n - 2].iter().all(|a| !a.contains("shell script")));
    assert_eq!(notification_argv("", "")[n - 2], "paddock");
}

/// Without paddock for Mac running, paddock posts with terminal-notifier
/// from Homebrew, as paddock for Mac when that app is installed, and with
/// osascript only when nothing else is there.
#[test]
fn macos_notifies_with_terminal_notifier_before_osascript() {
    let home = Path::new("/Users/demo");
    let none = |_: &Path| false;
    assert_eq!(mac_notifier(home, none), MacNotifier::Osascript);

    let intel = |p: &Path| p == Path::new("/usr/local/bin/terminal-notifier");
    assert_eq!(
        mac_notifier(home, intel),
        MacNotifier::TerminalNotifier {
            path: "/usr/local/bin/terminal-notifier".into(),
            sender: None
        }
    );

    for app in launcher_app_paths(home) {
        let both = |p: &Path| {
            p == Path::new("/opt/homebrew/bin/terminal-notifier")
                || p == Path::new("/usr/local/bin/terminal-notifier")
                || p == app
        };
        assert_eq!(
            mac_notifier(home, both),
            MacNotifier::TerminalNotifier {
                path: "/opt/homebrew/bin/terminal-notifier".into(),
                sender: Some(LAUNCHER_BUNDLE_ID)
            },
            "{}",
            app.display()
        );
    }
    // The app alone does not post: it shows notices only while it runs.
    let app_only = |p: &Path| p == Path::new("/Applications/paddock for Mac.app");
    assert_eq!(mac_notifier(home, app_only), MacNotifier::Osascript);
}

/// terminal-notifier gets the text as plain values: no leading "-" that
/// it could read as an option, no control characters, and never an empty
/// message, so it does not read stdin.
#[test]
fn terminal_notifier_text_is_a_value_not_an_option() {
    let n = MacNotifier::TerminalNotifier {
        path: "/opt/homebrew/bin/terminal-notifier".into(),
        sender: Some(LAUNCHER_BUNDLE_ID),
    };
    let (prog, args) = mac_notify_command(&n, "-execute \"id\"", "-open\nhttp://x");
    assert_eq!(prog, Path::new("/opt/homebrew/bin/terminal-notifier"));
    assert_eq!(
        args,
        [
            "-title",
            "execute \"id\"",
            "-message",
            "open http://x",
            "-sender",
            "dev.paddock.mac"
        ]
    );
    let plain = MacNotifier::TerminalNotifier {
        path: "/usr/local/bin/terminal-notifier".into(),
        sender: None,
    };
    let (_, args) = mac_notify_command(&plain, "", "");
    assert_eq!(args, ["-title", "paddock", "-message", "paddock"]);
    let (prog, args) = mac_notify_command(&MacNotifier::Osascript, "t", "b");
    assert_eq!(prog, Path::new(OSASCRIPT));
    assert_eq!(args, notification_argv("t", "b"));
}

/// The doctor names the way, and says when it is the osascript fallback.
#[test]
fn doctor_says_how_macos_notifies() {
    let tn = MacNotifier::TerminalNotifier {
        path: "/opt/homebrew/bin/terminal-notifier".into(),
        sender: None,
    };
    let running = mac_notifier_line(&MacNotifier::Osascript, &[4321]);
    assert!(
        running.contains("paddock for Mac shows them (pid 4321)"),
        "{running}"
    );
    assert!(running.starts_with("  ok "), "{running}");
    let plain = mac_notifier_line(&tn, &[]);
    assert!(
        plain.contains("terminal-notifier") && plain.contains("Install paddock for Mac"),
        "{plain}"
    );
    let fallback = mac_notifier_line(&MacNotifier::Osascript, &[]);
    assert!(fallback.starts_with("  fallback "), "{fallback}");
    assert!(
        fallback.contains("osascript") && fallback.contains("Script Editor"),
        "{fallback}"
    );
    assert!(
        fallback.contains("brew install terminal-notifier"),
        "{fallback}"
    );
}

#[test]
fn macos_opens_terminal_and_linux_needs_a_terminal() {
    let e = terminal_command(None, None, Os::Linux).unwrap_err();
    assert!(e.to_string().contains("no terminal to open"), "{e}");
    let x = terminal_command(
        Some(Path::new("/usr/bin/xdg-terminal-exec")),
        None,
        Os::Macos,
    );
    assert_eq!(
        x.unwrap(),
        vec![OsString::from("/usr/bin/xdg-terminal-exec")]
    );
    let t = terminal_command(None, Some("kitty"), Os::Macos).unwrap();
    assert_eq!(t, vec![OsString::from("kitty"), "-e".into()]);
    let m = terminal_command(None, None, Os::Macos).unwrap();
    assert_eq!(m[0], OsString::from(OSASCRIPT));
    assert!(m
        .iter()
        .any(|a| a.to_str().unwrap().contains("quoted form of")));
    assert!(m.iter().any(|a| a.to_str().unwrap() == "do script cmd"));
}
