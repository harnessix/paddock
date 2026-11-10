//! A clean start of the commands that start a sandbox, Linux only.
//!
//! A process with `no_new_privs` (`NoNewPrivs: 1` in /proc/self/status)
//! passes the flag to every child. Some desktop apps set it for the shells
//! they run, so a `paddock run` from such a shell starts pasta with it.
//! The pasta child then has only the capabilities that pasta kept, and the
//! user namespace of bwrap inside the one of pasta cannot get its uid map:
//! `bwrap: setting up uid map: Operation not permitted`.
//!
//! The flag comes from the caller, not from a policy of the user. So such a
//! start runs itself again as a transient unit of the user's own systemd
//! manager (`systemd-run --user`), which starts it without the flag, with
//! the same arguments, environment, folder and terminal. Inside a sandbox
//! (`PADDOCK=1`) nothing changes: the flag is part of the sandbox there,
//! and the user manager is out of reach anyway.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Set on the clean start, so it never starts itself again.
pub const AGAIN_VAR: &str = "PADDOCK_CLEAN_START";

/// True when the status text says `NoNewPrivs: 1`.
pub fn no_new_privs(status: &str) -> bool {
    status
        .lines()
        .find_map(|l| l.strip_prefix("NoNewPrivs:"))
        .is_some_and(|v| v.trim() == "1")
}

/// The commands that start a sandbox: `run`, `shell`, `wrap` and
/// `app run`. Everything else runs as it is.
pub fn starts_sandbox(args: &[OsString]) -> bool {
    let word = |i: usize| args.get(i).map(|a| a.as_bytes());
    // The first word that is not an option is the command.
    let first = args.iter().position(|a| !a.as_bytes().starts_with(b"-"));
    match first.and_then(word) {
        Some(b"run" | b"shell" | b"wrap") => true,
        Some(b"app") => first.and_then(|i| word(i + 1)) == Some(b"run"),
        _ => false,
    }
}

/// The `systemd-run` arguments of the clean start of `exe ARGS`.
pub fn unit_args(
    exe: &Path,
    args: &[OsString],
    vars: &[(OsString, OsString)],
    terminal: bool,
) -> Vec<OsString> {
    let mut out: Vec<OsString> = ["--user", "--quiet", "--collect", "--wait", "--same-dir"]
        .iter()
        .map(OsString::from)
        .collect();
    out.push(if terminal { "--pty" } else { "--pipe" }.into());
    for (k, v) in vars {
        // A name or value that systemd cannot carry stays behind: a name
        // is letters, digits and `_`, not first a digit.
        let name = k.as_bytes();
        let plain = !name.is_empty()
            && !name[0].is_ascii_digit()
            && name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_');
        if !plain || v.as_bytes().contains(&b'\n') {
            continue;
        }
        let mut s = OsString::from("--setenv=");
        s.push(k);
        s.push("=");
        s.push(v);
        out.push(s);
    }
    let mut again = OsString::from("--setenv=");
    again.push(AGAIN_VAR);
    again.push("=1");
    out.push(again);
    out.push("--".into());
    out.push(exe.as_os_str().to_os_string());
    out.extend(args.iter().cloned());
    out
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Start this command again without `no_new_privs` when it starts a
/// sandbox and has the flag. The exit code of the clean start, or None
/// when this process goes on itself.
pub fn again(args: &[OsString]) -> Option<i32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    if std::env::var_os(AGAIN_VAR).is_some()
        || std::env::var_os("PADDOCK").is_some_and(|v| v == OsStr::new("1"))
        || !starts_sandbox(args)
    {
        return None;
    }
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    if !no_new_privs(&status) {
        return None;
    }
    let run = which("systemd-run")?;
    let exe = std::fs::read_link("/proc/self/exe").ok()?;
    // SAFETY: isatty on the standard descriptors.
    let terminal = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    let vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let code = std::process::Command::new(run)
        .args(unit_args(&exe, args, &vars, terminal))
        .status()
        .ok()?
        .code()
        .unwrap_or(1);
    Some(code)
}

#[cfg(test)]
#[path = "cleanstart_tests.rs"]
mod tests;
