//! The identity of an allowlisted program, and the programs that can never
//! be on an allowlist.
//!
//! A program is named by its absolute path, plus an optional SHA-256 of its
//! content or, on macOS, the identity that signed it. A shell, an
//! interpreter, `env` or `printenv` runs whatever it is told to run, or
//! prints its environment: allowlisting one would give the value to any
//! command. They are refused by name, and by the name of the file that a
//! symbolic link points to.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Programs that run other code they are given, or print the environment.
/// Versioned names (`python3.12`, `perl5.36`) count too.
const REFUSED: &[(&str, &str)] = &[
    ("sh", "a shell"),
    ("bash", "a shell"),
    ("dash", "a shell"),
    ("ash", "a shell"),
    ("zsh", "a shell"),
    ("fish", "a shell"),
    ("ksh", "a shell"),
    ("mksh", "a shell"),
    ("oksh", "a shell"),
    ("yash", "a shell"),
    ("csh", "a shell"),
    ("tcsh", "a shell"),
    ("nu", "a shell"),
    ("elvish", "a shell"),
    ("xonsh", "a shell"),
    ("pwsh", "a shell"),
    ("busybox", "a shell"),
    ("toybox", "a shell"),
    ("python", "an interpreter"),
    ("python3", "an interpreter"),
    ("python2", "an interpreter"),
    ("pypy", "an interpreter"),
    ("pypy3", "an interpreter"),
    ("perl", "an interpreter"),
    ("ruby", "an interpreter"),
    ("irb", "an interpreter"),
    ("node", "an interpreter"),
    ("nodejs", "an interpreter"),
    ("deno", "an interpreter"),
    ("bun", "an interpreter"),
    ("lua", "an interpreter"),
    ("luajit", "an interpreter"),
    ("php", "an interpreter"),
    ("tclsh", "an interpreter"),
    ("wish", "an interpreter"),
    ("expect", "an interpreter"),
    ("osascript", "an interpreter"),
    ("awk", "an interpreter"),
    ("gawk", "an interpreter"),
    ("mawk", "an interpreter"),
    ("nawk", "an interpreter"),
    ("jshell", "an interpreter"),
    ("R", "an interpreter"),
    ("Rscript", "an interpreter"),
    ("julia", "an interpreter"),
    (
        "env",
        "a program that runs other programs and prints the environment",
    ),
    ("printenv", "a program that prints the environment"),
    ("xargs", "a program that runs other programs"),
    ("nohup", "a program that runs other programs"),
    ("sudo", "a program that runs other programs"),
    ("doas", "a program that runs other programs"),
    ("exec", "a program that runs other programs"),
];

/// Why a program can never be allowlisted, or None.
pub fn refused(path: &Path) -> Option<String> {
    let mut names = vec![path.to_path_buf()];
    // A link such as /usr/local/bin/tool -> /usr/bin/bash.
    if let Ok(real) = std::fs::canonicalize(path) {
        if real != path {
            names.push(real);
        }
    }
    for p in names {
        let Some(base) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(kind) = refused_name(base) {
            let via = if p == path {
                String::new()
            } else {
                format!(" ({} points to {})", path.display(), p.display())
            };
            return Some(format!("{base} is {kind}{via}"));
        }
    }
    None
}

fn refused_name(base: &str) -> Option<&'static str> {
    let stem = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let stem = stem.strip_suffix('-').unwrap_or(stem);
    REFUSED
        .iter()
        .find(|(n, _)| *n == base || *n == stem)
        .map(|(_, k)| *k)
}

/// Whether `p` is a path that an allowlist entry can name: absolute, with
/// no `.` or `..` parts.
pub fn valid_path(p: &Path) -> bool {
    // components() drops a `.` in the middle, so look at the text.
    let text = p.as_os_str().as_encoded_bytes();
    p.is_absolute()
        && p.file_name().is_some()
        && !text.ends_with(b"/")
        && !text.split(|&c| c == b'/').any(|s| s == b"." || s == b"..")
        && p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// Whether `h` is a SHA-256 in lower-case hex.
pub fn valid_sha256(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// The SHA-256 of a file's content, in lower-case hex.
pub fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(crate::manifest::hex(&h.finalize()))
}

/// The state of one allowlisted program on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// It is there, and its hash matches when the entry has one.
    Ok {
        hashed: bool,
    },
    Missing,
    NotExecutable,
    /// Its content changed since the entry was written: an update, or a
    /// replaced file.
    HashChanged {
        now: String,
    },
    /// It cannot be read to compute its hash.
    Unreadable(String),
}

/// Check one program against its entry.
pub fn check(program: &Path, sha256: Option<&str>) -> State {
    let Ok(m) = std::fs::metadata(program) else {
        return State::Missing;
    };
    if !m.is_file() || !crate::env::is_executable(program) {
        return State::NotExecutable;
    }
    let Some(want) = sha256 else {
        return State::Ok { hashed: false };
    };
    match sha256_file(program) {
        Ok(now) if now == want => State::Ok { hashed: true },
        Ok(now) => State::HashChanged { now },
        Err(e) => State::Unreadable(e.to_string()),
    }
}

/// The real file behind a program path, for messages.
pub fn real_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn shells_interpreters_env_and_printenv_are_refused() {
        for p in [
            "/bin/sh",
            "/usr/bin/bash",
            "/usr/bin/zsh",
            "/usr/bin/fish",
            "/usr/bin/python3",
            "/usr/bin/python3.12",
            "/usr/bin/perl5.36",
            "/usr/bin/node",
            "/usr/bin/env",
            "/usr/bin/printenv",
            "/usr/bin/sudo",
            "/opt/homebrew/bin/ruby",
        ] {
            assert!(refused(Path::new(p)).is_some(), "{p}");
        }
        for p in [
            "/usr/bin/gh",
            "/usr/bin/git",
            "/usr/local/bin/aws",
            "/usr/bin/shellcheck",
        ] {
            assert_eq!(refused(Path::new(p)), None, "{p}");
        }
    }

    #[test]
    fn a_link_to_a_shell_is_refused_by_its_target() {
        let t = tempfile::tempdir().unwrap();
        let bash = t.path().join("bash");
        std::fs::write(&bash, "").unwrap();
        let tool = t.path().join("mytool");
        std::os::unix::fs::symlink(&bash, &tool).unwrap();
        let why = refused(&tool).unwrap();
        assert!(
            why.contains("a shell") && why.contains("points to"),
            "{why}"
        );
    }

    #[test]
    fn paths_must_be_absolute_and_plain() {
        assert!(valid_path(Path::new("/usr/bin/gh")));
        for p in ["gh", "./gh", "/usr/../bin/gh", "/usr/./gh", "/"] {
            assert!(!valid_path(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn check_names_missing_changed_and_matching_programs() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("tool");
        assert_eq!(check(&p, None), State::Missing);
        std::fs::write(&p, "#!/bin/true\n").unwrap();
        assert_eq!(check(&p, None), State::NotExecutable);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let h = sha256_file(&p).unwrap();
        assert!(valid_sha256(&h));
        assert_eq!(check(&p, Some(&h)), State::Ok { hashed: true });
        std::fs::write(&p, "#!/bin/false\n").unwrap();
        assert!(matches!(check(&p, Some(&h)), State::HashChanged { .. }));
        assert_eq!(check(t.path(), None), State::NotExecutable);
    }
}
