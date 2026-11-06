use super::*;
use std::os::unix::fs::PermissionsExt;

fn exe(p: &Path, body: &str) {
    std::fs::write(p, body).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn me() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { libc::geteuid() }
}

#[test]
fn only_folders_that_everyone_can_list_count() {
    let t = tempfile::tempdir().unwrap();
    let open = t.path().join("open");
    let shut = t.path().join("shut");
    for d in [&open, &shut] {
        std::fs::create_dir(d).unwrap();
    }
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o750)).unwrap();
    let path = format!(
        "{}:{}:relative:{}:/nonexistent",
        open.display(),
        shut.display(),
        open.display()
    );
    let got = dirs_for(&path, &[]);
    // The temporary folder's parents must be enterable by others too.
    let parents_open = t.path().ancestors().skip(1).all(|a| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(a).is_ok_and(|m| m.mode() & 1 != 0)
    });
    if parents_open {
        assert_eq!(got, std::slice::from_ref(&open));
    } else {
        assert!(got.is_empty());
    }
    assert!(dirs_for(&open.display().to_string(), std::slice::from_ref(&open)).is_empty());
}

#[test]
fn the_wrapper_folder_links_every_name_and_is_made_once() {
    let t = tempfile::tempdir().unwrap();
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin = t.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe(&bin.join("git"), "#!/bin/sh\n");
    exe(&bin.join(".hidden"), "#!/bin/sh\n");
    let target = t.path().join("tackd");
    exe(&target, "");
    let root = t.path().join("wrap");
    let dirs = format!("{}:/usr/bin", bin.display());
    let a = ensure(&root, me(), &dirs, &target, &[]).unwrap();
    let b = ensure(&root, me(), &dirs, &target, &[]).unwrap();
    assert_eq!(a, b, "the same folder for the same PATH");
    if dirs_for(&dirs, &[]).contains(&bin) {
        assert_eq!(std::fs::read_link(a.join("git")).unwrap(), target);
        assert!(!a.join(".hidden").exists());
    }
    // A new program changes the folder: a new wrapper folder.
    std::thread::sleep(std::time::Duration::from_millis(20));
    exe(&bin.join("gh"), "#!/bin/sh\n");
    let c = ensure(&root, me(), &dirs, &target, &[]).unwrap();
    if dirs_for(&dirs, &[]).contains(&bin) {
        assert_ne!(a, c);
        assert!(c.join("gh").exists());
    }
    // No temporary folder is left.
    let left: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".new-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // A root folder that others can write is refused.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(ensure(&root, me(), &dirs, &target, &[]).is_err());
}

#[test]
fn the_real_program_is_found_after_the_wrapper_and_never_is_tackd() {
    let t = tempfile::tempdir().unwrap();
    let wrap = t.path().join("wrap");
    let a = t.path().join("a");
    let b = t.path().join("b");
    for d in [&wrap, &a, &b] {
        std::fs::create_dir(d).unwrap();
    }
    let tackd = t.path().join("tackd");
    exe(&tackd, "");
    std::os::unix::fs::symlink(&tackd, wrap.join("git")).unwrap();
    std::os::unix::fs::symlink(&tackd, a.join("git")).unwrap();
    std::fs::write(b.join("notexec"), "").unwrap();
    exe(&b.join("git"), "#!/bin/sh\n");
    let path = format!("{}:{}:{}", wrap.display(), a.display(), b.display());
    let me_real = std::fs::canonicalize(&tackd).unwrap();
    assert_eq!(
        real_program("git", &path, &wrap, Some(&me_real)),
        Some(b.join("git"))
    );
    assert_eq!(real_program("notexec", &path, &wrap, Some(&me_real)), None);
    assert_eq!(real_program("nope", &path, &wrap, None), None);
}

#[test]
fn the_wrapper_puts_references_in_and_sends_listed_programs_to_tackd() {
    let has = |n: &str| n == "GH_TOKEN" || n == "NPM_TOKEN";
    assert_eq!(
        references("GH_TOKEN,NPM_TOKEN,UNSET,bad name", &has),
        [
            ("GH_TOKEN".to_string(), "tack://GH_TOKEN".to_string()),
            ("NPM_TOKEN".to_string(), "tack://NPM_TOKEN".to_string())
        ]
    );
    assert!(references("", &has).is_empty());
    let t = tempfile::tempdir().unwrap();
    let gh = t.path().join("gh");
    exe(&gh, "");
    let link = t.path().join("gh-link");
    std::os::unix::fs::symlink(&gh, &link).unwrap();
    let real = std::fs::canonicalize(&gh).unwrap();
    let list = format!("/usr/bin/x:{}", real.display());
    assert!(listed(&list, &gh) && listed(&list, &link));
    assert!(!listed("/usr/bin/x:", &gh));
}
