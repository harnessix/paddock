//! The seed folder and `install.sh`, run against a test root with a fake
//! `chown` and `launchctl` that write their arguments to a log.

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

fn agent_file(t: &Path) -> PathBuf {
    let a = t.join("paddock-guest-build");
    std::fs::write(&a, "#!/bin/sh\nexit 0\n").unwrap();
    a
}

#[test]
fn the_command_runs_install_sh_from_the_share_with_sudo() {
    assert_eq!(
        command(),
        "sudo /bin/sh \"/Volumes/My Shared Files/paddock-seed/install.sh\""
    );
    assert_eq!(plist_file(), "dev.paddock.guest.plist");
}

#[test]
fn prepare_writes_the_folder_with_modes_and_ready_sees_it() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("seed");
    assert!(!ready(&dir));
    prepare(&dir, Some(&agent_file(t.path()))).unwrap();
    assert!(ready(&dir));
    assert!(!layout::partial(&dir).exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("install.sh")).unwrap(),
        INSTALL
    );
    assert_eq!(mode(&dir.join("install.sh")), 0o644);
    assert_eq!(mode(&dir.join("dev.paddock.guest.plist")), 0o644);
    assert_eq!(mode(&dir.join("agent/launch")), 0o755);
    assert_eq!(mode(&dir.join("agent/current/paddock-guest")), 0o755);
    assert_eq!(mode(&dir.join("agent/current")), 0o755);
}

#[test]
fn prepare_again_replaces_a_stale_folder_and_a_partial() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("seed");
    prepare(&dir, Some(&agent_file(t.path()))).unwrap();
    std::fs::write(dir.join("stale"), "x").unwrap();
    std::fs::write(dir.join("install.sh"), "old").unwrap();
    assert!(!ready(&dir), "an older install.sh is not ready");
    std::fs::create_dir_all(layout::partial(&dir).join("junk")).unwrap();
    prepare(&dir, None).unwrap();
    assert!(ready(&dir));
    assert!(!dir.join("stale").exists());
    assert!(!dir.join("agent/current/paddock-guest").exists());
    assert!(!layout::partial(&dir).exists());
    remove(&dir).unwrap();
    assert!(!dir.exists());
    remove(&dir).unwrap();
}

#[test]
fn a_missing_agent_file_fails_and_leaves_only_a_partial() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("seed");
    let e = prepare(&dir, Some(&t.path().join("none"))).unwrap_err();
    assert!(format!("{e:#}").contains("none"), "{e:#}");
    assert!(!dir.exists());
    assert!(layout::partial(&dir).exists(), "the next setup removes it");
}

