//! The shared-cache guard uses only fake directories and sentinels.
mod common;

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[test]
fn creates_and_removes_only_its_own_directory() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("p");
    let outside = root.join("o");
    fs::create_dir(&parent).unwrap();
    fs::create_dir(&outside).unwrap();
    let old = parent.join("t-old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("keep"), b"old").unwrap();
    fs::File::open(&old)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(172800)),
        )
        .unwrap();
    fs::write(outside.join("keep"), b"outside").unwrap();
    let link = parent.join("t-link");
    symlink(&outside, &link).unwrap();

    let own = common::tempdir_in(&parent, &[]).unwrap();
    assert!(own.path().starts_with(&parent));
    assert_ne!(own.path(), old);
    fs::write(own.path().join("owned"), b"ours").unwrap();
    let own_path = own.path().to_path_buf();
    drop(own);
    assert!(!own_path.exists());

    assert_eq!(fs::read(old.join("keep")).unwrap(), b"old");
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside");
    assert!(link.is_symlink());
}

#[test]
fn rejects_shared_cache_home_and_symlinked_parents() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let cache = root.join("c");
    let home = root.join("h");
    let safe = root.join("p");
    for dir in [&cache, &home, &safe] {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("keep"), b"unchanged").unwrap();
    }
    let child = cache.join("nested");
    fs::create_dir(&child).unwrap();
    let alias = root.join("alias");
    symlink(&cache, &alias).unwrap();
    let protected = vec![cache.clone(), home.clone()];
    for parent in [&cache, &child, &home, &alias] {
        assert!(common::tempdir_in(parent, &protected).is_err());
    }
    // An alias in the cache setting must also protect its resolved target.
    assert!(common::tempdir_in(&cache, &[alias]).is_err());
    assert!(common::tempdir_in(&PathBuf::from("relative"), &protected).is_err());
    assert!(common::tempdir_in(&safe, &[cache.join("../cache")]).is_err());
    fs::set_permissions(&safe, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(common::tempdir_in(&safe, &protected).is_err());
    fs::set_permissions(&safe, fs::Permissions::from_mode(0o700)).unwrap();
    for dir in [&cache, &home, &safe] {
        assert_eq!(fs::read(dir.join("keep")).unwrap(), b"unchanged");
    }
    assert_eq!(fs::read_dir(&safe).unwrap().count(), 1);
}

#[test]
fn socket_budget_rejects_long_parents() {
    assert!(common::fits_socket_paths(std::path::Path::new("/short")));
    assert!(!common::fits_socket_paths(std::path::Path::new(
        "/a-really-long-parent-directory-for-fixtures"
    )));
}

// The subprocess has its own environment, so this also checks that the
// public helper honors XDG_CACHE_HOME without racing other test threads.
#[test]
fn helper_refuses_a_cache_parent() {
    // This test does not launch a sandbox. A short fixture in /tmp lets it
    // exercise cache validation rather than the socket-length guard.
    let tmp = fs::canonicalize("/tmp").unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("z-")
        .tempdir_in(tmp)
        .unwrap();
    let root = fixture.path();
    let cache = root.join("c");
    let home = root.join("h");
    assert!(common::fits_socket_paths(&cache));
    fs::create_dir(&cache).unwrap();
    fs::create_dir(&home).unwrap();
    fs::write(cache.join("keep"), b"unchanged").unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "scratch_child", "--nocapture"])
        .env("PADDOCK_SCRATCH_CHILD", "1")
        .env("PADDOCK_TEST_PARENT", &cache)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("test parent is protected"), "{error}");
    assert_eq!(fs::read(cache.join("keep")).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
}

#[test]
fn scratch_child() {
    if std::env::var_os("PADDOCK_SCRATCH_CHILD").is_some() {
        drop(common::tempdir());
    }
}

#[test]
fn concurrent_folders_are_distinct_and_drop_independently() {
    let fixture = tempfile::tempdir().unwrap();
    let parent = fixture.path().canonicalize().unwrap();
    let old = parent.join("old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("keep"), b"old").unwrap();
    let barrier = std::sync::Barrier::new(3);
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            common::tempdir_in(&parent, &[]).unwrap()
        });
        let second = scope.spawn(|| {
            barrier.wait();
            common::tempdir_in(&parent, &[]).unwrap()
        });
        barrier.wait();
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_ne!(first.path(), second.path());
    fs::write(first.path().join("own"), b"first").unwrap();
    fs::write(second.path().join("keep"), b"second").unwrap();
    let first_path = first.path().to_path_buf();
    drop(first);
    assert!(!first_path.exists());
    assert_eq!(fs::read(second.path().join("keep")).unwrap(), b"second");
    assert_eq!(fs::read(old.join("keep")).unwrap(), b"old");
}
