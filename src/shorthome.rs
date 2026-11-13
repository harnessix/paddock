//! Short paths for the sandbox home and `TMPDIR` of a Seatbelt run.
//!
//! macOS limits the path of a Unix socket to 104 bytes (`sun_path`).
//! Programs build socket paths from `$HOME` and `$TMPDIR`: codex binds
//! `$HOME/.codex/app-server-control/app-server-control.sock`. A sandbox home
//! such as `/Users/NAME/.local/share/paddock/homes/codex/shared/PROJECT-HASH`
//! leaves no room for that name.
//!
//! The run gets a short alias instead: a symlink `~/.pdk/h/<8 hex>` that
//! points to the real sandbox home, and `~/.pdk/t/<8 hex>` for `TMPDIR`.
//! `$HOME` is then 16 bytes longer than the host home: 30 bytes for
//! `/Users/NAME` with an 8 letter name, so 72 bytes stay for the names
//! below it.
//!
//! Why an alias and not a short folder for the data: every part of paddock
//! that walks the homes (gc, sizes, dedupe, the usage view, the folder
//! check) reads `homes/HARNESS/LOGIN/PROJECT` and skips symlinks. The data
//! stays there, so a home of an earlier version needs no move: it gets its
//! alias at its next start, and nothing is copied or removed. The account
//! variable of a harness (`CODEX_HOME`) never goes into the sandbox, so
//! codex builds its folder from `$HOME` and does not resolve the alias.
//!
//! Seatbelt checks file access on the resolved path, which is the real
//! home, so the rules of the home apply through the alias. For Unix socket
//! binds and connects the profile also allows the alias path itself
//! ([`allow`]), so a bind works whichever path Seatbelt matches. The
//! sandbox cannot change an alias: `~/.pdk` is in the denied real home, and
//! the profile denies writes to the link.
//!
//! The name is the hash of the target, so one home keeps one alias. A name
//! that holds something else (a link to another existing folder, a file, a
//! folder) is never removed: the next name is used. A link whose target is
//! gone is stale and is replaced.

use crate::seatbelt::{Access, Policy};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The folder of the aliases in the host home.
pub const ROOT: &str = ".pdk";
/// The aliases of sandbox homes.
pub const HOMES: &str = "h";
/// The aliases of the private temporary folders (`TMPDIR`).
pub const TMPS: &str = "t";
/// Hex digits in an alias name.
pub const HEX: usize = 8;
/// How many names [`ensure`] tries before it gives up.
const TRIES: u32 = 16;

/// The folder of the aliases of `kind` ([`HOMES`], [`TMPS`]).
pub fn dir(host_home: &Path, kind: &str) -> PathBuf {
    host_home.join(ROOT).join(kind)
}

/// Name number `n` for `target`. Number 0 is the first choice; the others
/// are for a name that is taken.
pub fn candidate(host_home: &Path, kind: &str, target: &Path, n: u32) -> PathBuf {
    let mut h = Sha256::new();
    h.update(target.as_os_str().as_encoded_bytes());
    if n > 0 {
        h.update(format!("\0{n}").as_bytes());
    }
    let hex = crate::manifest::hex(&h.finalize());
    dir(host_home, kind).join(&hex[..HEX])
}

/// The length of an alias of `kind` for a host home of `host_home_len` bytes.
pub fn alias_len(host_home_len: usize, kind: &str) -> usize {
    host_home_len + 1 + ROOT.len() + 1 + kind.len() + 1 + HEX
}

/// What a name holds.
#[derive(Debug, PartialEq, Eq)]
enum Slot {
    /// Nothing.
    Free,
    /// The link to `target`.
    Ours,
    /// A link whose target is gone.
    Stale,
    /// Something else: a link to another folder, a file or a folder.
    Taken,
}

fn slot(path: &Path, target: &Path) -> Slot {
    let Ok(m) = std::fs::symlink_metadata(path) else {
        return Slot::Free;
    };
    if !m.file_type().is_symlink() {
        return Slot::Taken;
    }
    match std::fs::read_link(path) {
        Ok(t) if t == target => Slot::Ours,
        // A dangling link to nothing: no data is behind it.
        Ok(t) if std::fs::symlink_metadata(&t).is_err() => Slot::Stale,
        _ => Slot::Taken,
    }
}

