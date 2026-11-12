//! `paddock --version` names the build: only the version for a release,
//! the version with `-dev`, the commit and a dirty mark for any other build.
//! src/buildinfo.rs has the tests of the rule itself.

mod common;

use std::process::Command;

#[test]
fn the_version_names_the_build() {
    // Temporary folders, so that no build reads or writes the user's own.
    let tmp = common::tempdir();
    let root = tmp.path();
    let out = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let rest = text
        .trim_end()
        .strip_prefix(concat!("paddock ", env!("CARGO_PKG_VERSION")))
        .unwrap_or_else(|| panic!("{text:?}"));
    let dev = rest
        .strip_prefix("-dev (commit ")
        .map(|r| r.trim_end_matches(')'));
    match dev {
        None => assert_eq!(rest, "", "{text:?}"),
        Some(c) => {
            let hash = c.strip_suffix(", dirty").unwrap_or(c);
            assert!(
                hash == "unknown"
                    || (hash.len() == 7 && hash.bytes().all(|b| b.is_ascii_hexdigit())),
                "{text:?}"
            );
        }
    }
}
