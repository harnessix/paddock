//! The platform decisions of the first-run commands: setup, doctor, shims,
//! launchers, update and view.
//!
//! Each decision takes the platform as an argument, so the tests check the
//! macOS answers on Linux too. The commands pass [`Os::HOST`].

use crate::env::HostEnv;
use anyhow::{bail, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
}

impl Os {
    /// The platform of this build.
    pub const HOST: Os = if cfg!(target_os = "macos") {
        Os::Macos
    } else {
        Os::Linux
    };
}

/// A command or a setup step that does not run on every platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// `paddock launchers`: XDG desktop entries and the Omarchy menu.
    Launchers,
    /// `paddock shims install --scope omarchy-agent`: Omarchy's mise wrapper
    /// and the Hyprland hook of the agents panel.
    OmarchyScope,
    /// `paddock setup --plugin`: the Omarchy bar widget.
    BarWidget,
    /// `paddock update` replaces the binary with a release binary.
    UpdateBinary,
    /// `paddock setup` runs the package manager after a yes.
    PackageInstall,
    /// The devices of `/dev/paddock` as files (FUSE). macOS gets them
    /// over a socket instead (Seatbelt).
    Devices,
}

/// Every feature, for the doctor's list of what macOS refuses.
pub const FEATURES: &[Feature] = &[
    Feature::Launchers,
    Feature::OmarchyScope,
    Feature::BarWidget,
    Feature::UpdateBinary,
    Feature::PackageInstall,
    Feature::Devices,
];

/// The one line that refuses `f` on `os`, or None when `f` runs there.
pub fn refusal(os: Os, f: Feature) -> Option<&'static str> {
    if os == Os::Linux {
        return None;
    }
    Some(match f {
        Feature::Launchers => {
            "paddock launchers writes XDG desktop entries for Linux app menus: it is Linux only"
        }
        Feature::OmarchyScope => {
            "the omarchy-agent scope wraps the agent starts of Omarchy (mise, Hyprland): it is Linux only. On macOS use: paddock shims install"
        }
        Feature::BarWidget => "the bar widget is an Omarchy plugin: it is Linux only",
        Feature::UpdateBinary => {
            "paddock update installs Linux release binaries only. On macOS, update your clone of paddock and run: cargo install --path ."
        }
        Feature::PackageInstall => {
            "paddock setup does not install packages on macOS: run the commands yourself"
        }
        Feature::Devices => "the devices are not files in /dev/paddock on macOS (that needs FUSE): Seatbelt runs reach them with paddock-host device, and the VM does not get them yet",
    })
}

/// Whether `f` runs on `os`.
pub fn available(os: Os, f: Feature) -> bool {
    refusal(os, f).is_none()
}

/// An error with the refusal line when `f` does not run on this platform.
pub fn refuse(f: Feature) -> Result<()> {
    refuse_on(Os::HOST, f)
}

pub fn refuse_on(os: Os, f: Feature) -> Result<()> {
    match refusal(os, f) {
        Some(line) => bail!("{line}"),
        None => Ok(()),
    }
}

/// The backends that `paddock setup --backend` accepts.
pub fn backends(os: Os) -> &'static [&'static str] {
    match os {
        Os::Linux => &["auto", "bwrap", "podman", "docker", "srt"],
        Os::Macos => &["auto", "seatbelt", "podman", "docker", "srt"],
    }
}

/// The backends that `paddock doctor` checks.
pub fn doctor_backends(os: Os) -> &'static [&'static str] {
    match os {
        Os::Linux => &["bwrap", "podman", "docker", "srt"],
        Os::Macos => &["seatbelt", "podman", "docker", "srt"],
    }
}

/// What `auto` picks, for the backend step of `paddock setup`.
pub fn auto_backend_text(os: Os) -> &'static str {
    match os {
        Os::Linux => "auto picks bwrap: every host tool, and starts in milliseconds. podman or docker\n   suit projects that need system packages the host does not have.",
        Os::Macos => "auto picks seatbelt (sandbox-exec): every host tool, and starts at once. podman or docker\n   run a Linux container for projects that need one.",
    }
}

/// The shell startup files that `paddock shims install` adds its line to.
/// Linux: `~/.bashrc` and `~/.zshrc` where they exist. macOS: also a new
/// `~/.zshrc`, because zsh is the default shell there and a new Mac has none.
pub fn rc_targets(home: &Path, os: Os) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [".bashrc", ".zshrc"]
        .iter()
        .map(|f| home.join(f))
        .filter(|p| p.is_file())
        .collect();
    let zshrc = home.join(".zshrc");
    // Nothing at the path at all: not a dangling symlink, not a folder.
    if os == Os::Macos && std::fs::symlink_metadata(&zshrc).is_err() {
        out.push(zshrc);
    }
    out
}

