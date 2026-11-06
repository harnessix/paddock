use super::*;
use std::cell::RefCell;

struct Fake {
    calls: RefCell<Vec<String>>,
    /// The version that a new copy reports.
    new_version: String,
    old_version: String,
    binary: PathBuf,
}

impl Fake {
    fn new(p: &Paths) -> Self {
        Fake {
            calls: RefCell::default(),
            new_version: format!("paddock {}", env!("CARGO_PKG_VERSION")),
            old_version: "paddock 0.0.1".into(),
            binary: p.binary.clone(),
        }
    }
}

impl Runner for Fake {
    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(format!("{program} {}", args.join(" ")));
        Ok(())
    }
    fn version(&self, bin: &Path) -> Result<String> {
        Ok(if bin == self.binary {
            self.old_version.clone()
        } else {
            self.new_version.clone()
        })
    }
}

fn me() -> u32 {
    unsafe { libc::geteuid() }
}

struct Box_ {
    t: tempfile::TempDir,
    p: Paths,
    src: PathBuf,
}

fn fixture() -> Box_ {
    let t = tempfile::tempdir().unwrap();
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let p = Paths::under(&t.path().join("root"));
    let src = t.path().join("paddock-new");
    std::fs::write(&src, "new binary").unwrap();
    Box_ { t, p, src }
}

fn stop(b: &Box_) -> PathBuf {
    b.t.path().to_path_buf()
}

#[test]
fn every_unit_option_is_explained_by_a_comment_above_it() {
    let text = unit_text(&Paths::system());
    assert!(text.lines().next().unwrap().contains(MARK));
    let mut in_service = false;
    let mut commented = false;
    for l in text.lines() {
        if l.starts_with('[') {
            in_service = l == "[Service]";
            commented = false;
            continue;
        }
        if l.starts_with('#') {
            commented = true;
        } else if l.is_empty() {
            commented = false;
        } else if in_service {
            assert!(commented, "no comment explains {l:?}");
        }
    }
}

// The systemd unit is Linux only; macOS uses a launchd plist.
#[cfg(target_os = "linux")]
#[test]
fn the_unit_runs_the_root_copy_with_least_privilege() {
    let p = Paths::system();
    let text = unit_text(&p);
    for want in [
        "ExecStart=/usr/local/libexec/paddock/tackd tack serve",
        "User=root",
        "CapabilityBoundingSet=CAP_DAC_READ_SEARCH CAP_SYS_PTRACE CAP_SETUID CAP_SETGID CAP_KILL CAP_DAC_OVERRIDE\n",
        "NoNewPrivileges=yes",
        "RuntimeDirectory=paddock-tackd",
        "RuntimeDirectoryMode=0755",
        "LogsDirectoryMode=0700",
        "KillMode=process",
        "Restart=always",
        "WantedBy=multi-user.target",
    ] {
        assert!(text.contains(want), "missing {want:?}");
    }
    // A started program inherits the unit's namespaces and filters: none
    // that would change what it sees or can do.
    for gone in [
        "ProtectSystem",
        "ProtectHome",
        "PrivateTmp",
        "PrivateNetwork",
        "PrivateDevices",
        "RestrictAddressFamilies",
        "SystemCallFilter",
        "MemoryDenyWriteExecute",
        "RestrictNamespaces",
        "IPAddressDeny",
        "ProtectHostname",
        "SystemCallArchitectures",
    ] {
        assert!(
            !text.lines().any(|l| l.starts_with(gone)),
            "{gone} would reach the started programs"
        );
    }
    if !cfg!(target_os = "macos") {
        assert_eq!(p.socket, Path::new("/run/paddock-tackd/tackd.sock"));
        assert_eq!(p.audit, Path::new("/var/log/paddock/tackd.log"));
    }
}

