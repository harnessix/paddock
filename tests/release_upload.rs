//! .github/scripts/release-upload.sh with a fake `gh`: a new release, a
//! new asset, an asset with the same sha256 and one with another sha256.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A `gh` that keeps the release of one tag as a folder: `release` exists
/// when the release does, `release/assets/` holds the files. Each call
/// goes to `calls`.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE_GH/calls"
rel="$FAKE_GH/release"
[ "$1" = release ] || exit 99
cmd=$2; tag=$3; shift 3
case "$cmd" in
  view)
    [ -d "$rel" ] || { echo "release not found" >&2; exit 1; }
    if [ "${1:-}" = --json ]; then ls "$rel/assets"; fi ;;
  create)
    mkdir -p "$rel/assets"
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --title|--notes-file) shift 2 ;;
        --*) shift ;;
        *) cp "$1" "$rel/assets/"; shift ;;
      esac
    done ;;
  download)
    [ "$1" = --pattern ] && [ "$3" = --dir ] || exit 98
    cp "$rel/assets/$2" "$4/" ;;
  upload)
    for f in "$@"; do
      [ "$f" = --clobber ] && { echo "--clobber used" >&2; exit 97; }
      [ -e "$rel/assets/$(basename "$f")" ] && { echo "asset exists" >&2; exit 1; }
    done
    for f in "$@"; do cp "$f" "$rel/assets/"; done ;;
  *) exit 96 ;;
esac
"#;

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let gh = bin.join("gh");
        std::fs::write(&gh, FAKE_GH).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(dir.path().join("dist")).unwrap();
        std::fs::write(dir.path().join("notes.md"), "notes\n").unwrap();
        Self { dir }
    }

    fn file(&self, name: &str, body: &str) -> PathBuf {
        let p = self.dir.path().join("dist").join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    fn asset(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.path().join("release/assets").join(name)).ok()
    }

    fn upload(&self, files: &[&Path]) -> Output {
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/scripts/release-upload.sh");
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new("bash")
            .arg(script)
            .arg("v9.9.9")
            .arg(self.dir.path().join("notes.md"))
            .args(files)
            .env("PATH", path)
            .env("FAKE_GH", self.dir.path())
            .output()
            .unwrap()
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls")).unwrap_or_default()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn creates_the_release_with_every_file() {
    let f = Fake::new();
    let a = f.file("a.tar.gz", "a");
    let b = f.file("SHA256SUMS", "b");
    let out = f.upload(&[&a, &b]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(f.asset("a.tar.gz").as_deref(), Some("a"));
    assert_eq!(f.asset("SHA256SUMS").as_deref(), Some("b"));
    assert!(f.calls().contains("release create v9.9.9 --verify-tag"));
}

#[test]
fn keeps_an_asset_with_the_same_checksum_and_adds_the_new_one() {
    let f = Fake::new();
    let a = f.file("a.tar.gz", "a");
    assert!(f.upload(&[&a]).status.success());
    let b = f.file("b.tar.gz", "b");
    let out = f.upload(&[&a, &b]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("a.tar.gz is already on v9.9.9 with the same sha256"));
    assert_eq!(f.asset("b.tar.gz").as_deref(), Some("b"));
    let calls = f.calls();
    let uploads: Vec<&str> = calls
        .lines()
        .filter(|l| l.starts_with("release upload"))
        .collect();
    assert_eq!(uploads.len(), 1, "{uploads:?}");
    assert!(!uploads[0].contains("a.tar.gz"), "{uploads:?}");
    assert!(!f.calls().contains("--clobber"));
}

#[test]
fn refuses_an_asset_with_another_checksum_and_uploads_nothing() {
    let f = Fake::new();
    let a = f.file("a.tar.gz", "a");
    assert!(f.upload(&[&a]).status.success());
    let a = f.file("a.tar.gz", "changed");
    let b = f.file("b.tar.gz", "b");
    let out = f.upload(&[&b, &a]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("a.tar.gz is already on v9.9.9 with sha256"));
    assert_eq!(
        f.asset("a.tar.gz").as_deref(),
        Some("a"),
        "the old asset stays"
    );
    assert_eq!(f.asset("b.tar.gz"), None, "nothing is uploaded");
    assert!(!f.calls().contains("release upload"));
}
