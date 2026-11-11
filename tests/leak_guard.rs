//! Leak guard: the document rule and the local pattern rule on a
//! temporary git repository.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A repository with the guard script and the public documents.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/scripts/leak-guard.sh");
    write(
        d,
        ".github/scripts/leak-guard.sh",
        &std::fs::read_to_string(script).unwrap(),
    );
    for f in [
        "README.md",
        "CHANGELOG.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        ".github/pull_request_template.md",
        "changes/a-feature.md",
        "skills/paddock/SKILL.md",
        "skills/paddock/references/kv.md",
        "docs/assets/hero.png",
        "src/main.rs",
    ] {
        write(d, f, "x\n");
    }
    git(d, &["add", "-A"]);
    dir
}

fn guard(dir: &Path) -> (bool, String) {
    let out = Command::new("bash")
        .arg(".github/scripts/leak-guard.sh")
        .current_dir(dir)
        .output()
        .expect("bash");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn the_public_documents_pass() {
    let r = repo();
    let (ok, out) = guard(r.path());
    assert!(ok, "{out}");
}

#[test]
fn design_notes_and_other_documents_fail() {
    for f in [
        "docs/design/protected-windows.md",
        "docs/guide.md",
        "docs/plan.txt",
        "notes.md",
        "src/NOTES.md",
        "PRODUCT.md",
        "skills/paddock/plan.md",
        "changes/sub/x.md",
    ] {
        let r = repo();
        write(r.path(), f, "internal\n");
        git(r.path(), &["add", "-A"]);
        let (ok, out) = guard(r.path());
        assert!(!ok, "{f} passed:\n{out}");
        assert!(out.contains(f), "{f} not named:\n{out}");
    }
}

/// `rel` with `body` added to a fresh repository: the guard's verdict.
fn with(rel: &str, body: &str) -> (bool, String) {
    let r = repo();
    write(r.path(), rel, body);
    git(r.path(), &["add", "-A"]);
    guard(r.path())
}

/// Sample local patterns, as `.git/info/leak-patterns` holds them.
const PATTERNS: &str = "ZQ-?[0-9]+[a-z]?\nsecret-plan\\.md\nplan section [0-9]+\n";

fn with_patterns(rel: &str, body: &str) -> (bool, String) {
    let dir = repo();
    write(dir.path(), ".git/info/leak-patterns", PATTERNS);
    write(dir.path(), rel, body);
    git(dir.path(), &["add", "-A"]);
    guard(dir.path())
}

#[test]
fn local_patterns_fail_with_file_and_line() {
    for text in [
        "as ZQ-7 says",
        "see zq-12b",
        "in secret-plan.md",
        "as Plan Section 4 says",
    ] {
        let (ok, out) = with_patterns("src/lib.rs", &format!("fn a() {{}}\n// {text}\n"));
        assert!(!ok, "{text:?} passed:\n{out}");
        assert!(
            out.contains("src/lib.rs:2:"),
            "{text:?}: no file and line:\n{out}"
        );
    }
}

#[test]
fn without_a_patterns_file_the_rule_is_off() {
    let (ok, out) = with("src/lib.rs", "// as ZQ-7 says\n");
    assert!(ok, "{out}");
}

#[test]
fn product_words_and_other_ids_pass() {
    for text in [
        "the pasture scope lives under /pasture/demo",
        "AVX-512, SHA-256, UTF-8 and x86-64",
        "a design of its own, section by section",
        "the order of the steps",
    ] {
        let (ok, out) = with_patterns("src/lib.rs", &format!("// {text}\n"));
        assert!(ok, "{text:?} failed:\n{out}");
    }
}

#[test]
fn released_changelog_sections_are_history() {
    let id = "ZQ-3";
    let old =
        format!("# Changelog\n\n## Unreleased\n\n- New: a thing.\n\n## 0.1.0\n\n- {id} landed.\n");
    let (ok, out) = with_patterns("CHANGELOG.md", &old);
    assert!(ok, "{out}");
    let new =
        format!("# Changelog\n\n## Unreleased\n\n- New: {id} landed.\n\n## 0.1.0\n\n- Old.\n");
    let (ok, out) = with_patterns("CHANGELOG.md", &new);
    assert!(!ok, "{out}");
    assert!(out.contains("CHANGELOG.md:5:"), "{out}");
}
