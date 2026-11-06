//! The socket path rules of both systems, on Linux too.

use super::*;

/// The default `$TMPDIR` of a macOS user is this long; a run folder under
/// it never fits a socket address, which is why macOS uses `short_dir`.
const MAC_TMPDIR: &str = "/var/folders/zz/zyxvpxvq6csfxvn_n0000000000000/T/";

#[test]
fn the_limits_and_the_check() {
    assert_eq!(MACOS_MAX, 103);
    assert_eq!(LINUX_MAX, 107);
    let p = |n: usize| PathBuf::from(format!("/{}", "a".repeat(n - 1)));
    assert!(fits(&p(102), MACOS_MAX));
    assert!(!fits(&p(103), MACOS_MAX));
    assert!(fits(&p(106), LINUX_MAX));
    assert!(!fits(&p(107), LINUX_MAX));
    let e = check(&p(200)).unwrap_err().to_string();
    assert!(e.contains("200 bytes") && e.contains("byte limit"), "{e}");
    assert!(check(&p(20)).is_ok());
}

#[test]
fn the_short_folders_fit_every_name_for_any_run_and_id() {
    let top = Path::new("/tmp/paddock-4294967295");
    let deep = PathBuf::from(MAC_TMPDIR).join("paddock-tests/t-abcdef/run/paddock");
    assert!(!fits(
        &deep.join("devices/mac/check-1793849743-24736/dev.sock"),
        MACOS_MAX
    ));
    let names = [
        "dev.sock",
        "kv.sock",
        "host.sock",
        "pty.sock",
        "proxy.sock",
        "api-proxy.sock",
        "vm-proxy.sock",
        "ssh-agent.sock",
        "credentials.sock",
        "gatewayd.sock",
        "view.sock",
    ];
    let long_id = "x".repeat(200);
    for id in [
        "",
        "check-1793849743-24736",
        "claude-1793849743-24736-host",
        &long_id,
        "a/b",
        "..",
    ] {
        let dir = short_dir(top, &deep, id, MACOS_MAX);
        assert!(dir.starts_with(top), "{}", dir.display());
        for n in names {
            let p = dir.join(n);
            assert!(
                fits(&p, MACOS_MAX),
                "{} is {} bytes",
                p.display(),
                p.as_os_str().len()
            );
        }
        assert_eq!(dir, short_dir(top, &deep, id, MACOS_MAX), "stable");
    }
    // A short id stays readable; a long or unsafe one is a hash, one level deep.
    let base = short_dir(top, &deep, "", MACOS_MAX);
    assert_eq!(short_dir(top, &deep, "sb1", MACOS_MAX), base.join("sb1"));
    for id in [long_id.as_str(), "a/b", ".."] {
        let d = short_dir(top, &deep, id, MACOS_MAX);
        assert_eq!(d.parent(), Some(base.as_path()), "{id}");
        assert_eq!(d.file_name().unwrap().len(), 16, "{id}");
    }
    // Two runs (a test home and the real one) never share a folder, and
    // two ids never share one either.
    let other = PathBuf::from("/Users/me/.local/state/paddock/run");
    assert_ne!(short_dir(top, &other, "", MACOS_MAX), base);
    assert_ne!(
        short_dir(top, &deep, &long_id, MACOS_MAX),
        short_dir(top, &deep, &"y".repeat(200), MACOS_MAX)
    );
}
