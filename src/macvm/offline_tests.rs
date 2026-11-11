use super::*;
use crate::macvm::apfs_fixture::Entry;
use crate::macvm::offline_fake::{FakeDisk, Store};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

/// A golden folder with a disk and a seed folder with an agent.
struct World {
    t: tempfile::TempDir,
    disk: PathBuf,
    seed: PathBuf,
    mnt: PathBuf,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let disk = t.path().join("disk.img");
    let seed = t.path().join("seed");
    let agent = t.path().join("paddock-guest");
    std::fs::write(&agent, "#!/bin/sh\necho agent\n").unwrap();
    crate::macvm::seed::prepare(&seed, Some(&agent)).unwrap();
    let mnt = t.path().join("mnt");
    World { t, disk, seed, mnt }
}

impl World {
    /// The image written by `detach` must be read back after each call,
    /// because the fake rewrites it from the model there.
    fn run(&self, d: &mut FakeDisk) -> Result<Report> {
        let files = files(&self.seed).unwrap();
        let disk = self.disk.clone();
        // The fake stores the model at each detach.
        let mut wrapped = Store {
            d,
            image: disk.clone(),
        };
        seed_disk(&mut wrapped, &disk, &self.mnt, &files)
    }
}

#[test]
fn the_files_hold_the_agent_tree_the_script_and_the_daemon_last() {
    let w = world();
    let f = files(&w.seed).unwrap();
    let paths: Vec<&str> = f.iter().map(|g| g.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Library/Application Support/paddock-firstboot/firstboot.sh",
            "Library/Application Support/paddock-firstboot/install.sh",
            "Library/Application Support/paddock-firstboot/dev.paddock.guest.plist",
            "Library/Application Support/paddock-firstboot/agent/launch",
            "Library/Application Support/paddock-firstboot/agent/current/paddock-guest",
            "Library/LaunchDaemons/dev.paddock.firstboot.plist",
        ]
    );
    let mode = |p: &str| f.iter().find(|g| g.path.ends_with(p)).unwrap().mode;
    assert_eq!(mode("firstboot.sh"), 0o755);
    assert_eq!(mode("install.sh"), 0o644);
    assert_eq!(mode("paddock-guest"), 0o755);
    assert_eq!(mode("dev.paddock.firstboot.plist"), 0o644);
    assert_eq!(f[1].body, seed::INSTALL.as_bytes());
    // Without an agent binary the tree has no current/paddock-guest.
    std::fs::remove_file(w.seed.join("agent/current/paddock-guest")).unwrap();
    assert_eq!(files(&w.seed).unwrap().len(), 5);
    // A seed folder without install.sh is an error that names it.
    std::fs::remove_file(w.seed.join("install.sh")).unwrap();
    let e = files(&w.seed).unwrap_err();
    assert!(format!("{e:#}").contains("install.sh"), "{e:#}");
}

#[test]
fn the_daemon_plist_runs_the_script_once_as_root_at_load() {
    for part in [
        "<string>dev.paddock.firstboot</string>",
        "<key>RunAtLoad</key>",
        "<string>/bin/sh</string>",
        "<string>/Library/Application Support/paddock-firstboot/firstboot.sh</string>",
    ] {
        assert!(DAEMON_PLIST.contains(part), "{part}");
    }
    assert!(!DAEMON_PLIST.contains("KeepAlive"), "it must not run again");
    assert!(!DAEMON_PLIST.contains("UserName"), "it runs as root");
    assert!(DAEMON_PLIST.contains(&format!("<string>{DAEMON_LABEL}</string>")));
    assert!(DAEMON.ends_with(&format!("{DAEMON_LABEL}.plist")));
    assert!(FIRSTBOOT.contains(STAGE.trim_start_matches("Library/Application Support/")));
}

