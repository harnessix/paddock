use super::*;
use std::os::unix::fs::PermissionsExt;

fn env_in(t: &Path) -> HostEnv {
    crate::privfs::tests::env_in(t)
}

#[test]
fn every_path_for_a_fake_home() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path());
    let l = Layout::new(&env);
    let root = env.paddock_data().join("macvm");
    assert_eq!(l.root, root);
    assert_eq!(l.golden.dir, root.join("golden.noindex"));
    assert_eq!(l.golden.manifest, root.join("golden.noindex/manifest.toml"));
    assert_eq!(l.golden.disk, root.join("golden.noindex/disk.img"));
    assert_eq!(l.golden.aux, root.join("golden.noindex/aux.img"));
    assert_eq!(
        l.golden.hardware_model,
        root.join("golden.noindex/hardware-model")
    );
    assert_eq!(l.golden.machine_id, root.join("golden.noindex/machine-id"));
    assert_eq!(l.golden_new.dir, root.join("golden.new.noindex"));
    assert_eq!(l.golden_prev.dir, root.join("golden.prev.noindex"));
    assert_eq!(l.ipsw_file("15.2"), root.join("ipsw/15.2.ipsw"));
    assert_eq!(
        l.agent_file("0.9.0"),
        root.join("agent/0.9.0/paddock-guest")
    );
    assert_eq!(l.sessions, root.join("sessions"));
    assert_eq!(l.trash, root.join("trash"));
    assert_eq!(
        l.profile("chatgpt-desktop", "default").dir,
        root.join("profiles.noindex/chatgpt-desktop/default")
    );
    assert_eq!(
        l.record("chatgpt-desktop", "default"),
        root.join("sessions/chatgpt-desktop--default.json")
    );
    assert_eq!(l.shims, root.join("shims"));
    // Spotlight stays out of the golden and the profiles.
    for d in [
        &l.golden.dir,
        &l.golden_new.dir,
        &l.golden_prev.dir,
        &l.profiles,
    ] {
        assert!(d.to_string_lossy().ends_with(".noindex"), "{}", d.display());
    }
    // A name never escapes its folder.
    assert_eq!(l.ipsw_file("../x"), root.join("ipsw/_x.ipsw"));
    assert_eq!(safe_name("../../etc"), "_.._etc");
    assert_eq!(safe_name("15.2"), "15.2");
    assert_eq!(safe_name("15.3 (24D1)"), "15.3_24D1");
    assert_eq!(safe_name("a  b__c"), "a_b_c");
    assert_eq!(safe_name(""), "_");
}

#[test]
fn partial_commit_renames_and_never_leaves_partial() {
    let t = tempfile::tempdir().unwrap();
    let disk = t.path().join("disk.img");
    let p = partial(&disk);
    assert_eq!(p, t.path().join("disk.img.partial"));
    assert!(is_partial(&p) && !is_partial(&disk));
    assert_eq!(final_name(&p).unwrap(), disk);
    assert!(final_name(&disk).is_none());
    assert!(final_name(Path::new("/x/.partial")).is_none());
    std::fs::write(&p, b"data").unwrap();
    assert_eq!(partials_in(t.path()), vec![p.clone()]);
    commit(&p).unwrap();
    assert!(!p.exists() && disk.exists());
    assert!(partials_in(t.path()).is_empty());
    assert_eq!(std::fs::read(&disk).unwrap(), b"data");
    // A commit of a name without the suffix is an error, not a rename.
    let e = commit(&disk).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    assert!(disk.exists());
    // A folder commits too.
    let d = t.path().join("apps.partial");
    std::fs::create_dir(&d).unwrap();
    commit(&d).unwrap();
    assert!(t.path().join("apps").is_dir());
}

#[test]
fn golden_partials_are_listed_and_removed() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    assert!(l.golden.partials().is_empty(), "no folder, no partials");
    std::fs::create_dir_all(&l.golden.dir).unwrap();
    std::fs::write(partial(&l.golden.disk), "x").unwrap();
    std::fs::write(&l.golden.aux, "y").unwrap();
    std::fs::create_dir(l.golden.dir.join("seed.partial")).unwrap();
    let list = l.golden.partials();
    assert_eq!(
        list,
        vec![partial(&l.golden.disk), l.golden.dir.join("seed.partial")]
    );
    assert_eq!(l.golden.remove_partials().unwrap(), list);
    assert!(l.golden.partials().is_empty());
    assert!(l.golden.aux.exists(), "a complete file stays");
}