/// The alias of `kind` for `target`, made when it is missing. `target`
/// must be an absolute path of a folder.
pub fn ensure(host_home: &Path, kind: &str, target: &Path) -> Result<PathBuf> {
    if !target.is_absolute() {
        bail!("an alias needs an absolute target: {}", target.display());
    }
    let d = dir(host_home, kind);
    crate::privfs::dir(&d).with_context(|| format!("cannot make {}", d.display()))?;
    for n in 0..TRIES {
        let link = candidate(host_home, kind, target, n);
        // Two tries: another start can make the same link at the same time.
        for _ in 0..2 {
            match slot(&link, target) {
                Slot::Ours => return Ok(link),
                Slot::Taken => break,
                Slot::Stale => {
                    // Only the link goes: its target is already gone.
                    let _ = std::fs::remove_file(&link);
                }
                Slot::Free => {}
            }
            match std::os::unix::fs::symlink(target, &link) {
                Ok(()) => return Ok(link),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("cannot link {} to {}", link.display(), target.display())
                    })
                }
            }
        }
    }
    bail!(
        "no free short alias for {} in {}: {TRIES} names are taken",
        target.display(),
        d.display()
    )
}

/// The short paths of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Short {
    /// The real sandbox home and its alias.
    pub home: (PathBuf, PathBuf),
    /// The real private temporary folder and its alias.
    pub tmp: (PathBuf, PathBuf),
}

impl Short {
    /// Make the aliases of a run: `home` is the sandbox home, `tmp` its
    /// private temporary folder.
    pub fn ensure(host_home: &Path, home: &Path, tmp: &Path) -> Result<Self> {
        Ok(Self {
            home: (home.to_path_buf(), ensure(host_home, HOMES, home)?),
            tmp: (tmp.to_path_buf(), ensure(host_home, TMPS, tmp)?),
        })
    }

    /// `value` with a leading real path replaced by its alias. The
    /// temporary folder comes first: it is inside the home.
    pub fn path(&self, value: &str) -> String {
        let p = Path::new(value);
        for (real, alias) in [&self.tmp, &self.home] {
            if let Ok(rest) = p.strip_prefix(real) {
                return if rest.as_os_str().is_empty() {
                    alias.display().to_string()
                } else {
                    alias.join(rest).display().to_string()
                };
            }
        }
        value.to_string()
    }

    /// Every value of `vars` with the real paths replaced by the aliases,
    /// also each folder of a `PATH`-like list.
    pub fn env(&self, vars: &mut std::collections::BTreeMap<String, String>) {
        for (k, v) in vars.iter_mut() {
            *v = if k == "PATH" {
                v.split(':')
                    .map(|d| self.path(d))
                    .collect::<Vec<_>>()
                    .join(":")
            } else {
                self.path(v)
            };
        }
    }
}

/// Allow Unix sockets at the alias paths themselves, and a read of the
/// links, in a Seatbelt policy. The real folders keep their own rules.
pub fn allow(p: &mut Policy, s: &Short) {
    for (alias, what) in [(&s.home.1, "sandbox home"), (&s.tmp.1, "TMPDIR")] {
        p.add(
            alias,
            Access::Sockets,
            &format!("Unix sockets through the short alias of the {what}"),
        );
        p.add_file(
            alias,
            Access::ReadOnly,
            &format!("the short alias of the {what}: the sandbox cannot change it"),
        );
    }
}

/// The lines of `paddock doctor` on macOS: the longest socket path that a
/// sandbox home allows.
pub fn doctor_lines(host_home: &Path) -> Vec<String> {
    let max = crate::sockpath::MACOS_MAX - 1;
    let home = alias_len(host_home.as_os_str().len(), HOMES);
    let tmp = alias_len(host_home.as_os_str().len(), TMPS);
    let room = max.saturating_sub(home + 1);
    let codex = "/.codex/app-server-control/app-server-control.sock".len();
    let state = if home + codex <= max { "ok" } else { "warn" };
    let mut v = vec![
        format!(
            "  {state:<9}$HOME of a run is {home} bytes ({}/<8 hex>): socket paths up to {max} bytes, so {room} bytes below $HOME",
            dir(host_home, HOMES).display()
        ),
        format!(
            "           TMPDIR of a run is {tmp} bytes ({}/<8 hex>)",
            dir(host_home, TMPS).display()
        ),
    ];
    if state == "warn" {
        v.push(format!(
            "           codex needs {codex} bytes below $HOME for its app server socket: a shorter user name or home folder helps"
        ));
    }
    v
}

/// The section of `paddock doctor` on macOS.
pub fn print_doctor(env: &crate::env::HostEnv) {
    println!("\nUnix socket paths in a Seatbelt run (macOS limit: 104 bytes):");
    for l in doctor_lines(&env.home) {
        println!("{l}");
    }
}

#[cfg(test)]
#[path = "shorthome_tests.rs"]
mod tests;
