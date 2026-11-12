//! `paddock init` writes a new paddock.toml that loads, and never writes
//! through a symlink that a sandbox left in the project.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The user's own folders stay out of tests. A fake harness `fake` runs `sh`.
fn paddock(dir: &Path, args: &[&str]) -> Output {
    let xdg = dir.parent().unwrap().join("xdg");
    std::fs::create_dir_all(xdg.join("home")).unwrap();
    let hd = xdg.join("cfg/paddock/harness.d");
    std::fs::create_dir_all(&hd).unwrap();
    std::fs::write(
        hd.join("fake.toml"),
        "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n",
    )
    .unwrap();
    let run = xdg.join("run");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .env("XDG_CONFIG_HOME", xdg.join("cfg"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_RUNTIME_DIR", run)
        .env("HOME", xdg.join("home"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .args(args)
        .arg(dir)
        .output()
        .unwrap()
}

fn init(dir: &Path) -> bool {
    paddock(dir, &["init"]).status.success()
}

/// `paddock explain` loads the project file, as every run does.
fn explain(dir: &Path, extra: &[&str]) {
    let mut args = vec!["explain", "fake"];
    args.extend_from_slice(extra);
    let out = paddock(dir, &args);
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn project() -> (paddock::view::guard::TestDir, PathBuf) {
    let t = paddock::view::guard::tempdir();
    let p = t.path().join("p");
    std::fs::create_dir_all(&p).unwrap();
    (t, p)
}

#[test]
fn init_never_follows_a_planted_link() {
    let (t, p) = project();
    let target = t.path().join("outside.toml");
    std::os::unix::fs::symlink(&target, p.join("paddock.toml")).unwrap();
    assert!(!init(&p));
    assert!(!target.exists());

    std::fs::remove_file(p.join("paddock.toml")).unwrap();
    assert!(init(&p));
    assert!(std::fs::read_to_string(p.join("paddock.toml"))
        .unwrap()
        .contains("[mounts]"));
    explain(&p, &[]);
    assert!(!init(&p), "an existing file is never replaced");
}

#[test]
fn the_file_of_init_loads_as_written_and_with_a_device_request() {
    let (_t, p) = project();
    assert!(init(&p));
    explain(&p, &[]);
    let file = p.join("paddock.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let filled = text.replace("# use = [\"mac\"]", "use = [\"mac\"]");
    assert_ne!(filled, text, "the template has a commented device request");
    std::fs::write(&file, filled).unwrap();
    // The request widens access: without a terminal, it needs approval.
    let out = paddock(&p, &["explain", "fake"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("not approved"), "{err}");
    assert!(
        !err.contains("is only read from the user configuration"),
        "{err}"
    );
    explain(&p, &["--yes"]);
}