#[test]
fn write_notes_each_made_folder_before_its_files_and_replaces_an_old_stage() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    // A missing folder on the way is made and noted.
    let made = write(
        root,
        &[GuestFile {
            path: "Library/x".into(),
            body: vec![],
            mode: 0o600,
        }],
    )
    .unwrap();
    assert_eq!(made, ["Library", "Library/x"]);
    std::fs::create_dir_all(root.join("Library/LaunchDaemons")).unwrap();
    std::fs::create_dir_all(root.join(STAGE).join("old")).unwrap();
    std::fs::write(root.join(STAGE).join("old/stale"), "x").unwrap();
    let f = vec![
        GuestFile {
            path: format!("{STAGE}/a/b.sh"),
            body: b"b".to_vec(),
            mode: 0o755,
        },
        GuestFile {
            path: DAEMON.into(),
            body: b"p".to_vec(),
            mode: 0o644,
        },
    ];
    let made = write(root, &f).unwrap();
    assert_eq!(
        made,
        [
            "Library/Application Support/paddock-firstboot",
            "Library/Application Support/paddock-firstboot/a",
            "Library/Application Support/paddock-firstboot/a/b.sh",
            "Library/LaunchDaemons/dev.paddock.firstboot.plist",
        ]
    );
    assert!(!root.join(STAGE).join("old").exists());
    let m = |p: &str| {
        std::fs::metadata(root.join(p))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(m(&format!("{STAGE}/a/b.sh")), 0o755);
    assert_eq!(m(DAEMON), 0o644);
    assert_eq!(m(STAGE), 0o755);
}

#[test]
fn the_seed_writes_the_files_and_makes_exactly_them_root_owned() {
    let w = world();
    let mut d = FakeDisk::new(&w.disk);
    let r = w.run(&mut d).unwrap();
    assert_eq!(d.calls, ["clone", "attach", "detach", "fsck"]);
    assert_eq!(r.fsck, Fsck::Clean);
    assert!(r.owned.contains(&DAEMON.to_string()));
    assert!(r.owned.contains(&STAGE.to_string()));
    assert!(r.owned.iter().any(|p| p.ends_with("current/paddock-guest")));
    assert_eq!(r.owned.len(), 9, "{:?}", r.owned);
    for p in &r.owned {
        assert_eq!(FakeDisk::owner(&w.disk, p), (0, 0), "{p}");
    }
    // The folders that were there keep their owner; nothing else is new.
    assert_eq!(FakeDisk::owner(&w.disk, "Library/LaunchDaemons"), (0, 0));
    let new: Vec<&Entry> = d
        .model
        .iter()
        .filter(|e| e.uid == apfs::NOOWNERS_ID)
        .collect();
    assert_eq!(new.len(), r.owned.len(), "the model before the patch");
    assert!(!backup_of(&w.disk).exists(), "the clone goes after a seed");
    assert!(!w.mnt.exists(), "the mount folder goes");
    drop(w.t);
}

#[test]
fn every_failure_leaves_the_disk_as_it_was() {
    type Break = fn(&mut FakeDisk);
    let cases: [(&str, Break); 5] = [
        ("clone", |d| d.fail_clone = true),
        ("attach", |d| d.fail_attach = true),
        ("detach", |d| d.fail_detach = true),
        ("fsck", |d| {
            d.fsck = Err("fsck_apfs: the B-tree is invalid".into())
        }),
        ("write", |d| {
            // A file where the first-boot folder goes: the write fails.
            d.model.push(Entry {
                path: STAGE.into(),
                dir: false,
                uid: 0,
                gid: 0,
            })
        }),
    ];
    for (name, brk) in cases {
        let w = world();
        let mut d = FakeDisk::new(&w.disk);
        brk(&mut d);
        d.store(&w.disk);
        let before = std::fs::read(&w.disk).unwrap();
        let e = w.run(&mut d).expect_err(name);
        assert!(!format!("{e:#}").is_empty());
        assert_eq!(
            std::fs::read(&w.disk).unwrap(),
            before,
            "{name}: the disk changed"
        );
        assert!(!backup_of(&w.disk).exists(), "{name}: the clone stays");
        if name == "write" {
            assert!(
                d.calls.contains(&"detach".to_string()),
                "a failed write still detaches"
            );
        }
    }
}

