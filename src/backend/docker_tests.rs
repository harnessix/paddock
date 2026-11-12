use super::*;

#[test]
fn mount_points_never_follow_links_planted_in_the_home() {
    use crate::spec::Mount;
    use std::os::unix::fs::symlink;
    let t = tempfile::tempdir().unwrap();
    let (home_src, outside) = (t.path().join("hs"), t.path().join("outside"));
    std::fs::create_dir_all(&home_src).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), "host").unwrap();
    let home = Path::new("/home/demo");
    let spec = |mounts: Vec<Mount>| {
        let mut s = SandboxSpec::new(
            "t".into(),
            "claude",
            Path::new("/w"),
            home_src.clone(),
            home,
            1000,
            1000,
            "demo",
        );
        s.mounts = mounts;
        s
    };
    // Plain mount points are made as folders and new files.
    let ok = spec(vec![
        Mount::tmpfs(home.join(".cache/x"), "t"),
        Mount::mask(home.join(".config/a/token"), false, "t"),
    ]);
    precreate_mount_points(&ok).unwrap();
    precreate_mount_points(&ok).unwrap();
    assert!(home_src.join(".cache/x").is_dir());
    assert!(home_src.join(".config/a/token").is_file());

    // A planted folder link creates nothing outside.
    symlink(&outside, home_src.join(".cargo")).unwrap();
    let s = spec(vec![Mount::tmpfs(home.join(".cargo/registry"), "t")]);
    assert!(precreate_mount_points(&s).is_err());
    assert!(!outside.join("registry").exists());

    // A dangling file link creates no file anywhere; a link to a host
    // file does not truncate it.
    symlink(outside.join("new"), home_src.join("dangling")).unwrap();
    symlink(outside.join("keep"), home_src.join("keep")).unwrap();
    for n in ["dangling", "keep"] {
        let s = spec(vec![Mount::mask(home.join(n), false, "t")]);
        assert!(precreate_mount_points(&s).is_err(), "{n}");
    }
    assert!(!outside.join("new").exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("keep")).unwrap(),
        "host"
    );
}

#[test]
fn image_variants_share_layers() {
    let cfg = |gui, app, keyring| ImageCfg {
        image: None,
        packages: vec![],
        gui,
        app,
        keyring,
    };
    // Hosts that paddock cannot map to a base image have nothing to share.
    let Ok(plain) = dockerfile(&cfg(false, false, false)) else {
        return;
    };
    let desktop = dockerfile(&cfg(true, false, false)).unwrap();
    // Every desktop variant is one image.
    assert_eq!(desktop, dockerfile(&cfg(false, true, false)).unwrap());
    assert_eq!(desktop, dockerfile(&cfg(false, false, true)).unwrap());
    assert_eq!(desktop, dockerfile(&cfg(true, true, true)).unwrap());
    // The desktop image starts with the plain image's core layer.
    let core = plain.lines().nth(1).unwrap();
    assert!(core.starts_with("RUN : paddock-base core"));
    assert_eq!(desktop.lines().nth(1).unwrap(), core);
    assert!(desktop.contains(": paddock-base desktop"));
    let with_pkgs = dockerfile(&ImageCfg {
        packages: vec!["jq".into()],
        ..cfg(false, false, false)
    })
    .unwrap();
    assert_eq!(with_pkgs.lines().nth(1).unwrap(), core);
    assert!(with_pkgs.contains(": paddock-base packages"));
}

#[test]
fn derived_seccomp_blocks_only_new_user_namespaces() {
    let default = serde_json::json!({
        "defaultAction": "SCMP_ACT_ERRNO",
        "syscalls": [
            { "names": ["read", "clone", "clone3", "unshare", "setns"], "action": "SCMP_ACT_ALLOW" },
            { "names": ["clone3"], "action": "SCMP_ACT_ALLOW" }
        ]
    });
    let d = derive_seccomp(&default).unwrap();
    let rules = d["syscalls"].as_array().unwrap();
    let first: Vec<&str> = rules[0]["names"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert_eq!(first, ["read", "setns"]);
    assert_eq!(
        rules.len(),
        4,
        "the empty clone3 rule is dropped, three rules are added"
    );
    let clone = rules.iter().find(|r| r["names"][0] == "clone").unwrap();
    assert_eq!(clone["args"][0]["value"], CLONE_NEWUSER);
    assert_eq!(clone["args"][0]["valueTwo"], 0);
    let clone3 = rules.iter().find(|r| r["names"][0] == "clone3").unwrap();
    assert_eq!(clone3["action"], "SCMP_ACT_ERRNO");
}
