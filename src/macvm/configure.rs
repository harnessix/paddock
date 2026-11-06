//! The configuration of the golden guest, a step of its build: one command
//! per line, so that the tests see the list and the Mac check sees the
//! result. The guest agent runs each step and reports it as done or not.
//! The `verify` command prints the proof, and the Mac check
//! `packaging/macos/checks/macvm-golden.sh` compares it with `expect`.
//!
//! The password never appears in an argument: a root step runs under
//! `sudo -S` with the password on stdin, and `sysadminctl` takes `-` for
//! a password that it reads from stdin too. Nothing of this list goes to
//! a log with the password.

use super::golden::GUEST_USER;

/// One configuration step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// A short name for the reports: `autologin`, `proxy-https`.
    pub name: &'static str,
    /// Run with `sudo -S` (the password on stdin) instead of as the user.
    pub as_root: bool,
    /// The command, an absolute path first.
    pub argv: &'static [&'static str],
    /// Why the golden needs it.
    pub why: &'static str,
    /// The command whose output proves the step.
    pub verify: &'static [&'static str],
    /// A text that the output of `verify` must contain.
    pub expect: &'static str,
}

impl Step {
    /// The step as the agent takes it (`proto::ConfigureStep`). The
    /// password is not in it: the request carries it once.
    pub fn wire(&self) -> super::proto::ConfigureStep {
        super::proto::ConfigureStep {
            name: self.name.to_string(),
            as_root: self.as_root,
            argv: self.argv.iter().map(|s| s.to_string()).collect(),
            verify: self.verify.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Whether a result of the agent proves the step: it ran, and its
    /// verify output has the expected text.
    pub fn proven(&self, result: &super::proto::StepResult) -> bool {
        if self.name == "proxy-no-bypass" {
            return result.ok && bypass_is_loopback_only(&result.verify);
        }
        result.ok && (self.expect.is_empty() || result.verify.contains(self.expect))
    }
}

/// What `sudo -S` of macOS (sudo 1.9) prints when the password on stdin is
/// wrong: `Sorry, try again.` for each refused line, then `sudo: 1
/// incorrect password attempt` at the end of stdin. The agent runs with
/// no `LANG`, so the texts are the English ones.
pub const SUDO_REFUSED: &[&str] = &["Sorry, try again.", "incorrect password attempt"];

/// Whether the results say that the guest refused the password: a root
/// step failed with a text of [`SUDO_REFUSED`]. The setup then asks the
/// person for the password they set, instead of failing the state.
pub fn wrong_password(results: &[super::proto::StepResult]) -> bool {
    results.iter().any(|r| {
        !r.ok
            && STEPS.iter().any(|s| s.as_root && s.name == r.name)
            && SUDO_REFUSED.iter().any(|m| r.output.contains(m))
    })
}

/// The first word of a step that the agent runs itself, on its main
/// thread, instead of a command (`@agent OP ARGS`). The steps that used
/// `osascript` with System Events asked macOS for the Automation
/// permission and waited for a click that nobody makes. No step of
/// [`STEPS`] uses it now, but the agent keeps the door for a step that
/// needs its own process.
///
/// Gone from the list, and why:
///
/// - `black-desktop`: on macOS 26 and later the picture lives in the
///   store of the wallpaper agent
///   (`~/Library/Application Support/com.apple.wallpaper/Store/Index.plist`),
///   a private format with no public API that writes it. `NSWorkspace`
///   set the picture without an error on a 27.2 guest, and the next read
///   still gave `DefaultDesktop.heic`. A write of the private store would
///   tie the setup to a format that changes between releases. The guest
///   desktop never shows: the host shows only the windows of the app.
/// - `no-time-machine`: `tmutil disable` needs Full Disk Access, which
///   paddock grants to nothing. Time Machine does nothing without a
///   backup disk, and a golden never has one.
/// - `no-login-items`: a provisioned account has none, and only System
///   Events lists them.
pub const AGENT_OP: &str = "@agent";

/// How long one command of a step may run before the agent kills it and
/// its process group: a command that waits on a permission prompt fails
/// instead of hanging the configure step.
pub const STEP_TIMEOUT_SECS: u64 = 90;

/// How long the host waits for the reply of a configure request of `n`
/// steps: each step is a command and a verify at most, each at most
/// [`STEP_TIMEOUT_SECS`], plus a minute; never less than five minutes.
pub fn request_secs(n: usize) -> u64 {
    (n as u64 * 2 * STEP_TIMEOUT_SECS + 60).max(300)
}

/// Where `~/Downloads` of the guest points: the `downloads` share. The
/// offline trust makes the link, the `downloads-link` step proves it.
pub const DOWNLOADS_TARGET: &str = "/Volumes/My Shared Files/downloads";

/// The steps that the list had before and that a golden of an older
/// paddock may show in its event log. See [`AGENT_OP`] for why each one
/// went.
pub const DROPPED: [&str; 3] = ["black-desktop", "no-time-machine", "no-login-items"];

/// The network service of the stub device in the guest, as a placeholder
/// that the agent resolves at run time (`pick_service`): the service of
/// the stub device can be named `com.redhat.spice.0`, not `Ethernet`, so the name is read from
/// `networksetup -listallnetworkservices` in the guest.
pub const SERVICE: &str = "$SERVICE";
/// The guest proxy of the agent.
pub const PROXY_HOST: &str = "127.0.0.1";
pub const PROXY_PORT: &str = "3128";
/// The proxy URL for the environment of the app.
pub const PROXY_URL: &str = "http://127.0.0.1:3128";

/// The first network service of `networksetup -listallnetworkservices`:
/// the output starts with a line of explanation, a disabled service
/// starts with `*`. The guest has one service, the stub device.
pub fn pick_service(listing: &str) -> Option<String> {
    listing
        .lines()
        .skip(1)
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('*'))
        .map(str::to_string)
}

/// The argv of a step with the service placeholder replaced.
pub fn resolve_service(argv: &[String], service: &str) -> Vec<String> {
    argv.iter()
        .map(|a| {
            if a == SERVICE {
                service.to_string()
            } else {
                a.clone()
            }
        })
        .collect()
}

/// The names that go around the guest proxy, and nothing else: the
/// guest loopback. The bypass list of the system proxy and `NO_PROXY` of
/// the app hold exactly these.
pub const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
/// What `networksetup -getproxybypassdomains` prints for [`LOOPBACK`].
pub const LOOPBACK_LISTED: &str = "localhost\n127.0.0.1\n::1";

/// Whether the output of `-getproxybypassdomains` lists exactly
/// [`LOOPBACK`]: no other name goes around the proxy.
pub fn bypass_is_loopback_only(output: &str) -> bool {
    let mut got: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    got.sort_unstable();
    let mut want = LOOPBACK.to_vec();
    want.sort_unstable();
    got == want
}

/// The environment of the app (`app.launch {env}`): a program that
/// reads only the variables, such as a Go sidecar, follows the proxy
/// too. `NO_PROXY` names the guest loopback only, as the bypass list.
pub fn launch_env() -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    let no_proxy = LOOPBACK.join(",");
    for var in crate::proxy::PROXY_VARS {
        let value = if var.eq_ignore_ascii_case("NO_PROXY") {
            no_proxy.as_str()
        } else {
            PROXY_URL
        };
        env.insert(var.to_string(), value.to_string());
    }
    env
}

