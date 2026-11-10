use super::*;
use std::process::Command;

fn git(p: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(p)
        .env("HOME", p)
        .args(args)
        .status()
        .unwrap()
        .success());
}
fn setup() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q"]);
    t
}
#[test]
fn selection_incremental_and_integrity() {
    let t = setup();
    let p = t.path();
    for (n, v) in [
        ("tracked", "one"),
        ("extra", "two"),
        ("unused", "no"),
        ("ignored", "no"),
        (".env", "secret"),
        ("hide", "hidden"),
        (".gitignore", "ignored\n"),
    ] {
        std::fs::write(p.join(n), v).unwrap();
    }
    git(p, &["add", "tracked", ".env", "hide", ".gitignore"]);
    let snap = capture(p, &["extra".into(), "ignored".into()], &["hide".into()]).unwrap();
    assert_eq!(
        snap.manifest
            .files
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![".gitignore", "extra", "tracked"]
    );
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    assert_eq!(store.missing(&snap.manifest).unwrap().len(), 3);
    for (h, b) in &snap.blobs {
        store.put(h, b).unwrap();
    }
    assert!(store.missing(&snap.manifest).unwrap().is_empty());
    let dst = tempfile::tempdir().unwrap();
    store.materialize(&snap.manifest, dst.path()).unwrap();
    assert_eq!(std::fs::read(dst.path().join("tracked")).unwrap(), b"one");
    let source = std::fs::metadata(p.join("tracked")).unwrap();
    let copied = std::fs::metadata(dst.path().join("tracked")).unwrap();
    assert_eq!(
        (source.mtime(), source.mtime_nsec()),
        (copied.mtime(), copied.mtime_nsec())
    );
    std::fs::write(p.join("tracked"), "changed").unwrap();
    let next = capture(p, &["extra".into()], &["hide".into()]).unwrap();
    assert_eq!(store.missing(&next.manifest).unwrap().len(), 1);
    let h = &snap.manifest.files["tracked"].hash;
    assert!(store.put(h, b"wrong").is_err());
    std::fs::write(state.path().join(h), "corrupt").unwrap();
    assert!(store.get(h).is_err());
}
#[test]
fn paths_links_outputs_and_conflicts_are_rejected() {
    let t = setup();
    let p = t.path();
    std::fs::write(p.join("ok"), "ok").unwrap();
    std::os::unix::fs::symlink("ok", p.join("link")).unwrap();
    assert!(collect_outputs(p, &["../outside".into()], &[]).is_err());
    assert!(collect_outputs(p, &["link".into()], &[]).is_err());
    let output = collect_outputs(p, &["ok".into()], &[]).unwrap();
    assert_eq!(output.manifest.files.len(), 1);
    let mut m = output.manifest;
    m.files.insert("ok/child".into(), m.files["ok"].clone());
    assert!(m.validate().is_err());
}

#[test]
fn manifest_rejects_credentials_traversal_and_bad_hashes() {
    let mut manifest = Manifest::default();
    let entry = Entry {
        hash: "a".repeat(64),
        size: 0,
        executable: false,
        modified_ns: 0,
    };
    for name in ["../escape", ".git/config", ".env", "sub/.env"] {
        manifest.files.clear();
        manifest.files.insert(name.into(), entry.clone());
        assert!(manifest.validate().is_err(), "{name}");
    }
    manifest.files.clear();
    manifest.files.insert(
        "safe".into(),
        Entry {
            hash: "z".repeat(64),
            ..entry
        },
    );
    assert!(manifest.validate().is_err());
}

#[test]
fn tracked_links_are_preserved_and_escaping_links_are_named() {
    let t = setup();
    let p = t.path();
    std::fs::create_dir(p.join("sub")).unwrap();
    std::fs::write(p.join("file"), "content").unwrap();
    std::os::unix::fs::symlink("../file", p.join("sub/link")).unwrap();
    git(p, &["add", "file", "sub/link"]);
    let snap = capture(p, &[], &[]).unwrap();
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    for (hash, bytes) in &snap.blobs {
        store.put(hash, bytes).unwrap();
    }
    let dst = tempfile::tempdir().unwrap();
    store.materialize(&snap.manifest, dst.path()).unwrap();
    assert_eq!(
        std::fs::read_link(dst.path().join("sub/link")).unwrap(),
        Path::new("../file")
    );
    assert_eq!(
        std::fs::read(dst.path().join("sub/link")).unwrap(),
        b"content"
    );
    std::fs::remove_file(p.join("sub/link")).unwrap();
    std::os::unix::fs::symlink("../../outside", p.join("sub/link")).unwrap();
    assert!(format!("{:#}", capture(p, &[], &[]).unwrap_err()).contains("sub/link"));
    let mut links = BTreeMap::new();
    links.insert("alias".into(), ".".into());
    links.insert("sub/link".into(), "../alias/../outside".into());
    assert!(link_leaves(&links, "sub/link", &links["sub/link"]));
}

#[test]
fn plain_project_inside_repository_uses_only_its_own_ignore_rules() {
    let parent = setup();
    std::fs::write(parent.path().join(".gitignore"), "parent-only\n").unwrap();
    let project = parent.path().join("plain");
    std::fs::create_dir_all(project.join("nested")).unwrap();
    for name in [
        "parent-only",
        "private",
        "keep.log",
        "drop.log",
        "nested/skip",
        "nested/keep",
    ] {
        std::fs::write(project.join(name), "data").unwrap();
    }
    std::fs::write(project.join(".gitignore"), "*.log\n!keep.log\n").unwrap();
    std::fs::write(project.join(".paddockignore"), "private\n").unwrap();
    std::fs::write(project.join("nested/.gitignore"), "/skip\n").unwrap();
    let snapshot = capture(&project, &[], &[]).unwrap();
    let files = &snapshot.manifest.files;
    for name in ["parent-only", "keep.log", "nested/keep"] {
        assert!(files.contains_key(name), "{name}");
    }
    for name in ["private", "drop.log", "nested/skip"] {
        assert!(!files.contains_key(name), "{name}");
    }
}