#[test]
fn a_clone_left_by_a_crash_is_put_back_first() {
    let w = world();
    let mut d = FakeDisk::new(&w.disk);
    let good = std::fs::read(&w.disk).unwrap();
    std::fs::write(backup_of(&w.disk), &good).unwrap();
    std::fs::write(&w.disk, b"half patched").unwrap();
    w.run(&mut d).unwrap();
    assert_eq!(FakeDisk::owner(&w.disk, DAEMON), (0, 0));
    assert!(restore_leftover(&w.disk).is_ok_and(|b| !b));
}

#[test]
fn a_second_seed_on_a_seeded_disk_works() {
    let w = world();
    let mut d = FakeDisk::new(&w.disk);
    w.run(&mut d).unwrap();
    // The model still has the first files, root-owned.
    let r = w.run(&mut d).unwrap();
    for p in &r.owned {
        assert_eq!(FakeDisk::owner(&w.disk, p), (0, 0), "{p}");
    }
}

#[test]
fn a_skipped_fsck_is_not_a_failure() {
    let w = world();
    let mut d = FakeDisk::new(&w.disk);
    d.fsck = Ok(Fsck::Skipped("Permission denied".into()));
    assert_eq!(
        w.run(&mut d).unwrap().fsck,
        Fsck::Skipped("Permission denied".into())
    );
}

// ---- firstboot.sh ------------------------------------------------------

/// A guest root with the first-boot files in place (as the disk would
/// hold them), a fake `chown` and a fake `launchctl` first on PATH.
struct Boot {
    _t: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Boot {
    fn new(agent: bool) -> Boot {
        let w = world();
        if !agent {
            std::fs::remove_file(w.seed.join("agent/launch")).unwrap();
        }
        let root = w.t.path().join("guest");
        std::fs::create_dir_all(&root).unwrap();
        let mut fs = Vec::new();
        for name in [
            "install.sh",
            "dev.paddock.guest.plist",
            "agent/current/paddock-guest",
        ] {
            fs.push(GuestFile {
                path: format!("{STAGE}/{name}"),
                body: std::fs::read(w.seed.join(name)).unwrap(),
                mode: 0o644,
            });
        }
        if agent {
            fs.push(GuestFile {
                path: format!("{STAGE}/agent/launch"),
                body: std::fs::read(w.seed.join("agent/launch")).unwrap(),
                mode: 0o755,
            });
        }
        fs.push(GuestFile {
            path: format!("{STAGE}/firstboot.sh"),
            body: FIRSTBOOT.into(),
            mode: 0o755,
        });
        fs.push(GuestFile {
            path: DAEMON.into(),
            body: DAEMON_PLIST.into(),
            mode: 0o644,
        });
        write(&root, &fs).unwrap();
        let bin = w.t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = w.t.path().join("calls.log");
        let b = Boot {
            _t: w.t,
            root,
            bin,
            log,
        };
        b.fake("chown", "exit 0");
        b
    }

    fn fake(&self, name: &str, then: &str) {
        let p = self.bin.join(name);
        let body = format!(
            "#!/bin/sh\necho \"{name} $*\" >> \"{}\"\n{then}\n",
            self.log.display()
        ); // not user text
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(&self, wait: u32) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new("/bin/sh")
            .arg(self.root.join(STAGE).join("firstboot.sh"))
            .env("PADDOCK_FIRSTBOOT_ROOT", &self.root)
            .env("PADDOCK_FIRSTBOOT_WAIT", wait.to_string())
            .env("PADDOCK_FIRSTBOOT_SLEEP", "1")
            .env("PATH", path)
            .output()
            .unwrap()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.replace(&self.root.display().to_string(), "ROOT"))
            .collect()
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(self.root.join("Library/Logs/paddock-firstboot.log"))
            .unwrap_or_default()
    }
}