/// The steps, in the order the agent runs them.
pub const STEPS: &[Step] = &[
    Step {
        name: "autologin",
        as_root: true,
        argv: &[
            "/usr/sbin/sysadminctl",
            "-autologin",
            "set",
            "-userName",
            GUEST_USER,
            "-password",
            "-",
        ],
        why: "a session boots to the desktop with no login window, so the app opens without a click",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        expect: GUEST_USER,
    },
    Step {
        name: "no-sleep",
        as_root: true,
        argv: &[
            "/usr/bin/pmset",
            "-a",
            "sleep",
            "0",
            "displaysleep",
            "0",
            "disksleep",
            "0",
        ],
        why: "a sleeping guest stops answering the host; the host pauses the guest itself when it needs to",
        verify: &["/usr/bin/pmset", "-g", "custom"],
        expect: "displaysleep         0",
    },
    Step {
        name: "no-screen-saver",
        as_root: false,
        argv: &[
            "/usr/bin/defaults",
            "-currentHost",
            "write",
            "com.apple.screensaver",
            "idleTime",
            "-int",
            "0",
        ],
        why: "a screen saver in the guest would cover the forwarded windows",
        verify: &[
            "/usr/bin/defaults",
            "-currentHost",
            "read",
            "com.apple.screensaver",
            "idleTime",
        ],
        expect: "0",
    },
    Step {
        name: "no-screen-lock",
        as_root: false,
        argv: &["/usr/sbin/sysadminctl", "-screenLock", "off", "-password", "-"],
        why: "a locked guest would ask for the password in a window of its own",
        verify: &["/usr/sbin/sysadminctl", "-screenLock", "status"],
        expect: "off",
    },
    // The screen saver and lock settings that macOS reads from places
    // that do not depend on the machine's identity, as Lume and the Cirrus
    // Labs templates set them: every profile boots with a machine
    // identifier of its own, which the `-currentHost` and per-UUID files
    // of the steps above do not follow.
    Step {
        name: "no-screen-saver-any-host",
        as_root: false,
        argv: &["/usr/bin/defaults", "write", "com.apple.screensaver", "idleTime", "-int", "0"],
        why: "the screen saver of a profile's machine would start after the default idle time and lock the guest",
        verify: &["/usr/bin/defaults", "read", "com.apple.screensaver", "idleTime"],
        expect: "0",
    },
    Step {
        name: "no-saver-password",
        as_root: false,
        argv: &["/usr/bin/defaults", "write", "com.apple.screensaver", "askForPassword", "-int", "0"],
        why: "a screen saver that asks for the password locks the guest",
        verify: &["/usr/bin/defaults", "read", "com.apple.screensaver", "askForPassword"],
        expect: "0",
    },
    Step {
        name: "no-saver-password-delay",
        as_root: false,
        argv: &["/usr/bin/defaults", "write", "com.apple.screensaver", "askForPasswordDelay", "-int", "0"],
        why: "the delay before the password of the screen saver",
        verify: &["/usr/bin/defaults", "read", "com.apple.screensaver", "askForPasswordDelay"],
        expect: "0",
    },
    Step {
        name: "no-login-window-saver",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.screensaver",
            "loginWindowIdleTime",
            "-int",
            "0",
        ],
        why: "the screen saver of the login window",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.screensaver",
            "loginWindowIdleTime",
        ],
        expect: "0",
    },
    Step {
        name: "no-auto-logout",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/.GlobalPreferences",
            "com.apple.autologout.AutoLogOutDelay",
            "-int",
            "0",
        ],
        why: "a log out after inactivity would end the guest's session",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/.GlobalPreferences",
            "com.apple.autologout.AutoLogOutDelay",
        ],
        expect: "0",
    },
    Step {
        name: "no-software-update-check",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.SoftwareUpdate",
            "AutomaticCheckEnabled",
            "-bool",
            "false",
        ],
        why: "the guest never runs Software Update; paddock macvm update rebuilds the golden instead",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.SoftwareUpdate",
            "AutomaticCheckEnabled",
        ],
        expect: "0",
    },
    Step {
        name: "no-software-update-download",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.SoftwareUpdate",
            "AutomaticDownload",
            "-bool",
            "false",
        ],
        why: "a background download would fill the system disk of every profile",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.SoftwareUpdate",
            "AutomaticDownload",
        ],
        expect: "0",
    },
    Step {
        name: "no-app-store-update",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.commerce",
            "AutoUpdate",
            "-bool",
            "false",
        ],
        why: "the apps of a profile are copies from the host; the host updates them",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.commerce",
            "AutoUpdate",
        ],
        expect: "0",
    },
    Step {
        name: "no-spotlight",
        as_root: true,
        // Not `-a`: a volume that cannot index (a shared folder) makes it
        // exit 1 although every real volume is off.
        argv: &["/usr/bin/mdutil", "-i", "off", "/", "/System/Volumes/Data"],
        why: "indexing would read every file of the data disk and the shares at each boot",
        verify: &["/usr/bin/mdutil", "-s", "/"],
        expect: "Indexing disabled",
    },
    Step {
        name: "proxy-http",
        as_root: true,
        argv: &[
            "/usr/sbin/networksetup",
            "-setwebproxy",
            SERVICE,
            PROXY_HOST,
            PROXY_PORT,
        ],
        why: "the agent's proxy on the guest loopback is the only way out; an app that ignores it has no network",
        verify: &["/usr/sbin/networksetup", "-getwebproxy", SERVICE],
        expect: "Server: 127.0.0.1",
    },
    Step {
        name: "proxy-https",
        as_root: true,
        argv: &[
            "/usr/sbin/networksetup",
            "-setsecurewebproxy",
            SERVICE,
            PROXY_HOST,
            PROXY_PORT,
        ],
        why: "HTTPS goes through the same proxy, as CONNECT",
        verify: &["/usr/sbin/networksetup", "-getsecurewebproxy", SERVICE],
        expect: "Server: 127.0.0.1",
    },
    Step {
        name: "proxy-no-bypass",
        as_root: true,
        // The guest loopback only: the callback server of a login and
        // the helpers of an app on 127.0.0.1 stay in the guest. The host
        // proxy refuses loopback targets, so through the proxy they fail.
        argv: &[
            "/usr/sbin/networksetup",
            "-setproxybypassdomains",
            SERVICE,
            LOOPBACK[0],
            LOOPBACK[1],
            LOOPBACK[2],
        ],
        why: "only the guest loopback goes around the proxy, so a login callback to localhost reaches the app; every host name goes through the proxy",
        verify: &["/usr/sbin/networksetup", "-getproxybypassdomains", SERVICE],
        expect: LOOPBACK_LISTED,
    },
    Step {
        name: "dock-hidden",
        as_root: false,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "com.apple.dock",
            "autohide",
            "-bool",
            "true",
        ],
        why: "the guest Dock would take screen room from the forwarded windows",
        verify: &["/usr/bin/defaults", "read", "com.apple.dock", "autohide"],
        expect: "1",
    },
    Step {
        name: "menu-bar-hidden",
        as_root: false,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "NSGlobalDomain",
            "_HIHideMenuBar",
            "-bool",
            "true",
        ],
        why: "the host shows its own menu bar for the app",
        verify: &["/usr/bin/defaults", "read", "NSGlobalDomain", "_HIHideMenuBar"],
        expect: "1",
    },
    Step {
        name: "no-banners",
        as_root: false,
        argv: &[
            "/usr/bin/defaults",
            "-currentHost",
            "write",
            "com.apple.notificationcenterui",
            "doNotDisturb",
            "-bool",
            "true",
        ],
        why: "a banner of the guest would be a window of nobody on the host desktop",
        verify: &[
            "/usr/bin/defaults",
            "-currentHost",
            "read",
            "com.apple.notificationcenterui",
            "doNotDisturb",
        ],
        expect: "1",
    },
    Step {
        name: "downloads-link",
        as_root: false,
        // The offline trust makes the link (`offline_link.rs`): a command
        // in the guest that removes ~/Downloads waits on a Files and
        // Folders prompt. Here only `lstat` and `readlink` of the entry in
        // the home run, which macOS does not guard.
        argv: &["/bin/sh", "-c", "test -L \"$HOME/Downloads\""],
        why: "every download of the app lands in profile/downloads/ on the host, and the agent checks the link at each boot",
        verify: &["/bin/sh", "-c", "readlink \"$HOME/Downloads\""],
        expect: DOWNLOADS_TARGET,
    },
];