/// Whether `paddock shims remove` deletes `rc` when nothing else is left in
/// it: only a `~/.zshrc` on macOS, which `install` can have created.
pub fn removes_empty_rc(rc: &Path, rest: &str, os: Os) -> bool {
    os == Os::Macos && rc.file_name().is_some_and(|n| n == ".zshrc") && rest.trim().is_empty()
}

// ---------------------------------------------------------------------------
// paddock doctor and paddock setup on macOS

/// A program that paddock uses on macOS.
pub struct MacDep {
    pub cmd: &'static str,
    pub required: bool,
    pub why: &'static str,
    /// The command that installs it.
    pub install: &'static str,
}

/// The programs on PATH that paddock uses on macOS. `sandbox-exec` and the
/// command line tools have their own checks in [`MacProbe`].
pub const MAC_DEPS: &[MacDep] = &[
    MacDep {
        cmd: "git",
        required: true,
        why: "project and worktree detection",
        install: "xcode-select --install",
    },
    MacDep {
        cmd: "tmux",
        required: false,
        why: "detached runs (paddock run --detach) and paddock view",
        install: "brew install tmux",
    },
    MacDep {
        cmd: "podman",
        required: false,
        why: "the container backend (a Linux machine)",
        install: "brew install podman",
    },
    MacDep {
        cmd: "docker",
        required: false,
        why: "the container backend (Docker Desktop)",
        install: "brew install --cask docker",
    },
    MacDep {
        cmd: "srt",
        required: false,
        why: "network mode 'allowlist' (sandbox-runtime from npm, also needs ripgrep)",
        install: "npm install -g @anthropic-ai/sandbox-runtime",
    },
    MacDep {
        cmd: "rg",
        required: false,
        why: "the srt backend",
        install: "brew install ripgrep",
    },
    MacDep {
        cmd: "mise",
        required: false,
        why: "per-project toolchains from mise.toml",
        install: "brew install mise",
    },
];

/// The macOS release that paddock is tested on.
pub const TESTED_MACOS: u32 = 27;

/// What `paddock doctor` reads from the Mac itself.
#[derive(Debug, Clone, Default)]
pub struct MacProbe {
    /// `sw_vers -productVersion`, for example `27.0.1`.
    pub version: Option<String>,
    /// Whether `/usr/bin/sandbox-exec` exists.
    pub sandbox_exec: bool,
    /// `xcode-select -p`: the folder of the command line tools or of Xcode.
    pub developer_dir: Option<PathBuf>,
}

impl MacProbe {
    /// Read the Mac. Runs `sw_vers` and `xcode-select -p`, which only print.
    /// It never runs `cc` or `git`: on a Mac without the command line tools,
    /// those open a dialog that offers to install them.
    pub fn host() -> MacProbe {
        let out = |exe: &str, args: &[&str]| {
            let o = Command::new(exe)
                .args(args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (o.status.success() && !s.is_empty()).then_some(s)
        };
        MacProbe {
            version: out("/usr/bin/sw_vers", &["-productVersion"]),
            sandbox_exec: Path::new(crate::backend::seatbelt::SANDBOX_EXEC).is_file(),
            developer_dir: out("/usr/bin/xcode-select", &["-p"])
                .map(PathBuf::from)
                .filter(|d| d.is_dir()),
        }
    }
}

/// The major version of a macOS version string such as `27.0.1`.
pub fn macos_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// The doctor's macOS lines, and whether every required part is there.
pub fn mac_system_lines(p: &MacProbe) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut ok = true;
    lines.push(match p.version.as_deref() {
        Some(v) if macos_major(v).is_some_and(|m| m >= TESTED_MACOS) => {
            format!("  ok       macOS {v}")
        }
        Some(v) => format!(
            "  note     macOS {v}: paddock is tested on macOS {TESTED_MACOS}. Older releases can work"
        ),
        None => "  unknown  sw_vers did not tell the macOS version".to_string(),
    });
    if p.sandbox_exec {
        lines.push(format!(
            "  ok       {:<22} the seatbelt backend",
            "sandbox-exec"
        ));
    } else {
        ok = false;
        lines.push(format!(
            "  MISSING  {:<22} {} is part of macOS: without it, paddock run has no sandbox",
            "sandbox-exec",
            crate::backend::seatbelt::SANDBOX_EXEC
        ));
    }
    match &p.developer_dir {
        Some(d) => lines.push(format!(
            "  ok       {:<22} {} (cc builds the pty library)",
            "command line tools",
            d.display()
        )),
        None => {
            ok = false;
            lines.push(format!(
                "  MISSING  {:<22} cc builds the pty library: xcode-select --install",
                "command line tools"
            ));
        }
    }
    (lines, ok)
}

