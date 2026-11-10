use super::*;
use crate::manifest::SetupCfg;
#[cfg(not(target_os = "macos"))]
use crate::storage::subdirs;

/// A temporary folder on the file system of the build folder, which on a
/// developer machine is often btrfs, so the copy-on-write path runs.
fn tmp() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("buildcache-test-")
        .tempdir_in(base)
        .unwrap()
}

fn env_in(dir: &Path) -> HostEnv {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut vars = std::collections::BTreeMap::new();
    if let Ok(p) = std::env::var("PATH") {
        vars.insert("PATH".to_string(), p);
    }
    HostEnv::from_parts(home, "tester".into(), 1000, 1000, vars)
}

/// A repository with a Cargo.toml, and the spec of a worktree sandbox.
fn rust_project(env: &HostEnv, name: &str, id: &str) -> (PathBuf, SandboxSpec) {
    let repo = env.home.join("src").join(name);
    let wt = env.home.join("wt").join(id);
    for d in [&repo, &wt] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    }
    let spec = SandboxSpec::new(
        id.into(),
        "claude",
        &wt,
        env.home.join("sandbox-home"),
        &env.home,
        1000,
        1000,
        "tester",
    );
    (repo, spec)
}

fn mounted(spec: &SandboxSpec) -> Option<PathBuf> {
    spec.mounts
        .iter()
        .find(|m| m.origin == ORIGIN)
        .and_then(|m| m.src.clone())
}

#[cfg(not(target_os = "macos"))]
fn config(env: &HostEnv, text: &str) {
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(env.paddock_config().join("config.toml"), text).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_gets_no_build_cache() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let (repo, mut spec) = rust_project(&env, "app", "claude-1");
    apply_with(&env, &repo, true, &on, &mut spec).unwrap();
    assert!(mounted(&spec).is_none());
}

// The build cache is off on macOS (see apply).
#[cfg(not(target_os = "macos"))]
#[test]
fn only_worktree_runs_of_rust_projects_get_a_target_folder() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let (repo, mut spec) = rust_project(&env, "app", "claude-1");
    apply_with(&env, &repo, false, &on, &mut spec).unwrap();
    assert!(mounted(&spec).is_none(), "not a worktree run");
    let off = SetupCfg {
        shared_build_cache: Some(false),
        ..Default::default()
    };
    apply_with(&env, &repo, true, &off, &mut spec).unwrap();
    assert!(mounted(&spec).is_none(), "the project opted out");
    config(&env, "[storage]\nbuild_cache = false\n");
    apply_with(&env, &repo, true, &on, &mut spec).unwrap();
    assert!(mounted(&spec).is_none(), "the user turned it off");
    config(&env, "");
    std::fs::remove_file(spec.project.join("Cargo.toml")).unwrap();
    apply_with(&env, &repo, true, &on, &mut spec).unwrap();
    assert!(mounted(&spec).is_none(), "no Rust project");

    let (repo, mut spec) = rust_project(&env, "app", "claude-2");
    apply_with(&env, &repo, true, &on, &mut spec).unwrap();
    let w = mounted(&spec).unwrap();
    assert_eq!(w, project_dir(&env, &repo).join("w/claude-2"));
    assert!(w.is_dir());
    let m = spec.mounts.iter().find(|m| m.origin == ORIGIN).unwrap();
    assert_eq!(m.kind, crate::spec::MountKind::Rw);
    assert_eq!(m.dst, spec.home.join(DST));
    assert_eq!(
        spec.env.get("CARGO_TARGET_DIR").map(|v| v.value.clone()),
        Some(spec.home.join(DST).display().to_string())
    );
    let named = std::fs::read_to_string(project_dir(&env, &repo).join("project")).unwrap();
    assert_eq!(named, repo.display().to_string());
}

// The build cache is off on macOS (see apply).
#[cfg(not(target_os = "macos"))]
#[test]
fn a_project_never_sees_the_cache_of_another() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let (a, mut sa) = rust_project(&env, "a", "claude-a");
    let (b, mut sb) = rust_project(&env, "b", "claude-b");
    // b has a base with a secret build output.
    let bdir = project_dir(&env, &b);
    std::fs::create_dir_all(bdir.join("base")).unwrap();
    std::fs::write(bdir.join("base/secret.rlib"), "b only").unwrap();
    apply_with(&env, &a, true, &on, &mut sa).unwrap();
    apply_with(&env, &b, true, &on, &mut sb).unwrap();
    let wa = mounted(&sa).unwrap();
    assert!(wa.starts_with(project_dir(&env, &a)));
    assert!(!wa.join("secret.rlib").exists());
    // Only the sandbox's own folder is mounted, never a base.
    for s in [&sa, &sb] {
        let srcs: Vec<_> = s
            .mounts
            .iter()
            .filter_map(|m| m.src.clone())
            .filter(|p| p.starts_with(root(&env)))
            .collect();
        assert_eq!(srcs.len(), 1, "{srcs:?}");
        assert!(srcs[0].ends_with(format!("w/{}", s.id)));
    }
}

