use super::*;
use std::fs;

const HOME: &str = "/Users/me";

fn root(p: &str, ro: bool) -> Root {
    Root {
        path: PathBuf::from(p),
        read_only: ro,
        unprotected: false,
    }
}

fn profile(t: &Scratch) -> Profile {
    Profile::new(t.path().join("profile"))
}

/// A scratch folder that is the home of the test too. The temporary
/// folder of the system (`/tmp`, `/var/folders`) is one that the builder
/// refuses, but a folder in the home is the user's wherever the home is.
/// The path is the real one, because the shares carry the real path.
pub(crate) struct Scratch {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Scratch {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn home(&self) -> &Path {
        &self.path
    }
}

pub(crate) fn scratch() -> Scratch {
    let dir = tempfile::tempdir().unwrap();
    let path = fs::canonicalize(dir.path()).unwrap();
    Scratch { _dir: dir, path }
}

/// Item 1 of the security checklist: each listed root, a parent of the
/// home, the protected folders of the home, and the reason of each.
#[test]
fn refuses_every_listed_root() {
    let t = scratch();
    let p = profile(&t);
    let home = Path::new(HOME);
    let listed = [
        ("/", ShareWhy::Root),
        (HOME, ShareWhy::Home),
        ("/Users", ShareWhy::AllHomes),
        ("/Volumes", ShareWhy::Volumes),
        ("/private", ShareWhy::SystemFiles),
        ("/tmp", ShareWhy::SystemFiles),
        ("/var", ShareWhy::SystemFiles),
        ("/etc", ShareWhy::SystemFiles),
        ("/Library", ShareWhy::SystemApps),
        ("/System", ShareWhy::SystemApps),
        ("/Applications", ShareWhy::SystemApps),
        ("/Users/me/.ssh", ShareWhy::Protected),
        ("/Users/me/Library", ShareWhy::Protected),
        ("/Users/me/Library/Keychains", ShareWhy::Protected),
        ("/Users/me/.gnupg", ShareWhy::Protected),
        ("/Users/me/.local/share/keyrings", ShareWhy::Protected),
        ("/Users/me/src/..", ShareWhy::Home),
        ("/Users/me/src/../..", ShareWhy::AllHomes),
        ("/private/tmp/x", ShareWhy::SystemFiles),
        ("/System/Library", ShareWhy::SystemApps),
        ("relative/folder", ShareWhy::Relative),
    ];
    for (path, why) in listed {
        assert_eq!(refusal(Path::new(path), home), Some(why), "{path}");
        let e = build("vscode", &[root(path, false)], &p, None, home, &[], false)
            .unwrap_err()
            .to_string();
        let shown = normalize(Path::new(path));
        assert!(e.contains(&shown.display().to_string()), "{path}: {e}");
        assert!(e.contains(&text::share_reason(why)), "{path}: {e}");
        assert!(e.contains("paddock app run vscode"), "{e}");
    }
    // A folder that contains the home on another layout.
    assert_eq!(
        refusal(Path::new("/home"), Path::new("/home/demo")),
        Some(ShareWhy::ContainsHome)
    );
    // Not refused: a project folder, another user's folder, an external disk.
    for ok in [
        "/Users/me/src/web",
        "/Users/other/src",
        "/Volumes/External/project",
        "/opt/work",
    ] {
        assert_eq!(refusal(Path::new(ok), home), None, "{ok}");
    }
    // Every refusal of a command comes back at once, one line each.
    let e = build(
        "vscode",
        &[root("/", false), root("/etc", false)],
        &p,
        None,
        home,
        &[],
        false,
    )
    .unwrap_err()
    .to_string();
    assert_eq!(e.lines().count(), 2, "{e}");
    // A root that does not exist is named.
    let e = build(
        "vscode",
        &[root("/Users/me/src/missing", false)],
        &p,
        None,
        home,
        &[],
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("/Users/me/src/missing"), "{e}");
}

/// Item 2 of the security checklist: with no roots, the four folders of
/// paddock only; with two roots, `ws-1` and `ws-2`, `--ro` on the second.
#[test]
fn only_named_roots() {
    let t = scratch();
    let p = profile(&t);
    let agent = t.path().join("agent");
    fs::create_dir_all(&agent).unwrap();
    let b = build("a", &[], &p, Some(&agent), t.home(), &[], false).unwrap();
    let names: Vec<&str> = b.shares.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, RESERVED.to_vec());
    assert!(b.warnings.is_empty());
    let by_name = |n: &str| b.shares.iter().find(|s| s.name == n).unwrap().clone();
    assert_eq!(by_name("drop").host, p.drop_dir());
    assert!(!by_name("drop").read_only);
    assert_eq!(by_name("downloads").host, p.downloads());
    assert!(!by_name("downloads").read_only);
    assert_eq!(by_name("apps").host, p.apps());
    assert!(by_name("apps").read_only);
    assert_eq!(by_name("agent").host, agent);
    assert!(by_name("agent").read_only);
    // Without an agent folder, three.
    let b = build("a", &[], &p, None, t.home(), &[], false).unwrap();
    assert_eq!(b.shares.len(), 3);

