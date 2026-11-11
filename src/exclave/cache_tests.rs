use super::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::Command;

#[test]
fn isolated_caches_reject_links_and_unsafe_permissions() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let a = ProjectCache::open(temp.path(), b"project-a").unwrap();
    let b = ProjectCache::open(temp.path(), b"project-b").unwrap();
    assert_ne!(a.target_dir(), b.target_dir());
    fs::write(a.target_dir().join("marker"), b"a").unwrap();
    assert!(!b.target_dir().join("marker").exists());
    let target = a.target_dir().to_owned();
    drop(a);
    fs::remove_dir_all(&target).unwrap();
    symlink(b.target_dir(), &target).unwrap();
    assert!(ProjectCache::open(temp.path(), b"project-a").is_err());
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ProjectCache::open(temp.path(), b"project-c").is_err());
}

#[test]
fn fixture_crate_reuses_project_cache_offline() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("xdg-data/paddock");
    let project = temp.path().join("project");
    let cargo_home = temp.path().join("xdg-cache/cargo");
    fs::create_dir_all(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir_all(project.join("dependency/src")).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();
    fs::write(project.join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n[dependencies]\nlocal-dep={path='dependency'}\n").unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "#[test] fn works() { assert_eq!(local_dep::answer(), 42); }\n",
    )
    .unwrap();
    fs::write(
        project.join("dependency/Cargo.toml"),
        "[package]\nname='local-dep'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    fs::write(
        project.join("dependency/src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    )
    .unwrap();
    for run in 0..2 {
        // Each nested build has its own memory check. The outer test runner holds
        // the tree-wide cargo lease for both runs.
        #[cfg(target_os = "linux")]
        loop {
            let info = fs::read_to_string("/proc/meminfo").unwrap();
            let kb: u64 = info
                .lines()
                .find(|s| s.starts_with("MemAvailable:"))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            if kb >= 3 * 1024 * 1024 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        let cache = ProjectCache::open(&data, b"fixture-project").unwrap();
        let output = Command::new("cargo")
            .args(["test", "--offline", "--verbose"])
            .current_dir(&project)
            .env("CARGO_HOME", &cargo_home)
            .env("CARGO_TARGET_DIR", cache.target_dir())
            .env("CARGO_BUILD_JOBS", "2")
            // CI sets CARGO_TERM_COLOR=always; colour codes would split the
            // status words that this test reads.
            .env("CARGO_TERM_COLOR", "never")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        if run == 0 {
            assert!(stderr.contains("Compiling local-dep"), "{stderr}");
        } else {
            assert!(
                stderr.contains("Fresh local-dep") && stderr.contains("Fresh fixture"),
                "{stderr}"
            );
        }
    }
}

#[test]
fn python_cache_is_project_scoped_limited_and_removable() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let cache = ProjectCache::open(temp.path(), b"python-project").unwrap();
    std::fs::write(cache.python_dir().join("package"), vec![1u8; 1024]).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("large"), vec![0; 4096]).unwrap();
    symlink(outside.path(), cache.python_dir().join("outside")).unwrap();
    assert!(cache.check_size(10).is_err());
    assert!(cache.check_size(2048).is_ok());
    cache.record_size().unwrap();
    let listed = manage(temp.path(), None).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].1 >= 1024);
    assert!(manage(temp.path(), Some(&listed[0].0)).is_err());
    drop(cache);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match manage(temp.path(), Some(&listed[0].0)) {
            Ok(_) => break,
            Err(e)
                if e.to_string() == "project cache is in use"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(e) => panic!("{e}"),
        }
    }
    let reopened = ProjectCache::open(temp.path(), b"python-project").unwrap();
    assert!(!reopened.python_dir().join("package").exists());
}

#[test]
fn volume_full_respects_unbounded_inode_counts() {
    assert!(!full_counts(100, 0, 0));
    assert!(full_counts(0, 0, 0));
    assert!(full_counts(100, 100, 0));
    assert!(!full_counts(100, 100, 1));
}

#[test]
fn cap_evicts_oldest_idle_cache_and_preserves_leases() {
    // Other tests fork, temporarily inheriting flock descriptors. Run this
    // lease-reacquisition check alone so those children cannot hold its lock.
    const CHILD: &str = "PADDOCK_TEST_EXCLAVE_CACHE_ISOLATED";
    const NAME: &str = "exclave::cache::tests::cap_evicts_oldest_idle_cache_and_preserves_leases";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, "1")
            .env("RUST_TEST_THREADS", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let old = ProjectCache::open(root, b"old").unwrap();
    fs::write(old.target_dir().join("data"), vec![1; 100]).unwrap();
    let old_path = old.project.clone();
    drop(old);
    let newer = ProjectCache::open(root, b"newer").unwrap();
    fs::write(newer.target_dir().join("data"), vec![1; 100]).unwrap();
    let newer_path = newer.project.clone();
    drop(newer);
    let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
    fs::File::open(old_path.join("access"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(past))
        .unwrap();
    let active = ProjectCache::open(root, b"active").unwrap();
    fs::write(active.target_dir().join("data"), vec![1; 100]).unwrap();
    active.enforce_cap(200).unwrap();
    assert!(!old_path.join("target/data").exists());
    assert!(newer_path.join("target/data").exists());
    assert!(active.target_dir().join("data").exists());
    let lease = ProjectCache::open(root, b"newer").unwrap();
    let error = active.enforce_cap(100).unwrap_err();
    assert!(error
        .to_string()
        .contains("in-use caches cannot be evicted"));
    assert!(lease.target_dir().join("data").exists());
    assert!(active.target_dir().join("data").exists());
    drop(lease);
    active.enforce_cap(100).unwrap();
    assert!(!newer_path.join("target/data").exists());
}