#[test]
fn the_plist_runs_the_helper_copy_as_root() {
    let p = Paths {
        binary: PathBuf::from("/Library/PrivilegedHelperTools/io.paddock.tackd"),
        unit: PathBuf::from("/Library/LaunchDaemons/io.paddock.tackd.plist"),
        socket: PathBuf::from("/var/run/paddock-tackd/tackd.sock"),
        audit: PathBuf::from("/var/log/paddock/tackd.log"),
        stderr: Some(PathBuf::from("/var/log/paddock/<&>.err")),
    };
    let text = plist_text(&p);
    assert!(text.lines().nth(1).unwrap().contains(MARK));
    for want in [
        "<string>io.paddock.tackd</string>",
        "<string>/Library/PrivilegedHelperTools/io.paddock.tackd</string>\n    <string>tack</string>\n    <string>serve</string>",
        "<key>UserName</key>\n  <string>root</string>",
        "<key>KeepAlive</key>\n  <true/>",
        "/var/log/paddock/&lt;&amp;&gt;.err",
    ] {
        assert!(text.contains(want), "missing {want:?}\n{text}");
    }
}

#[test]
fn setup_installs_the_copy_and_starts_the_service() {
    let b = fixture();
    let r = Fake::new(&b.p);
    let rep = setup(&b.p, &b.src, me(), &stop(&b), &r).unwrap();
    assert_eq!(std::fs::read(&b.p.binary).unwrap(), b"new binary");
    let m = std::fs::metadata(&b.p.binary).unwrap();
    assert_eq!(m.mode() & 0o777, 0o755);
    assert_eq!(std::fs::metadata(&b.p.unit).unwrap().mode() & 0o777, 0o644);
    assert!(ours(&b.p.unit));
    assert_eq!(rep.upgraded_from, None);
    let calls = r.calls.borrow().join("\n");
    if cfg!(target_os = "macos") {
        assert!(calls.contains("launchctl bootstrap system"), "{calls}");
    } else {
        assert_eq!(calls, "systemctl daemon-reload\nsystemctl enable paddock-tackd.service\nsystemctl restart paddock-tackd.service");
    }
    // No temporary file is left next to the binary.
    let left: Vec<_> = std::fs::read_dir(b.p.binary.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
}

#[test]
fn setup_again_upgrades_in_place_and_restarts() {
    let b = fixture();
    let r = Fake::new(&b.p);
    setup(&b.p, &b.src, me(), &stop(&b), &r).unwrap();
    std::fs::write(&b.src, "newer binary").unwrap();
    let rep = setup(&b.p, &b.src, me(), &stop(&b), &r).unwrap();
    assert_eq!(rep.upgraded_from.as_deref(), Some("paddock 0.0.1"));
    assert!(rep
        .lines
        .iter()
        .any(|l| l.contains("restarted tackd (was paddock 0.0.1)")));
    assert_eq!(std::fs::read(&b.p.binary).unwrap(), b"newer binary");
}

#[test]
fn a_copy_that_does_not_report_this_version_leaves_the_old_tackd() {
    let b = fixture();
    let r = Fake::new(&b.p);
    setup(&b.p, &b.src, me(), &stop(&b), &r).unwrap();
    let unit_before = std::fs::read(&b.p.unit).unwrap();
    r.calls.borrow_mut().clear();
    std::fs::write(&b.src, "broken binary").unwrap();
    let bad = Fake {
        new_version: "segfault".into(),
        ..Fake::new(&b.p)
    };
    let e = format!(
        "{:#}",
        setup(&b.p, &b.src, me(), &stop(&b), &bad).unwrap_err()
    );
    assert!(
        e.contains("old tackd, if any, is unchanged") && e.contains("segfault"),
        "{e}"
    );
    assert_eq!(std::fs::read(&b.p.binary).unwrap(), b"new binary");
    assert_eq!(std::fs::read(&b.p.unit).unwrap(), unit_before);
    assert!(bad.calls.borrow().is_empty());
    let left = std::fs::read_dir(b.p.binary.parent().unwrap())
        .unwrap()
        .count();
    assert_eq!(left, 1);
}

#[test]
fn a_service_file_that_paddock_did_not_write_stops_setup() {
    let b = fixture();
    std::fs::create_dir_all(b.p.unit.parent().unwrap()).unwrap();
    std::fs::write(&b.p.unit, "[Service]\nExecStart=/bin/mine\n").unwrap();
    let r = Fake::new(&b.p);
    let e = setup(&b.p, &b.src, me(), &stop(&b), &r)
        .unwrap_err()
        .to_string();
    assert!(e.contains("paddock did not write it"), "{e}");
    assert!(!b.p.binary.exists());
    // remove leaves it alone too.
    let out = remove(&b.p, false, &r).unwrap();
    assert!(out[0].contains("left") && b.p.unit.exists(), "{out:?}");
}

#[test]
fn a_folder_that_others_can_write_stops_setup() {
    let b = fixture();
    let open = b.p.binary.parent().unwrap().parent().unwrap();
    std::fs::create_dir_all(open).unwrap();
    std::fs::set_permissions(open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let e = setup(&b.p, &b.src, me(), &stop(&b), &Fake::new(&b.p))
        .unwrap_err()
        .to_string();
    assert!(e.contains("chmod go-w"), "{e}");
    // Owned by another uid: the message names it.
    let e = safe_folders(b.t.path(), me() + 1, b.t.path())
        .unwrap_err()
        .to_string();
    assert!(e.contains("not root"), "{e}");
    // A link in the chain.
    let link = b.t.path().join("link");
    std::os::unix::fs::symlink(b.t.path(), &link).unwrap();
    let e = safe_folders(&link, me(), b.t.path())
        .unwrap_err()
        .to_string();
    assert!(e.contains("symbolic link"), "{e}");
}

fn files_under(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn remove_leaves_nothing_behind_and_can_run_twice() {
    let b = fixture();
    let r = Fake::new(&b.p);
    setup(&b.p, &b.src, me(), &stop(&b), &r).unwrap();
    // What a running tackd adds: the socket and the audit log, rotated.
    std::fs::create_dir_all(b.p.socket.parent().unwrap()).unwrap();
    std::os::unix::net::UnixListener::bind(&b.p.socket).unwrap();
    std::fs::create_dir_all(b.p.audit.parent().unwrap().parent().unwrap()).unwrap();
    let log = crate::tack::audit::Log::new(b.p.audit.clone(), me());
    log.check().unwrap();
    std::fs::write(b.p.audit.with_extension("log.1"), "{}\n").unwrap();
    if let Some(e) = &b.p.stderr {
        std::fs::write(e, "x").unwrap();
    }
    r.calls.borrow_mut().clear();
    let out = remove(&b.p, false, &r).unwrap();
    assert!(
        files_under(&b.t.path().join("root")).is_empty(),
        "{:?}",
        files_under(&b.t.path().join("root"))
    );
    assert!(!b.p.audit.parent().unwrap().exists());
    assert!(!b.p.socket.parent().unwrap().exists());
    assert!(
        out.iter()
            .any(|l| l.contains("deleted") && l.contains("tackd.log.1")),
        "{out:?}"
    );
    if !cfg!(target_os = "macos") {
        assert!(!b.p.binary.parent().unwrap().exists());
        let calls = r.calls.borrow().join("\n");
        assert!(
            calls.starts_with(
                "systemctl disable --now paddock-tackd.service\nsystemctl daemon-reload"
            ),
            "{calls}"
        );
    }
    assert!(remove(&b.p, false, &r).unwrap().is_empty());
}

#[test]
fn remove_can_keep_the_audit_log() {
    let b = fixture();
    let r = Fake::new(&b.p);
    setup(&b.p, &b.src, me(), &stop(&b), &r).unwrap();
    std::fs::create_dir_all(b.p.audit.parent().unwrap().parent().unwrap()).unwrap();
    crate::tack::audit::Log::new(b.p.audit.clone(), me())
        .check()
        .unwrap();
    let out = remove(&b.p, true, &r).unwrap();
    assert!(b.p.audit.exists());
    assert!(out.last().unwrap().contains("--keep-audit"), "{out:?}");
}

#[test]
fn the_system_runner_reads_a_version_and_times_out_a_hang() {
    let t = tempfile::tempdir().unwrap();
    let ok = t.path().join("ok");
    std::fs::write(&ok, "#!/bin/sh\necho paddock 9.9.9\n").unwrap();
    std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(System.version(&ok).unwrap(), "paddock 9.9.9");
    let bad = t.path().join("bad");
    std::fs::write(&bad, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(System
        .version(&bad)
        .unwrap_err()
        .to_string()
        .contains("failed"));
    assert!(System
        .run("false", &[])
        .unwrap_err()
        .to_string()
        .contains("false"));
}