#[test]
fn install_sh_is_posix_sh_and_names_its_owners_and_modes() {
    assert!(INSTALL.starts_with("#!/bin/sh\n"));
    for part in [
        "set -eu",
        "uid=501",
        "gid=20",
        "chown -R \"$uid:$gid\"",
        "chown 0:0",
        "chmod 0644 \"$plist.new\"",
        "launchctl bootstrap \"gui/$uid\" \"$plist\"",
        "plutil -lint",
        "PATH=/usr/bin:/bin:/usr/sbin:/sbin",
    ] {
        assert!(INSTALL.contains(part), "install.sh lacks {part}");
    }
    let out = Command::new("/bin/sh")
        .arg("-n")
        .arg("-c")
        .arg(INSTALL)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

/// A test root with the seed folder, a fake `chown` and a fake
/// `launchctl` first on PATH.
struct Guest {
    t: tempfile::TempDir,
    seed: PathBuf,
    root: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Guest {
    fn new(agent: bool) -> Self {
        let t = tempfile::tempdir().unwrap();
        let seed = t.path().join("My Shared Files/paddock-seed");
        std::fs::create_dir_all(seed.parent().unwrap()).unwrap();
        let a = agent.then(|| agent_file(t.path()));
        prepare(&seed, a.as_deref()).unwrap();
        let root = t.path().join("guest root");
        std::fs::create_dir_all(&root).unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = t.path().join("calls.log");
        let g = Guest {
            t,
            seed,
            root,
            bin,
            log,
        };
        g.fake("chown", "exit 0");
        g.fake("launchctl", "exit 0");
        g
    }

    /// A fake command that logs its arguments, then runs `then`.
    fn fake(&self, name: &str, then: &str) {
        let p = self.bin.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\necho \"{name} $*\" >> \"{}\"\n{then}\n",
                self.log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(&self) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new("/bin/sh")
            .arg(self.seed.join("install.sh"))
            .env("PADDOCK_SEED_ROOT", &self.root)
            .env("PADDOCK_SEED_SLEEP", "0")
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

    fn dir(&self) -> PathBuf {
        self.root.join("Library/Application Support/paddock")
    }

    fn plist(&self) -> PathBuf {
        self.root
            .join("Library/LaunchAgents/dev.paddock.guest.plist")
    }
}

#[test]
fn install_sh_installs_with_owners_modes_and_starts_the_agent() {
    let g = Guest::new(true);
    let out = g.run();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("paddock seed installed"));
    let dir = g.dir();
    assert_eq!(mode(&dir), 0o755);
    assert_eq!(mode(&dir.join("current")), 0o755);
    assert_eq!(mode(&dir.join("launch")), 0o755);
    assert_eq!(mode(&dir.join("current/paddock-guest")), 0o755);
    assert_eq!(
        std::fs::read_to_string(dir.join("launch")).unwrap(),
        guest_files::LAUNCH
    );
    assert_eq!(mode(&g.plist()), 0o644);
    assert_eq!(
        std::fs::read_to_string(g.plist()).unwrap(),
        guest_files::PLIST
    );
    assert_eq!(
        g.calls(),
        vec![
            "chown -R 501:20 ROOT/Library/Application Support/paddock.new",
            "chown 0:0 ROOT/Library/LaunchAgents/dev.paddock.guest.plist.new",
            "launchctl bootout gui/501/dev.paddock.guest",
            "launchctl enable gui/501/dev.paddock.guest",
            "launchctl bootstrap gui/501 ROOT/Library/LaunchAgents/dev.paddock.guest.plist",
        ]
    );
    drop(g.t);
}

#[test]
fn install_sh_twice_gives_the_same_result_and_no_leftovers() {
    let g = Guest::new(true);
    assert!(g.run().status.success());
    // An agent that ran in between left its counter and a prev/.
    std::fs::write(g.dir().join(".failures"), "2").unwrap();
    let first = g.calls();
    std::fs::remove_file(&g.log).unwrap();
    let out = g.run();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(g.calls(), first, "the second run does the same calls");
    assert!(!g.dir().join(".failures").exists());
    assert!(g.dir().join("current/paddock-guest").is_file());
    let lib = g.root.join("Library");
    let mut names: Vec<String> = ["Application Support", "LaunchAgents"]
        .iter()
        .flat_map(|d| std::fs::read_dir(lib.join(d)).unwrap())
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["dev.paddock.guest.plist", "paddock"]);
}

#[test]
fn install_sh_without_an_agent_installs_launch_and_says_so() {
    let g = Guest::new(false);
    let out = g.run();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no current/paddock-guest"));
    assert!(g.dir().join("launch").is_file());
    assert!(g.dir().join("current").is_dir());
}

#[test]
fn install_sh_fails_loudly_when_bootstrap_keeps_failing() {
    let g = Guest::new(true);
    g.fake(
        "launchctl",
        "[ \"$1\" = bootstrap ] && { echo 'Bootstrap failed: 5: Input/output error' >&2; exit 5; }; exit 0",
    );
    let out = g.run();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("paddock seed: FAILED"), "{err}");
    assert!(err.contains("failed 5 times"), "{err}");
    assert!(err.contains("log out"), "{err}");
    let tries = g
        .calls()
        .iter()
        .filter(|c| c.starts_with("launchctl bootstrap"))
        .count();
    assert_eq!(tries, 5);
    assert!(g.plist().is_file(), "the files stay for the next login");
}

#[test]
fn install_sh_fails_loudly_when_chown_fails() {
    let g = Guest::new(true);
    g.fake("chown", "echo 'chown: Operation not permitted' >&2; exit 1");
    let out = g.run();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("FAILED at \"copy the agent folder\""), "{err}");
    assert!(!g.dir().exists(), "nothing half done is in place");
    assert!(!g.plist().exists());
}

#[test]
fn install_sh_refuses_a_seed_folder_without_its_files() {
    let g = Guest::new(true);
    std::fs::remove_file(g.seed.join("agent/launch")).unwrap();
    let out = g.run();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has no agent/launch"), "{err}");
    assert!(g.calls().is_empty());
}

#[test]
fn install_sh_without_root_asks_for_sudo() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: the test runs as root");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let seed = t.path().join("paddock-seed");
    prepare(&seed, None).unwrap();
    let out = Command::new("/bin/sh")
        .arg(seed.join("install.sh"))
        .env_remove("PADDOCK_SEED_ROOT")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("run it with sudo"), "{err}");
}