// The build cache is off on macOS (see apply).
#[cfg(not(target_os = "macos"))]
#[test]
fn a_sandbox_starts_from_the_base_and_leaves_its_folder_as_the_next_base() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let (repo, mut s1) = rust_project(&env, "app", "claude-1");
    // The repository's own target folder seeds the first sandbox.
    let own = repo.join("target");
    std::fs::create_dir_all(own.join("debug")).unwrap();
    std::fs::write(
        own.join(crate::storage::buildout::TAG),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::fs::write(own.join("debug/dep.rlib"), "dep v1").unwrap();
    apply_with(&env, &repo, true, &on, &mut s1).unwrap();
    let dir = project_dir(&env, &repo);
    let w1 = mounted(&s1).unwrap();
    let reflinks = can_clone(&dir);
    assert_eq!(w1.join("debug/dep.rlib").exists(), reflinks);

    // The sandbox builds; at its end its folder becomes the base.
    std::fs::create_dir_all(w1.join("debug")).unwrap();
    std::fs::write(w1.join("debug/app"), "app of commit 1").unwrap();
    after_run(&env, &s1);
    assert!(!w1.exists());
    if reflinks {
        assert_eq!(
            std::fs::read_to_string(dir.join("base/debug/app")).unwrap(),
            "app of commit 1"
        );
        // The next sandbox gets its own copy; its writes stay its own.
        let (_, mut s2) = rust_project(&env, "app", "claude-2");
        apply_with(&env, &repo, true, &on, &mut s2).unwrap();
        let w2 = mounted(&s2).unwrap();
        std::fs::write(w2.join("debug/app"), "app of commit 2").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("base/debug/app")).unwrap(),
            "app of commit 1"
        );
        after_run(&env, &s2);
        assert_eq!(
            std::fs::read_to_string(dir.join("base/debug/app")).unwrap(),
            "app of commit 2"
        );
        // The old base waits in the trash for the gc.
        assert_eq!(subdirs(&dir.join("trash")).len(), 1);
    } else {
        // Without reflinks no base is kept: the folder goes to the trash.
        assert!(!dir.join("base").exists());
        assert_eq!(subdirs(&dir.join("trash")).len(), 1);
    }
}

// The build cache is off on macOS (see apply).
#[cfg(not(target_os = "macos"))]
#[test]
fn two_sandboxes_that_end_at_once_both_leave_a_whole_folder() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let mut specs = Vec::new();
    for id in ["claude-1", "claude-2", "claude-3"] {
        let (repo, mut s) = rust_project(&env, "app", id);
        apply_with(&env, &repo, true, &on, &mut s).unwrap();
        let w = mounted(&s).unwrap();
        std::fs::create_dir_all(w.join("debug")).unwrap();
        std::fs::write(w.join("debug/app"), id).unwrap();
        specs.push(s);
    }
    std::thread::scope(|sc| {
        for s in &specs {
            let env = &env;
            sc.spawn(move || after_run(env, s));
        }
    });
    let dir = project_dir(&env, &env.home.join("src/app"));
    assert!(subdirs(&dir.join("w")).is_empty());
    let mut whole = subdirs(&dir.join("trash"));
    if dir.join("base").exists() {
        whole.push(dir.join("base"));
    }
    assert_eq!(whole.len(), 3, "{whole:?}");
    for d in whole {
        let app = std::fs::read_to_string(d.join("debug/app")).unwrap();
        assert!(app.starts_with("claude-"), "{app}");
    }
}