/// The folders where the commands of the steps live.
pub const COMMAND_DIRS: [&str; 4] = ["/usr/bin/", "/usr/sbin/", "/bin/", "/sbin/"];

/// Whether a command is an absolute path under one of `COMMAND_DIRS`.
pub fn is_system_command(cmd: &str) -> bool {
    COMMAND_DIRS.iter().any(|d| {
        cmd.strip_prefix(d)
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
    })
}

/// The one step of the shutdown of the guest: `sudo shutdown`, the
/// password on stdin as for every root step. The agent's own shutdown
/// asks System Events, which needs the Automation permission that nobody
/// grants in the setup.
pub fn shutdown_step() -> super::proto::ConfigureStep {
    super::proto::ConfigureStep {
        name: "shutdown".into(),
        as_root: true,
        argv: vec!["/sbin/shutdown".into(), "-h".into(), "now".into()],
        verify: Vec::new(),
    }
}

/// Whether a step command is a system command or a step of the agent.
pub fn is_step_command(cmd: &str) -> bool {
    cmd == AGENT_OP || is_system_command(cmd)
}

/// One line per result for the terminal and the event log, and the names
/// of the steps that the results do not prove, in the order of [`STEPS`].
/// A step that the agent did not report is a failure too.
pub fn report(results: &[super::proto::StepResult]) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut failed = Vec::new();
    for s in STEPS {
        match results.iter().find(|r| r.name == s.name) {
            Some(r) => {
                let proven = s.proven(r);
                lines.push(super::text::configure_step_line(r, proven, s.expect));
                if !proven {
                    failed.push(s.name.to_string());
                }
            }
            None => {
                lines.push(super::text::configure_step_missing(s.name));
                failed.push(s.name.to_string());
            }
        }
    }
    (lines, failed)
}

/// The step by name.
pub fn step(name: &str) -> Option<&'static Step> {
    STEPS.iter().find(|s| s.name == name)
}

/// The names of the steps, in order.
pub fn names() -> Vec<String> {
    STEPS.iter().map(|s| s.name.to_string()).collect()
}

#[cfg(test)]
#[path = "configure_tests.rs"]
mod tests;