    let web = t.path().join("web");
    let api = t.path().join("api");
    fs::create_dir_all(&web).unwrap();
    fs::create_dir_all(&api).unwrap();
    let b = build(
        "a",
        &[
            root(web.to_str().unwrap(), false),
            root(api.to_str().unwrap(), true),
        ],
        &p,
        None,
        t.home(),
        &[],
        false,
    )
    .unwrap();
    assert_eq!(b.shares[0].name, "ws-1");
    assert_eq!(b.shares[0].host, web);
    assert!(!b.shares[0].read_only);
    assert_eq!(b.shares[1].name, "ws-2");
    assert_eq!(b.shares[1].host, api);
    assert!(b.shares[1].read_only);
    assert_eq!(b.shares[2].name, "drop");
    assert!(b.shares[0].is_workspace() && !b.shares[2].is_workspace());
    // The guest paths.
    assert_eq!(
        b.shares[0].guest_path(),
        PathBuf::from("/Volumes/My Shared Files/ws-1")
    );
    assert_eq!(
        b.shares[2].guest_path(),
        PathBuf::from("/Volumes/My Shared Files/drop")
    );
    // The same folder twice is one share.
    let b = build(
        "a",
        &[
            root(web.to_str().unwrap(), false),
            root(web.to_str().unwrap(), true),
        ],
        &p,
        None,
        t.home(),
        &[],
        false,
    )
    .unwrap();
    assert_eq!(b.shares.len(), 4);
}

/// A root named `agent` or `drop` is `ws-<n>`, never the reserved share.
#[test]
fn reserved_names_never_collide() {
    let t = scratch();
    let p = profile(&t);
    let mut roots = Vec::new();
    for n in RESERVED {
        let d = t.path().join("work").join(n);
        fs::create_dir_all(&d).unwrap();
        roots.push(root(d.to_str().unwrap(), false));
    }
    let b = build("a", &roots, &p, Some(t.path()), t.home(), &[], false).unwrap();
    let names: Vec<&str> = b.shares.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "ws-1",
            "ws-2",
            "ws-3",
            "ws-4",
            "drop",
            "downloads",
            "apps",
            "agent"
        ]
    );
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "every name is unique");
    assert_eq!(b.shares[0].host, t.path().join("work/drop"));
    assert_eq!(b.shares[4].host, p.drop_dir());
    // The translation of a path both ways.
    let guest = Path::new("/Volumes/My Shared Files/ws-1/a/b.txt");
    assert_eq!(
        to_host(&b.shares, guest),
        Some(t.path().join("work/drop/a/b.txt"))
    );
    assert_eq!(
        to_guest(&b.shares, &t.path().join("work/drop/a/b.txt")),
        Some(guest.to_path_buf())
    );
    assert_eq!(
        to_guest(&b.shares, &p.drop_dir().join("1-1/x.png")),
        Some(PathBuf::from("/Volumes/My Shared Files/drop/1-1/x.png"))
    );
    assert_eq!(to_host(&b.shares, Path::new("/Volumes/Other/x")), None);
    assert_eq!(
        to_host(&b.shares, Path::new("/Volumes/My Shared Files")),
        None
    );
    assert_eq!(
        to_host(
            &b.shares,
            Path::new("/Volumes/My Shared Files/ws-1/../../Other/x")
        ),
        None,
        "a dot-dot does not leave the shares"
    );
    assert_eq!(to_guest(&b.shares, Path::new("/Users/me/other")), None);
}

