//! The exec wrapper of `protect = "uid"` with the built binary: a link to
//! paddock in the wrapper folder runs the real program with references in
//! place of the values, and sends an allowlisted one to tackd. Temporary
//! folders and fake values only.
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const VALUE: &str = "fake-value-for-the-wrapper-test";

fn exe(p: &Path, body: &str) {
    std::fs::write(p, body).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct W {
    _t: tempfile::TempDir,
    wrap: std::path::PathBuf,
    bin: std::path::PathBuf,
}

fn world() -> W {
    let t = tempfile::tempdir().unwrap();
    let wrap = t.path().join("wrap");
    let bin = t.path().join("bin");
    std::fs::create_dir(&wrap).unwrap();
    std::fs::create_dir(&bin).unwrap();
    exe(
        &bin.join("show"),
        "#!/bin/sh\nprintf \"%s\\n\" \"token=$GH_TOKEN other=$OTHER args=$* argv0=$0\"\n",
    );
    for n in ["show", "missing"] {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_paddock"), wrap.join(n)).unwrap();
    }
    W { _t: t, wrap, bin }
}

fn run(w: &W, name: &str, listed: &str) -> std::process::Output {
    let path = format!("{}:{}:/usr/bin:/bin", w.wrap.display(), w.bin.display());
    Command::new(w.wrap.join(name))
        .args(["a b", "--flag"])
        .env_clear()
        .env("HOME", w.wrap.parent().unwrap())
        .env("PATH", &path)
        .env("PADDOCK_TACK_WRAP", &w.wrap)
        .env(
            "PADDOCK_TACK_PATH",
            format!("{}:/usr/bin:/bin", w.bin.display()),
        )
        .env("PADDOCK_TACK_SECRETS", "GH_TOKEN,UNSET")
        .env("PADDOCK_TACK_LISTED", listed)
        .env("GH_TOKEN", VALUE)
        .env("OTHER", "kept")
        .output()
        .unwrap()
}

#[test]
fn a_child_of_a_protected_program_gets_the_references() {
    let w = world();
    let o = run(&w, "show", "/usr/bin/gh");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{o:?}");
    assert_eq!(
        out.trim(),
        format!(
            "token=tack://GH_TOKEN other=kept args=a b --flag argv0={}",
            w.bin.join("show").display()
        )
    );
    assert!(!String::from_utf8_lossy(&o.stderr).contains(VALUE));
}

#[test]
fn a_name_with_no_real_program_is_not_found() {
    let w = world();
    let o = run(&w, "missing", "");
    assert_eq!(o.status.code(), Some(127), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("missing: command not found"));
}

#[test]
fn an_allowlisted_child_goes_to_tackd_and_never_runs_with_the_value() {
    let w = world();
    let real = std::fs::canonicalize(w.bin.join("show")).unwrap();
    let o = run(&w, "show", &real.display().to_string());
    let out = String::from_utf8_lossy(&o.stdout);
    // This test has no tackd at the system socket (and a host's tackd
    // would refuse a program on no allowlist): either way the program does
    // not run here with the value.
    assert!(!out.contains(VALUE), "{out}");
    if !Path::new("/run/paddock-tackd/tackd.sock").exists() {
        assert_eq!(o.status.code(), Some(69), "{o:?}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("tackd does not answer"), "{err}");
    }
}

#[test]
fn paddock_outside_the_wrapper_folder_is_paddock() {
    let w = world();
    let o = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .arg("--version")
        .env_clear()
        .env("HOME", w.wrap.parent().unwrap())
        .env("PADDOCK_TACK_WRAP", &w.wrap)
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stdout).contains(env!("CARGO_PKG_VERSION")));
}