#[test]
fn the_gc_takes_the_trash_leftovers_gone_repositories_and_the_oldest_bases() {
    let t = tmp();
    let env = env_in(t.path());
    let mk = |name: &str, base_age: u64| {
        let repo = env.home.join("src").join(name);
        std::fs::create_dir_all(&repo).unwrap();
        let dir = project_dir(&env, &repo);
        for d in [
            "base",
            "w/claude-run",
            "w/claude-old",
            "w/claude-new",
            "trash/base-1",
        ] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("project"), repo.display().to_string()).unwrap();
        let set = |p: &Path, age: u64| {
            let when = std::time::SystemTime::now() - Duration::from_secs(age);
            std::fs::File::open(p).unwrap().set_modified(when).unwrap();
        };
        set(&dir.join("base"), base_age);
        set(&dir.join("w/claude-old"), 3600);
        set(&dir.join("w/claude-run"), 3600);
        (repo, dir)
    };
    let (_, old) = mk("old", 3 * 86_400);
    let (_, new) = mk("new", 60);
    let (gone_repo, gone) = mk("gone", 60);
    std::fs::remove_dir_all(&gone_repo).unwrap();
    let running: BTreeSet<String> = ["claude-run".to_string()].into();
    // Each base counts as 30 GiB: two of them are over the 40 GiB default.
    let size = |_: &Path| 30 * GIB;
    let plan = plan(
        &env,
        &Settings::default(),
        &running,
        crate::state::now(),
        &size,
    );
    let paths: BTreeSet<PathBuf> = plan.iter().map(|(p, _)| p.clone()).collect();
    let expect: BTreeSet<PathBuf> = [
        gone.clone(),
        old.join("trash/base-1"),
        old.join("w/claude-old"),
        old.join("base"),
        new.join("trash/base-1"),
        new.join("w/claude-old"),
    ]
    .into();
    assert_eq!(paths, expect, "{plan:?}");
    // A running sandbox and a new leftover (maybe still seeding) stay.
    assert!(!paths.contains(&new.join("w/claude-run")));
    assert!(!paths.contains(&new.join("w/claude-new")));
    // No limit: the bases stay.
    let unlimited = Settings {
        build_cache_gib: 0,
        ..Settings::default()
    };
    let plan = super::plan(&env, &unlimited, &running, crate::state::now(), &size);
    assert!(plan.iter().all(|(p, _)| !p.ends_with("base")));
    // The gc may delete there: the folder is one of paddock's own.
    crate::storage::remove_path(&env, &old.join("trash/base-1")).unwrap();
}

// The build cache is off on macOS (see apply).
#[cfg(not(target_os = "macos"))]
#[test]
fn a_folder_with_a_planted_git_directory_never_becomes_a_seed() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let (repo, mut s1) = rust_project(&env, "app", "claude-1");
    apply_with(&env, &repo, true, &on, &mut s1).unwrap();
    let w1 = mounted(&s1).unwrap();
    std::fs::create_dir_all(w1.join(".git")).unwrap();
    after_run(&env, &s1);
    let dir = project_dir(&env, &repo);
    assert!(!dir.join("base").exists());
    assert_eq!(subdirs(&dir.join("trash")).len(), 1);
    // A base that has one (an older paddock kept it) is not cloned.
    std::fs::create_dir_all(dir.join("base/.git")).unwrap();
    std::fs::write(dir.join("base/out.rlib"), "x").unwrap();
    let (_, mut s2) = rust_project(&env, "app", "claude-2");
    apply_with(&env, &repo, true, &on, &mut s2).unwrap();
    let w2 = mounted(&s2).unwrap();
    assert!(!w2.join("out.rlib").exists() && !w2.join(".git").exists());
}

#[test]
fn rust_sandboxes_get_lean_builds_unless_turned_off() {
    let t = tmp();
    let env = env_in(t.path());
    let on = SetupCfg::default();
    let (repo, mut spec) = rust_project(&env, "app", "claude-lean");
    apply_with(&env, &repo, false, &on, &mut spec).unwrap();
    for (k, v) in LEAN_ENV {
        assert_eq!(spec.env.get(*k).map(|e| e.value.as_str()), Some(*v), "{k}");
    }
    let (repo, mut spec) = rust_project(&env, "app", "claude-mine");
    spec.set_env("CARGO_INCREMENTAL", "1", "user");
    apply_with(&env, &repo, false, &on, &mut spec).unwrap();
    assert_eq!(
        spec.env["CARGO_INCREMENTAL"].value, "1",
        "a value of the user stays"
    );
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        env.paddock_config().join("config.toml"),
        "[storage]\nlean_builds = false\n",
    )
    .unwrap();
    let (repo, mut spec) = rust_project(&env, "app", "claude-off");
    apply_with(&env, &repo, false, &on, &mut spec).unwrap();
    assert!(!spec.env.contains_key("CARGO_PROFILE_DEV_DEBUG"));
}

#[test]
fn a_run_in_a_worktree_of_paddock_counts_as_a_worktree_run() {
    let t = tmp();
    let env = env_in(t.path());
    let wt = env.paddock_data().join("worktrees/app-1234/wt-x");
    std::fs::create_dir_all(&wt).unwrap();
    assert!(in_paddock_worktree(&env, &wt));
    assert!(!in_paddock_worktree(
        &env,
        &env.paddock_data().join("worktrees")
    ));
    assert!(!in_paddock_worktree(&env, &env.home));
    assert!(!in_paddock_worktree(&env, &env.home.join("gone")));
}