/// A `.env` (or a key file) in a root stops the start with the file
/// named, unless unprotected_workspaces, which warns instead.
#[test]
fn env_file_stops_unless_unprotected() {
    let t = scratch();
    let p = profile(&t);
    let web = t.path().join("web");
    fs::create_dir_all(web.join("deep/er")).unwrap();
    fs::write(web.join(".env"), "KEY=1").unwrap();
    fs::write(web.join("deep/er/id_rsa"), "key").unwrap();
    fs::write(web.join(".env.example"), "KEY=").unwrap();
    let roots = [root(web.to_str().unwrap(), false)];
    let e = build("vscode", &roots, &p, None, t.home(), &[], false)
        .unwrap_err()
        .to_string();
    assert_eq!(e.lines().count(), 2, "{e}");
    assert!(e.contains(&web.join(".env").display().to_string()), "{e}");
    assert!(
        e.contains(&web.join("deep/er/id_rsa").display().to_string()),
        "{e}"
    );
    assert!(!e.contains(".env.example"), "{e}");
    assert!(e.contains("[apps.vscode]") && e.contains("unprotected_workspaces"));
    let b = build("vscode", &roots, &p, None, t.home(), &[], true).unwrap();
    assert_eq!(b.shares.len(), 4);
    assert_eq!(b.warnings.len(), 2);
    assert!(b.warnings[0].contains("unprotected_workspaces is on for vscode"));
    assert!(b.warnings[0].contains(".env"), "{:?}", b.warnings);
    // A clean folder gives no warning either way.
    let clean = t.path().join("clean");
    fs::create_dir_all(&clean).unwrap();
    let b = build(
        "vscode",
        &[root(clean.to_str().unwrap(), false)],
        &p,
        None,
        t.home(),
        &[],
        true,
    )
    .unwrap();
    assert!(b.warnings.is_empty());
    assert_eq!(normalize(Path::new("/a/b/")), PathBuf::from("/a/b"));
    assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    assert_eq!(normalize(Path::new("/..")), PathBuf::from("/"));
}

/// A folder in the home is the user's even where the home sits under a
/// system folder; a home that is a system folder gets no such pass; and
/// a symlink in the home that points into a system folder is refused by
/// its real path.
#[test]
fn the_home_and_the_real_path_decide() {
    let odd = Path::new("/private/var/users/me");
    assert_eq!(refusal(Path::new("/private/var/users/me/src"), odd), None);
    assert_eq!(
        refusal(Path::new("/private/var/users/me/.ssh/keys"), odd),
        Some(ShareWhy::Protected)
    );
    assert_eq!(
        refusal(Path::new("/private/var/users/other"), odd),
        Some(ShareWhy::SystemFiles)
    );
    assert_eq!(
        refusal(Path::new("/tmp/x"), Path::new("/tmp")),
        Some(ShareWhy::SystemFiles)
    );

    let t = scratch();
    let p = profile(&t);
    let link = t.path().join("etc-link");
    std::os::unix::fs::symlink("/etc", &link).unwrap();
    let e = build(
        "vscode",
        &[root(link.to_str().unwrap(), false)],
        &p,
        None,
        t.home(),
        &[],
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains(&text::share_reason(ShareWhy::SystemFiles)),
        "{e}"
    );
    // A link to a project folder in the home shares the real folder, once.
    let web = t.path().join("web");
    fs::create_dir_all(&web).unwrap();
    let alias = t.path().join("web-link");
    std::os::unix::fs::symlink(&web, &alias).unwrap();
    let b = build(
        "a",
        &[
            root(alias.to_str().unwrap(), false),
            root(web.to_str().unwrap(), false),
        ],
        &p,
        None,
        t.home(),
        &[],
        false,
    )
    .unwrap();
    assert_eq!(b.shares[0].host, web);
    assert_eq!(b.shares.len(), 4, "{:?}", b.shares);
}

#[test]
fn saved_workspace_cannot_share_paddock_data() {
    let t = scratch();
    let p = Profile::new(t.path().join("p"));
    let protected = t.home().join("paddock-data");
    std::fs::create_dir_all(&protected).unwrap();
    let roots = [Root {
        path: protected.clone(),
        read_only: true,
        unprotected: false,
    }];
    assert!(build("app", &roots, &p, None, t.home(), &[protected], false).is_err());
}
