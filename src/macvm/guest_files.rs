//! The files that the seed puts into the golden: the `launch` script of the
//! agent and its launchd plist. They are part of the binary, so that the
//! helper has them on any Mac.

/// `/Library/Application Support/paddock/launch`: runs
/// `current/paddock-guest`, and falls back to `prev/` after three fast
/// failures in a row.
pub const LAUNCH: &str = include_str!("../../packaging/macos/guest/launch");
/// `/Library/LaunchAgents/dev.paddock.guest.plist`: RunAtLoad, KeepAlive,
/// ThrottleInterval 1, the program `launch`.
pub const PLIST: &str = include_str!("../../packaging/macos/dev.paddock.guest.plist");

/// The agent folder in the guest.
pub const AGENT_DIR: &str = "Library/Application Support/paddock";
/// The plist in the guest.
pub const PLIST_PATH: &str = "Library/LaunchAgents/dev.paddock.guest.plist";
/// The name of the agent binary.
pub const BINARY: &str = "paddock-guest";
/// The file with the SHA-256 of the binary, next to it in the agent
/// folder of the host (`packaging/macos/build-guest.sh` writes it).
pub const BINARY_SHA256: &str = "paddock-guest.sha256";
/// The label of the launchd agent.
pub const LABEL: &str = "dev.paddock.guest";
/// The owner of the agent folder: the account that Setup Assistant makes
/// first.
pub const GUEST_UID: u32 = 501;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_has_the_launch_agent_keys() {
        for key in [
            "<string>dev.paddock.guest</string>",
            "<key>RunAtLoad</key>",
            "<key>KeepAlive</key>",
            "<key>ThrottleInterval</key>",
            "<integer>1</integer>",
            "<string>/Library/Application Support/paddock/launch</string>",
            "<key>StandardErrorPath</key>",
        ] {
            assert!(PLIST.contains(key), "{key}");
        }
        assert!(PLIST.starts_with("<?xml"));
    }

    #[test]
    fn launch_is_a_short_shell_script() {
        assert!(LAUNCH.starts_with("#!/bin/sh\n"));
        assert!(LAUNCH.contains("current/paddock-guest"));
        assert!(LAUNCH.contains("prev"));
        let code = LAUNCH
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
            .count();
        assert!(code <= 40, "{code} lines of code");
    }

    /// The fallback: three fast failures of `current`, then `prev` runs.
    #[test]
    fn launch_falls_back_to_prev_after_three_fast_failures() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("paddock");
        let launch = t.path().join("launch");
        std::fs::write(&launch, LAUNCH).unwrap();
        std::fs::set_permissions(&launch, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (name, body) in [
            (
                "current",
                "#!/bin/sh\necho current >> \"$PADDOCK_GUEST_DIR/ran\"\nexit 1\n",
            ),
            (
                "prev",
                "#!/bin/sh\necho prev >> \"$PADDOCK_GUEST_DIR/ran\"\nexit 0\n",
            ),
        ] {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join("paddock-guest");
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let run = || {
            std::process::Command::new("/bin/sh")
                .arg(&launch)
                .env("PADDOCK_GUEST_DIR", &dir)
                .output()
                .unwrap()
                .status
                .code()
        };
        assert_eq!(run(), Some(1));
        assert_eq!(run(), Some(1));
        assert_eq!(run(), Some(1), "the third fast failure swaps the folders");
        assert!(dir.join("current").join("paddock-guest").exists());
        assert!(!dir.join("prev").exists());
        assert_eq!(run(), Some(0), "prev runs as current now");
        let ran = std::fs::read_to_string(dir.join("ran")).unwrap();
        assert_eq!(ran, "current\ncurrent\ncurrent\nprev\n");
        assert_eq!(
            std::fs::read_to_string(dir.join(".failures"))
                .unwrap()
                .trim(),
            "0"
        );
    }

    #[test]
    fn launch_waits_without_an_agent() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let launch = t.path().join("launch");
        std::fs::write(&launch, LAUNCH).unwrap();
        std::fs::set_permissions(&launch, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg(&launch)
            .env("PADDOCK_GUEST_DIR", t.path().join("none"))
            .env("PADDOCK_GUEST_WAIT", "0")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&out.stderr).contains("no agent"));
    }
}