/// The doctor's lines for [`MAC_DEPS`], the install commands of the missing
/// ones (required, optional), and whether every required program is there.
pub fn mac_dep_lines(
    which: impl Fn(&str) -> Option<PathBuf>,
) -> (Vec<String>, Vec<&'static str>, Vec<&'static str>) {
    let (mut lines, mut req, mut opt) = (Vec::new(), Vec::new(), Vec::new());
    for d in MAC_DEPS {
        let found = which(d.cmd).is_some();
        let mark = match (found, d.required) {
            (true, _) => "ok      ",
            (false, true) => "MISSING ",
            (false, false) => "absent  ",
        };
        lines.push(format!("  {mark} {:<22} {}", d.cmd, d.why));
        if !found {
            let list = if d.required { &mut req } else { &mut opt };
            if !list.contains(&d.install) {
                list.push(d.install);
            }
        }
    }
    (lines, req, opt)
}

/// The dependency part of `paddock doctor` on macOS. Returns whether every
/// required part is there.
pub fn mac_doctor_deps(env: &HostEnv, probe: &MacProbe) -> bool {
    println!("macOS:");
    let (sys, sys_ok) = mac_system_lines(probe);
    for l in sys {
        println!("{l}");
    }
    println!("\nDependencies:");
    let (lines, req, opt) = mac_dep_lines(|c| env.which(c));
    for l in lines {
        println!("{l}");
    }
    if !req.is_empty() {
        println!("\nInstall the required programs:\n  {}", req.join("\n  "));
    }
    if !opt.is_empty() {
        println!("\nOptional programs:\n  {}", opt.join("\n  "));
    }
    println!("\nVM backend:");
    for l in crate::vm::doctor_lines(env) {
        println!("{l}");
    }
    println!("\nNotifications:");
    println!(
        "{}",
        mac_notifier_line(
            &mac_notifier_here(env),
            &crate::eventstream::Bus::of(env).notice_followers()
        )
    );
    println!();
    let view = crate::doctor::macvm::View::read(env, probe.version.as_deref());
    for l in crate::doctor::macvm::lines(&view) {
        println!("{l}");
    }
    sys_ok && req.is_empty()
}

/// The doctor's line for `f` when `os` refuses it.
pub fn refused_line(os: Os, f: Feature) -> Option<String> {
    refusal(os, f).map(|l| format!("  refused  {l}"))
}

/// The doctor's list of what `os` refuses, one line each.
pub fn refused_lines(os: Os) -> Vec<String> {
    FEATURES
        .iter()
        .filter_map(|f| refused_line(os, *f))
        .collect()
}

// ---------------------------------------------------------------------------
// Notifications and terminal windows on macOS

/// Plain text for an AppleScript argument: no control characters, a sane
/// length, and no leading "-" that osascript could read as an option.
fn applescript_arg(s: &str, max: usize) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    t.trim().trim_start_matches('-').trim_start().to_string()
}

/// The arguments of `osascript` that show a notification. The title and the
/// body are arguments of the script's run handler: neither the shell nor
/// AppleScript parses them as code.
pub fn notification_argv(title: &str, body: &str) -> Vec<String> {
    let mut title = applescript_arg(title, 200);
    if title.is_empty() {
        title = "paddock".into();
    }
    [
        "-e",
        "on run argv",
        "-e",
        "display notification (item 2 of argv) with title (item 1 of argv)",
        "-e",
        "end run",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([title, applescript_arg(body, 1000)])
    .collect()
}

pub const OSASCRIPT: &str = "/usr/bin/osascript";

/// Where Homebrew puts `terminal-notifier`, on Apple silicon and on Intel.
/// paddock looks only here, not on PATH: a PATH entry can be a folder that
/// a sandbox writes.
pub const TERMINAL_NOTIFIER_PATHS: &[&str] = &[
    "/opt/homebrew/bin/terminal-notifier",
    "/usr/local/bin/terminal-notifier",
];

/// The bundle of paddock for Mac and its bundle id.
pub const LAUNCHER_APP: &str = "paddock for Mac.app";
pub const LAUNCHER_BUNDLE_ID: &str = "dev.paddock.mac";

/// How paddock posts a notification on macOS when paddock for Mac does
/// not show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacNotifier {
    /// `terminal-notifier`. With `sender`, the bundle id of an installed
    /// paddock for Mac, macOS shows paddock's name and icon.
    TerminalNotifier {
        path: PathBuf,
        sender: Option<&'static str>,
    },
    /// The last way: macOS shows the notification as from Script Editor.
    Osascript,
}

/// The folders where paddock for Mac can be installed.
pub fn launcher_app_paths(home: &Path) -> [PathBuf; 2] {
    [
        Path::new("/Applications").join(LAUNCHER_APP),
        home.join("Applications").join(LAUNCHER_APP),
    ]
}