#[test]
fn manifest_round_trip_through_the_golden() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    assert!(l.golden.read_manifest().unwrap().is_none());
    let mut m = super::super::golden::Manifest::new("15.2", "24C101", "0.2.1");
    m.version = "15.2".into();
    m.build = "24C101".into();
    m.at(super::super::golden::State::Install);
    l.golden.write_manifest(&m).unwrap();
    assert!(l.golden.partials().is_empty(), "the manifest is committed");
    assert_eq!(l.golden.read_manifest().unwrap(), Some(m));
    assert_eq!(
        std::fs::metadata(&l.golden.manifest)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn seal_and_unseal_change_the_mode() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("disk.img");
    std::fs::write(&f, "x").unwrap();
    seal_file(&f).unwrap();
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o444
    );
    // A second seal is fine: the flag is cleared before the chmod.
    seal_file(&f).unwrap();
    unseal_file(&f).unwrap();
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!is_immutable(&f));
}

#[test]
fn profile_dirs_lists_app_and_home() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    assert!(l.profile_dirs().is_empty());
    std::fs::create_dir_all(l.profile("vscode", "default").dir).unwrap();
    std::fs::create_dir_all(l.profile("chatgpt-desktop", "work").dir).unwrap();
    std::fs::write(l.profiles.join("chatgpt-desktop/stray"), "").unwrap();
    let names: Vec<(String, String)> = l
        .profile_dirs()
        .into_iter()
        .map(|(a, h, _)| (a, h))
        .collect();
    assert_eq!(
        names,
        vec![
            ("chatgpt-desktop".to_string(), "work".to_string()),
            ("vscode".to_string(), "default".to_string())
        ]
    );
}

#[test]
fn agent_share_hash_matches() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    let install = t.path().join("bin");
    std::fs::create_dir_all(&install).unwrap();
    // No agent in the install: no share, no folder.
    assert_eq!(l.agent_share(&install, "0.9.0").unwrap(), None);
    assert!(!l.agent.exists());
    // The install's binary and its hash file make the folder; the hash
    // file of the share matches the binary of the share.
    let bin = install.join("paddock-guest");
    std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").unwrap();
    let real = sha256_file(&bin).unwrap();
    std::fs::write(install.join("paddock-guest.sha256"), format!("{real}\n")).unwrap();
    let dir = l.agent_share(&install, "0.9.0").unwrap().unwrap();
    assert_eq!(dir, l.root.join("agent/0.9.0"));
    assert_eq!(l.agent_file("0.9.0"), dir.join("paddock-guest"));
    let shared = dir.join("paddock-guest");
    assert_eq!(sha256_file(&shared).unwrap(), real);
    assert_eq!(
        std::fs::read_to_string(dir.join("paddock-guest.sha256"))
            .unwrap()
            .trim(),
        real
    );
    assert_eq!(
        std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(partials_in(&dir).is_empty());
    // The same binary again: nothing is rewritten.
    let before = std::fs::metadata(&shared).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    l.agent_share(&install, "0.9.0").unwrap();
    assert_eq!(
        std::fs::metadata(&shared).unwrap().modified().unwrap(),
        before
    );
    // A new binary in the install replaces the share, with a new hash;
    // without a hash file the hash is computed.
    std::fs::write(&bin, b"#!/bin/sh\nexit 2\n").unwrap();
    std::fs::remove_file(install.join("paddock-guest.sha256")).unwrap();
    l.agent_share(&install, "0.9.0").unwrap();
    let real2 = sha256_file(&bin).unwrap();
    assert_ne!(real2, real);
    assert_eq!(sha256_file(&shared).unwrap(), real2);
    assert_eq!(
        std::fs::read_to_string(dir.join("paddock-guest.sha256"))
            .unwrap()
            .trim(),
        real2
    );
    // A hash file that lies refuses the share and names both hashes.
    std::fs::write(install.join("paddock-guest.sha256"), "00ff\n").unwrap();
    let e = l.agent_share(&install, "0.9.0").unwrap_err().to_string();
    assert!(e.contains("00ff") && e.contains(&real2), "{e}");
    assert_eq!(
        sha256_file(&shared).unwrap(),
        real2,
        "the share is untouched"
    );
    // A torn earlier write goes before the new one.
    std::fs::write(install.join("paddock-guest.sha256"), format!("{real2}\n")).unwrap();
    std::fs::write(partial(&shared), b"half").unwrap();
    std::fs::write(&shared, b"stale").unwrap();
    l.agent_share(&install, "0.9.0").unwrap();
    assert!(partials_in(&dir).is_empty());
    assert_eq!(sha256_file(&shared).unwrap(), real2);
    // Other versions are listed for gc, in order, and this one is not.
    std::fs::create_dir_all(l.agent_dir("0.8.0")).unwrap();
    std::fs::create_dir_all(l.agent_dir("0.7.0")).unwrap();
    assert_eq!(
        l.other_agent_dirs("0.9.0"),
        vec![
            ("0.7.0".to_string(), l.agent_dir("0.7.0")),
            ("0.8.0".to_string(), l.agent_dir("0.8.0"))
        ]
    );
    assert!(l.other_agent_dirs("none").len() == 3);
}