#[test]
fn firstboot_installs_starts_the_agent_after_the_login_and_removes_itself() {
    let b = Boot::new(true);
    std::fs::create_dir_all(b.root.join("Users/paddock")).unwrap();
    // gui/501 exists, the agent is not loaded yet.
    b.fake(
        "launchctl",
        "case \"$1 $2\" in \"print gui/501\") exit 0;; \"print gui/501/dev.paddock.guest\") exit 113;; esac; exit 0",
    );
    let out = b.run(10);
    assert!(out.status.success(), "{out:?}");
    let agent = b.root.join("Library/Application Support/paddock");
    assert!(agent.join("current/paddock-guest").is_file());
    assert!(agent.join("launch").is_file());
    assert!(b
        .root
        .join("Library/LaunchAgents/dev.paddock.guest.plist")
        .is_file());
    assert!(!b.root.join(DAEMON).exists(), "the daemon removed itself");
    assert!(!b.root.join(STAGE).exists(), "the stage removed itself");
    let calls = b.calls();
    assert!(
        calls.contains(&"chown -R 501:20 ROOT/Library/Application Support/paddock.new".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.contains(
            &"chown 0:0 ROOT/Library/LaunchAgents/dev.paddock.guest.plist.new".to_string()
        ),
        "{calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("launchctl bootstrap"))
            .collect::<Vec<_>>(),
        ["launchctl bootstrap gui/501 ROOT/Library/LaunchAgents/dev.paddock.guest.plist"]
    );
    let log = b.log_text();
    for part in [
        "start",
        "agent installed",
        "agent started in gui/501",
        "done",
    ] {
        assert!(log.contains(part), "{log}");
    }
}

#[test]
fn firstboot_leaves_a_loaded_agent_alone() {
    let b = Boot::new(true);
    std::fs::create_dir_all(b.root.join("Users/paddock")).unwrap();
    b.fake("launchctl", "exit 0");
    assert!(b.run(10).status.success());
    assert!(
        !b.calls()
            .iter()
            .any(|c| c.starts_with("launchctl bootstrap")),
        "{:?}",
        b.calls()
    );
    assert!(b
        .log_text()
        .contains("the login of paddock loaded the agent"));
}

#[test]
fn firstboot_without_a_login_still_installs_and_removes_itself() {
    let b = Boot::new(true);
    b.fake("launchctl", "exit 113");
    let out = b.run(1);
    assert!(out.status.success(), "{out:?}");
    assert!(b
        .root
        .join("Library/LaunchAgents/dev.paddock.guest.plist")
        .is_file());
    assert!(!b.root.join(DAEMON).exists());
    assert!(
        b.log_text().contains("no login of paddock"),
        "{}",
        b.log_text()
    );
}

#[test]
fn a_failed_install_keeps_the_daemon_for_the_next_boot() {
    let b = Boot::new(false);
    b.fake("launchctl", "exit 0");
    let out = b.run(1);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(b.root.join(DAEMON).is_file(), "the daemon stays");
    assert!(b.root.join(STAGE).join("firstboot.sh").is_file());
    assert!(
        b.log_text().contains("FAILED: install.sh"),
        "{}",
        b.log_text()
    );
}

#[test]
fn firstboot_is_posix_sh() {
    assert!(FIRSTBOOT.starts_with("#!/bin/sh\n"));
    let out = Command::new("/bin/sh")
        .arg("-n")
        .arg("-c")
        .arg(FIRSTBOOT)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    for bash_only in ["[[", "function ", "local ", "$'"] {
        assert!(!FIRSTBOOT.contains(bash_only), "{bash_only}");
    }
}
