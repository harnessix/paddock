use super::*;
use crate::storage::tests::{inventory, setup};
use std::path::Path;

const NOW: u64 = 1_800_000_000;

fn wt(path: &str, merged: Option<bool>, dirty: Option<bool>, last_used: u64) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        repo: Some("/src/repo".into()),
        branch: Some(format!("paddock/{}", &path[path.len() - 1..])),
        unmerged: Some(1),
        dirty,
        merged,
        last_used,
        bytes: 0,
        running: false,
    }
}

fn target(_: &Worktree) -> Vec<PathBuf> {
    vec![PathBuf::from("target")]
}

fn kinds(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| match &i.action {
            Action::Worktree {
                path,
                delete_branch,
                force,
                ..
            } => format!(
                "remove {} branch={delete_branch} force={force}",
                path.display()
            ),
            Action::BuildOutput { worktree, folders } => {
                format!("drop {} {folders:?}", worktree.display())
            }
            Action::Path { path } => format!("path {}", path.display()),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn the_rate_limit_lets_one_pass_run_an_hour() {
    assert!(due(None, NOW));
    assert!(!due(Some(NOW), NOW));
    assert!(!due(Some(NOW - EVERY + 1), NOW));
    assert!(due(Some(NOW - EVERY), NOW));
    // The clock went back by more than an hour: the record is not trusted.
    assert!(due(Some(NOW + 2 * EVERY), NOW));
    assert!(!due(Some(NOW + 10), NOW));
}

#[test]
fn merged_clean_worktrees_go_and_the_others_lose_only_idle_build_output() {
    let s = Settings::default();
    let old = NOW - 3 * DAY;
    let fresh = NOW - 3600;
    let mut inv = inventory(vec![]);
    let mut running = wt("/wt/r", Some(true), Some(false), old);
    running.running = true;
    let mut no_repo = wt("/wt/n", Some(true), Some(false), old);
    no_repo.repo = None;
    inv.worktrees = vec![
        wt("/wt/a", Some(true), Some(false), fresh),
        wt("/wt/b", Some(true), Some(true), old),
        wt("/wt/c", Some(true), None, fresh),
        wt("/wt/d", Some(false), Some(false), old),
        wt("/wt/e", Some(false), Some(false), fresh),
        wt("/wt/f", None, None, old),
        running,
        no_repo,
    ];
    let items = plan_auto(&inv, &s, NOW, &target);
    assert_eq!(
        kinds(&items),
        vec![
            "remove /wt/a branch=true force=false",
            "drop /wt/b [\"target\"]",
            "drop /wt/d [\"target\"]",
            "drop /wt/f [\"target\"]",
            "drop /wt/n [\"target\"]",
        ]
    );
    assert!(items.iter().all(|i| i.selected() && i.tier == Tier::Auto));

    // 0 days turns the build output rule off; no droppable folder, no item.
    let off = Settings {
        worktree_cache_days: 0,
        ..Settings::default()
    };
    assert_eq!(plan_auto(&inv, &off, NOW, &target).len(), 1);
    assert_eq!(plan_auto(&inv, &s, NOW, &|_| vec![]).len(), 1);
}

#[test]
fn old_snapshots_go_and_no_home_ever_goes() {
    let s = Settings::default();
    let mut inv = inventory(vec![]);
    let snap = |p: &str, started: u64, trash: bool| SnapshotDir {
        path: PathBuf::from(p),
        project: Some("/src/repo".into()),
        harness: "claude".into(),
        started,
        bytes: 0,
        project_exists: true,
        trash,
    };
    inv.snapshots = vec![
        snap("/s/repo/claude-old", NOW - 40 * DAY, false),
        snap("/s/repo/claude-new", NOW - 2 * DAY, false),
        snap("/s/repo/claude-live", NOW - 40 * DAY, false),
        snap("/t/trash-1", NOW - 40 * DAY, true),
    ];
    inv.sessions = vec![SessionRecord {
        path: "/x".into(),
        id: "claude-live".into(),
        started: NOW - 40 * DAY,
        ended: None,
        running: true,
        bytes: 0,
    }];
    let home = |p: &str, exists: bool, app: bool, used: u64| {
        let mut h = crate::storage::tests::home(p, "claude");
        h.project = Some("/gone".into());
        h.project_exists = Some(exists);
        h.app = app;
        h.last_used = Some(used);
        h.modified = used;
        h
    };
    let mut busy = home("/h/busy", false, false, NOW - 40 * DAY);
    busy.running = true;
    // A home whose profile is gone is still the user's conversations.
    let mut no_profile = home("/h/no-profile", true, false, NOW - 400 * DAY);
    no_profile.profile = false;
    inv.homes = vec![
        home("/h/gone-old", false, false, NOW - 40 * DAY),
        home("/h/gone-new", false, false, NOW - 5 * DAY),
        home("/h/app", false, true, NOW - 40 * DAY),
        home("/h/live", true, false, NOW - 400 * DAY),
        no_profile,
        busy,
    ];
    let items = plan_auto(&inv, &s, NOW, &target);
    let k = kinds(&items);
    assert_eq!(
        k,
        vec!["path /s/repo/claude-old", "path /t/trash-1"],
        "{k:?}"
    );
    assert!(items[1].warning.as_deref().unwrap().contains("undo"));
    // 0 turns the rule off.
    let off = Settings {
        snapshot_days: 0,
        ..Settings::default()
    };
    assert!(plan_auto(&inv, &off, NOW, &target).is_empty());
}

/// Set the change time of a worktree and its git files, so it looks unused.
fn age(path: &Path, t: u64) {
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(t);
    let l = crate::hostgit::Layout::discover(path).unwrap().unwrap();
    for f in [
        path.to_path_buf(),
        l.git_dir.join("index"),
        l.git_dir.join("HEAD"),
        l.git_dir.join("logs/HEAD"),
    ] {
        if let Ok(h) = std::fs::File::open(&f) {
            h.set_modified(when).unwrap();
        }
    }
}

fn repo(env: &HostEnv) -> PathBuf {
    use crate::storage::merged::tests::git;
    let repo = env.home.join("src/repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), "/target\n").unwrap();
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    crate::state::record_recent(env, &repo, "claude").unwrap();
    repo
}

fn tag(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(crate::storage::buildout::TAG),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
}

#[test]
fn a_merged_worktree_is_removed_with_its_branch_by_the_real_steps() {
    use crate::storage::merged::tests::git;
    let (_t, env) = setup();
    let repo = repo(&env);
    let merged = crate::build::create_worktree(&env, &repo, None).unwrap();
    std::fs::write(merged.path.join("b"), "b\n").unwrap();
    git(&merged.path, &["add", "b"]);
    git(&merged.path, &["commit", "-q", "-m", "b"]);
    let pick = crate::build::create_worktree(&env, &repo, None).unwrap();
    std::fs::write(pick.path.join("c"), "c\n").unwrap();
    git(&pick.path, &["add", "c"]);
    git(&pick.path, &["commit", "-q", "-m", "c"]);
    let kept = crate::build::create_worktree(&env, &repo, None).unwrap();
    git(&kept.path, &["commit", "-q", "--allow-empty", "-m", "own"]);
    tag(&kept.path.join("target"));
    std::fs::write(kept.path.join("target/big.rlib"), vec![0u8; 4096]).unwrap();
    std::fs::write(kept.path.join("notes.txt"), "mine\n").unwrap();
    // main gets one branch by a merge and the other by a cherry-pick.
    git(&repo, &["merge", "-q", "--no-edit", &merged.branch]);
    let tip = git(&pick.path, &["rev-parse", "HEAD"]);
    git(&repo, &["cherry-pick", &tip]);
    age(&kept.path, crate::state::now() - 3 * DAY);

    let host = Host::real(&env, &NoContainers);
    let mut inv = scan(&env, &host, false);
    assert_eq!(inv.worktrees.len(), 3);
    for w in &mut inv.worktrees {
        // The sandboxed `git status` needs bubblewrap; its answer here: clean.
        w.dirty = Some(false);
    }
    let by = |p: &Path| inv.worktrees.iter().find(|w| w.path == p).unwrap().clone();
    assert_eq!(by(&merged.path).merged, Some(true));
    assert_eq!(by(&pick.path).merged, Some(true));
    assert_eq!(by(&kept.path).merged, Some(false));
    // A day later: the merged worktrees are past the grace time.
    let later = crate::state::now() + DAY;
    let items = plan_auto(&inv, &Settings::default(), later, &|w| {
        crate::storage::buildout::droppable(&w.path)
    });
    assert_eq!(items.len(), 3, "{:?}", kinds(&items));
    let removed = apply(&env, &NoContainers, &items);
    assert!(removed.iter().all(|r| r.error.is_none()), "{removed:?}");

    assert!(!merged.path.exists() && !pick.path.exists());
    let branches = git(&repo, &["branch", "--format=%(refname:short)"]);
    assert!(!branches.contains(&merged.branch), "{branches}");
    assert!(!branches.contains(&pick.branch), "{branches}");
    assert!(branches.contains(&kept.branch), "{branches}");
    // The kept worktree lost only its tagged, ignored build folder.
    assert!(kept.path.exists());
    assert!(!kept.path.join("target").exists());
    assert!(kept.path.join("notes.txt").exists());
    assert_eq!(git(&kept.path, &["log", "-1", "--format=%s"]), "own");
}

#[test]
fn the_pass_runs_once_an_hour_records_its_status_and_waits_for_the_lock() {
    let (_t, env) = setup();
    let s = Settings::default();
    let first = match pass(&env, &s, false, None) {
        Outcome::Done(st) => st,
        Outcome::Skipped(w) => panic!("skipped: {w}"),
    };
    assert!(first.ended.is_some());
    assert_eq!(load(&env).unwrap(), first);
    // Folder sizes come with the background pass.
    assert!(first.sizes_at > 0);
    match pass(&env, &s, false, None) {
        Outcome::Skipped(w) => assert!(w.contains("less than an hour"), "{w}"),
        Outcome::Done(_) => panic!("ran twice in an hour"),
    }
    // Forced: it runs again.
    assert!(matches!(pass(&env, &s, true, None), Outcome::Done(_)));
    // Another pass holds the lock: skipped, also when forced.
    let lock = try_lock(&env.paddock_state().join("storage-auto.lock")).unwrap();
    match pass(&env, &s, true, None) {
        Outcome::Skipped(w) => assert!(w.contains("another cleanup"), "{w}"),
        Outcome::Done(_) => panic!("ran beside another pass"),
    }
    drop(lock);
    assert!(matches!(pass(&env, &s, true, None), Outcome::Done(_)));
}

#[test]
fn a_crashed_pass_still_counts_for_the_rate_limit() {
    let (_t, env) = setup();
    // A pass that started and never wrote its end.
    let crashed = Status {
        started: crate::state::now() - 60,
        pid: 1,
        ..Default::default()
    };
    save(&env, &crashed);
    assert!(matches!(
        pass(&env, &Settings::default(), false, None),
        Outcome::Skipped(_)
    ));
    // A broken status file is no reason to stop: the pass runs.
    std::fs::write(status_file(&env), "{not json").unwrap();
    assert!(matches!(
        pass(&env, &Settings::default(), false, None),
        Outcome::Done(_)
    ));
}

#[test]
fn an_expired_deadline_leaves_the_items_for_the_next_pass() {
    let (_t, env) = setup();
    let repo = repo(&env);
    let w = crate::build::create_worktree(&env, &repo, None).unwrap();
    crate::storage::merged::tests::git(&w.path, &["commit", "-q", "--allow-empty", "-m", "x"]);
    tag(&w.path.join("target"));
    age(&w.path, crate::state::now() - 3 * DAY);
    let past = Instant::now() - std::time::Duration::from_secs(1);
    let st = match pass(&env, &Settings::default(), true, Some(past)) {
        Outcome::Done(st) => st,
        Outcome::Skipped(w) => panic!("{w}"),
    };
    assert!(st.removed.is_empty(), "{st:?}");
    assert!(st.errors.iter().any(|e| e.contains("time limit")), "{st:?}");
    assert!(w.path.join("target").exists());
    // The next pass without a deadline drops it.
    let st = match pass(&env, &Settings::default(), true, None) {
        Outcome::Done(st) => st,
        Outcome::Skipped(w) => panic!("{w}"),
    };
    assert_eq!(st.removed.len(), 1, "{st:?}");
    assert!(!w.path.join("target").exists());
}

#[test]
fn a_new_worktree_waits_for_the_grace_time() {
    let mut inv = inventory(vec![]);
    inv.worktrees = vec![wt("/wt/a", Some(true), Some(false), NOW - 60)];
    assert!(plan_auto(&inv, &Settings::default(), NOW, &target).is_empty());
    inv.worktrees[0].last_used = NOW - GRACE;
    assert_eq!(plan_auto(&inv, &Settings::default(), NOW, &target).len(), 1);
}

#[test]
fn a_worktree_that_another_process_leases_is_left_alone() {
    use crate::storage::merged::tests::git;
    let (_t, env) = setup();
    let repo = repo(&env);
    let w = crate::build::create_worktree(&env, &repo, None).unwrap();
    // This process made it: its own lease does not count.
    assert!(!crate::storage::worktree_in_use(&env, &w.path));
    // Another process that holds the lease: a run that is still starting.
    let lease = env.paddock_run().join("worktree-leases").join(format!(
        "{}--{}",
        name(w.path.parent().unwrap()),
        name(&w.path)
    ));
    assert!(lease.is_file(), "{}", lease.display());
    // perl holds a shared flock(2) on Linux and macOS alike (macOS has no
    // flock(1)).
    let mut holder = std::process::Command::new("perl")
        .args([
            "-MFcntl=:flock",
            "-e",
            "open(my $f, '<', $ARGV[0]) or die; flock($f, LOCK_SH) or die; sleep 30",
        ])
        .arg(&lease)
        .spawn()
        .unwrap();
    // The lease of this process is dropped by hand, as at its exit.
    crate::storage::drop_leases_for_test(&env, &w.path);
    let start = Instant::now();
    while !crate::storage::worktree_in_use(&env, &w.path) {
        assert!(start.elapsed().as_secs() < 10, "the holder did not lock");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let host = Host::real(&env, &NoContainers);
    let mut inv = scan(&env, &host, false);
    assert!(inv.worktrees[0].running);
    inv.worktrees[0].dirty = Some(false);
    let later = crate::state::now() + DAY;
    assert!(plan_auto(&inv, &Settings::default(), later, &target).is_empty());
    // Also an item from an older scan is refused at the removal.
    inv.worktrees[0].running = false;
    let items = plan_auto(&inv, &Settings::default(), later, &target);
    assert_eq!(items.len(), 1);
    let r = apply(&env, &NoContainers, &items);
    assert!(r[0].error.as_deref().unwrap().contains("skipped"), "{r:?}");
    assert!(w.path.exists());
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(!crate::storage::worktree_in_use(&env, &w.path));
    let r = apply(&env, &NoContainers, &items);
    assert!(r[0].error.is_none(), "{r:?}");
    assert!(!w.path.exists());
    assert!(!git(&repo, &["branch"]).contains(&w.branch));
}