/// The way to post. `is_there` says whether a path is there: a program
/// that runs, or the folder of an app.
pub fn mac_notifier(home: &Path, is_there: impl Fn(&Path) -> bool) -> MacNotifier {
    let Some(path) = TERMINAL_NOTIFIER_PATHS
        .iter()
        .map(Path::new)
        .find(|p| is_there(p))
    else {
        return MacNotifier::Osascript;
    };
    let sender = launcher_app_paths(home)
        .iter()
        .any(|p| is_there(p))
        .then_some(LAUNCHER_BUNDLE_ID);
    MacNotifier::TerminalNotifier {
        path: path.to_path_buf(),
        sender,
    }
}

/// The way to post on this Mac.
pub fn mac_notifier_here(env: &HostEnv) -> MacNotifier {
    use std::os::unix::fs::PermissionsExt;
    mac_notifier(&env.home, |p| match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => p.extension().is_some_and(|e| e == "app"),
        Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    })
}

/// The program and the arguments that post `title` and `body` with `n`.
/// The text is plain arguments: no shell reads them, and a leading "-"
/// is gone, so neither program reads the text as an option.
pub fn mac_notify_command(n: &MacNotifier, title: &str, body: &str) -> (PathBuf, Vec<String>) {
    match n {
        MacNotifier::Osascript => (PathBuf::from(OSASCRIPT), notification_argv(title, body)),
        MacNotifier::TerminalNotifier { path, sender } => {
            let mut title = applescript_arg(title, 200);
            if title.is_empty() {
                title = "paddock".into();
            }
            // Without -message, terminal-notifier reads stdin.
            let mut message = applescript_arg(body, 1000);
            if message.is_empty() {
                message = title.clone();
            }
            let mut args = vec!["-title".to_string(), title, "-message".into(), message];
            if let Some(id) = sender {
                args.extend(["-sender".to_string(), id.to_string()]);
            }
            (path.clone(), args)
        }
    }
}

/// The doctor's line about notifications on macOS. `launcher` holds the
/// pids of paddock for Mac that follow the events with `--notify`.
pub fn mac_notifier_line(n: &MacNotifier, launcher: &[u32]) -> String {
    const NAME: &str = "notifications";
    if let Some(pid) = launcher.first() {
        return format!("  ok       {NAME:<22} paddock for Mac shows them (pid {pid})");
    }
    match n {
        MacNotifier::TerminalNotifier {
            path,
            sender: Some(_),
        } => format!(
            "  ok       {NAME:<22} {} with the name and icon of paddock for Mac",
            path.display()
        ),
        MacNotifier::TerminalNotifier { path, sender: None } => format!(
            "  ok       {NAME:<22} {}. Install paddock for Mac to show its name and icon",
            path.display()
        ),
        MacNotifier::Osascript => format!(
            "  fallback {NAME:<22} {OSASCRIPT}: macOS shows them as from Script Editor. \
             Run paddock for Mac, or: brew install terminal-notifier"
        ),
    }
}

/// How long a notification program may run before paddock stops it.
const NOTIFY_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Show a notification on macOS and return at once, the best way that
/// [`mac_notifier_here`] finds.
pub fn mac_notify(env: &HostEnv, title: &str, body: &str) -> Result<()> {
    let (program, args) = mac_notify_command(&mac_notifier_here(env), title, body);
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let end = std::time::Instant::now() + NOTIFY_WAIT;
        while std::time::Instant::now() < end {
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = child.kill();
        let _ = child.wait();
    });
    Ok(())
}

/// The start of the command that opens a new terminal window, before the
/// program and its arguments. macOS without `$TERMINAL` opens Terminal: the
/// program and its arguments go to the run handler, which quotes each of them
/// with `quoted form of` before Terminal's shell reads the line.
pub fn terminal_command(
    xdg_terminal_exec: Option<&Path>,
    terminal: Option<&str>,
    os: Os,
) -> Result<Vec<OsString>> {
    Ok(match (xdg_terminal_exec, terminal, os) {
        (Some(x), _, _) => vec![x.into()],
        (None, Some(t), _) => vec![t.into(), "-e".into()],
        (None, None, Os::Macos) => {
            let mut v: Vec<OsString> = vec![OSASCRIPT.into()];
            for line in [
                "on run argv",
                "set cmd to \"exec\"",
                "repeat with a in argv",
                "set cmd to cmd & \" \" & quoted form of (a as text)",
                "end repeat",
                "tell application \"Terminal\"",
                "activate",
                "do script cmd",
                "end tell",
                "end run",
            ] {
                v.extend(["-e".into(), line.into()]);
            }
            v
        }
        (None, None, Os::Linux) => {
            bail!("no terminal to open: install xdg-terminal-exec or set $TERMINAL")
        }
    })
}

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "onboard/macos_tests.rs"]
mod macos_tests;
