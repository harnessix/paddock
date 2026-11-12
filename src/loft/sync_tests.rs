use super::*;
use crate::loft::{host, mount_with, Loft, NOTES};
use crate::spec::SandboxSpec;
use std::process::Command;

pub(super) fn temp_env() -> (tempfile::TempDir, HostEnv) {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        t.path().join("home"),
        "tester".into(),
        1000,
        1000,
        Default::default(),
    );
    (t, env)
}

pub(super) fn write_cfg(env: &HostEnv, text: &str) {
    let d = env.paddock_config();
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("config.toml"), text).unwrap();
}

pub(super) fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@localhost",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A repository with one commit, and a linked worktree of it.
pub(super) fn repo_and_worktree(t: &Path) -> (PathBuf, PathBuf) {
    let repo = t.join("app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README"), "x\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let wt = t.join("app-wt");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
    );
    (
        std::fs::canonicalize(repo).unwrap(),
        std::fs::canonicalize(wt).unwrap(),
    )
}

pub(super) fn spec(project: &Path, tmp: &Path) -> SandboxSpec {
    SandboxSpec::new(
        "s-1".into(),
        "fake",
        project,
        tmp.join("home/h"),
        &tmp.join("home"),
        1000,
        1000,
        "tester",
    )
}

/// The clock of the pending deletes goes DELETE_WAIT forward.
pub(super) fn later() {
    LATER.with(|l| l.set(l.get() + DELETE_WAIT.as_millis() as u64));
}

/// A delete in the work tree waits for a later sync; then it goes.
pub(super) fn sync_delete(loft: &Loft, w: &Path, who: &str, gone: &[&str], at: &str) {
    let r = sync(loft, w, who).unwrap();
    assert!(r.deleted.is_empty(), "{at}: {r:?}");
    assert_eq!(r.waiting, gone, "{at}");
    for g in gone {
        assert!(loft.notes().join(g).is_file(), "{at}: {g} went at once");
    }
    later();
    let r = sync(loft, w, who).unwrap();
    assert_eq!(r.deleted, gone, "{at}");
    assert!(r.waiting.is_empty(), "{at}: {r:?}");
}

pub(super) fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub(super) fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// All files below `d` with their content, temporary files included.
pub(super) fn tree(d: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(d: &Path, pre: &str, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = std::fs::read_dir(d) else {
            return;
        };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                walk(&p, &format!("{pre}{n}/"), out);
            } else if m.is_file() {
                out.insert(format!("{pre}{n}"), read(&p));
            }
        }
    }
    walk(d, "", &mut out);
    out
}

/// Two work trees in sync mode with one loft.
pub(super) fn two(t: &Path, env: &HostEnv) -> (Loft, PathBuf, PathBuf) {
    write_cfg(env, "[loft]\nmode = \"sync\"\n");
    let (repo, wt) = repo_and_worktree(t);
    host::init(env, &repo).unwrap();
    apply_mode(env, &wt, "test").unwrap();
    let loft = Loft::of(env, &repo);
    assert_eq!(loft, Loft::of(env, &wt), "one loft for the repository");
    assert!(synced(&loft, &repo) && synced(&loft, &wt));
    (loft, repo, wt)
}

#[test]
fn a_change_on_each_side_reaches_the_others() {
    let (t, env) = temp_env();
    let (loft, a, b) = two(t.path(), &env);
    // In the first work tree.
    write(&a.join(".paddock/plans/one.md"), "one");
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.pushed, vec!["plans/one.md"]);
    assert_eq!(read(&loft.notes().join("plans/one.md")), "one");
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.pulled, vec!["plans/one.md"]);
    assert_eq!(read(&b.join(".paddock/plans/one.md")), "one");
    // In the second work tree.
    write(&b.join(".paddock/plans/one.md"), "one, by b");
    sync(&loft, &b, "b").unwrap();
    sync(&loft, &a, "a").unwrap();
    assert_eq!(read(&a.join(".paddock/plans/one.md")), "one, by b");
    // In the loft itself (a link-mode tool or the host).
    write(&loft.notes().join("tickets/T1.md"), "ticket");
    for w in [&a, &b] {
        sync(&loft, w, "x").unwrap();
        assert_eq!(read(&w.join(".paddock/tickets/T1.md")), "ticket");
    }
    // A delete in one work tree reaches the loft and the other one, and
    // the empty folder goes too.
    std::fs::remove_file(a.join(".paddock/plans/one.md")).unwrap();
    sync_delete(&loft, &a, "a", &["plans/one.md"], "a");
    assert!(!loft.notes().join("plans").exists());
    sync(&loft, &b, "b").unwrap();
    assert!(!b.join(".paddock/plans/one.md").exists());
    // Nothing left to do: all three are the same.
    for w in [&a, &b] {
        assert!(!sync(&loft, w, "x").unwrap().changed());
        assert!(pending(&loft, w).unwrap().is_empty());
        let mut local = tree(&w.join(NOTES));
        local.retain(|k, _| !k.starts_with("inbox/") && k != ".gitignore" && k != MARK);
        assert_eq!(local, tree(&loft.notes()));
    }
}

#[test]
fn a_conflict_keeps_both_versions() {
    let (t, env) = temp_env();
    let (loft, a, b) = two(t.path(), &env);
    write(&a.join(".paddock/plan.md"), "base");
    sync(&loft, &a, "a").unwrap();
    sync(&loft, &b, "b").unwrap();
    write(&a.join(".paddock/plan.md"), "from a");
    write(&b.join(".paddock/plan.md"), "from b");
    sync(&loft, &a, "a").unwrap();
    assert_eq!(
        pending(&loft, &b).unwrap(),
        vec!["plan.md (changed on both sides)"]
    );
    let r = sync(&loft, &b, "run-b").unwrap();
    assert_eq!(r.conflicts.len(), 1, "{r:?}");
    let (p, c) = &r.conflicts[0];
    assert_eq!(p, "plan.md");
    assert!(c.starts_with("plan.md.conflict-run-b-"), "{c}");
    // The loft version stays at the name, the other one is next to it, on
    // both sides.
    assert_eq!(read(&loft.notes().join("plan.md")), "from a");
    assert_eq!(read(&loft.notes().join(c)), "from b");
    assert_eq!(read(&b.join(".paddock/plan.md")), "from a");
    assert_eq!(read(&b.join(".paddock").join(c)), "from b");
    assert_eq!(conflicts(&loft), vec![c.clone()]);
    sync(&loft, &a, "a").unwrap();
    assert_eq!(read(&a.join(".paddock").join(c)), "from b");
    // Two new files of the same name with different content: also both.
    write(&a.join(".paddock/new.md"), "a");
    write(&loft.notes().join("new.md"), "loft");
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(read(&a.join(".paddock/new.md")), "loft");
    assert_eq!(read(&a.join(".paddock").join(&r.conflicts[0].1)), "a");
    // The status lists the open conflicts.
    let s = host::status(&env, &a).unwrap();
    assert!(s.contains("conflicts 2 open"), "{s}");
    assert!(s.contains("mode      sync"), "{s}");
}

#[test]
fn a_delete_against_a_change_keeps_the_file() {
    let (t, env) = temp_env();
    let (loft, a, b) = two(t.path(), &env);
    write(&a.join(".paddock/x.md"), "v1");
    write(&a.join(".paddock/y.md"), "v1");
    sync(&loft, &a, "a").unwrap();
    sync(&loft, &b, "b").unwrap();
    // a deletes x, b changes it.
    std::fs::remove_file(a.join(".paddock/x.md")).unwrap();
    write(&b.join(".paddock/x.md"), "v2");
    sync(&loft, &b, "b").unwrap();
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.kept, vec!["x.md"]);
    assert_eq!(read(&a.join(".paddock/x.md")), "v2");
    assert_eq!(read(&loft.notes().join("x.md")), "v2");
    // The loft deletes y, a changes it.
    std::fs::remove_file(loft.notes().join("y.md")).unwrap();
    write(&a.join(".paddock/y.md"), "v2");
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.kept, vec!["y.md"]);
    assert_eq!(read(&loft.notes().join("y.md")), "v2");
    // b still had y v1, unchanged: it gets v2.
    sync(&loft, &b, "b").unwrap();
    assert_eq!(read(&b.join(".paddock/y.md")), "v2");
}

#[test]
fn a_crash_during_a_sync_leaves_no_half_file_and_repeats_cleanly() {
    // Every step of a sync with every kind of action, one crash per step.
    for at in 0..200 {
        let (t, env) = temp_env();
        let (loft, a, b) = two(t.path(), &env);
        write(&a.join(".paddock/same.md"), "s");
        write(&a.join(".paddock/both.md"), "base");
        write(&a.join(".paddock/gone.md"), "g");
        write(&a.join(".paddock/kept.md"), "k");
        sync(&loft, &a, "a").unwrap();
        sync(&loft, &b, "b").unwrap();
        write(&a.join(".paddock/new.md"), "new in a");
        write(&a.join(".paddock/deep/er/file.md"), "deep");
        write(&a.join(".paddock/both.md"), "a");
        write(&loft.notes().join("both.md"), "loft");
        write(&loft.notes().join("from-loft.md"), "pull me");
        std::fs::remove_file(a.join(".paddock/gone.md")).unwrap();
        std::fs::remove_file(a.join(".paddock/kept.md")).unwrap();
        write(&loft.notes().join("kept.md"), "k2");
        let versions: BTreeSet<&str> = [
            "s", "base", "g", "k", "new in a", "deep", "a", "loft", "pull me", "k2",
        ]
        .into();
        // The crash can come in any of the three syncs that do the work:
        // the first one waits with the delete and pushes, the second one
        // moves the delete to the trash, the third one pulls.
        CRASH_AT.with(|c| c.set(Some(at)));
        let mut crashed = false;
        for i in 0..3 {
            if i == 1 {
                later();
            }
            crashed |= std::panic::catch_unwind(|| sync(&loft, &a, "a")).is_err();
            // No half file: every file on both sides is a whole version.
            for side in [tree(&a.join(NOTES)), tree(&loft.notes())] {
                for (k, v) in &side {
                    if k.starts_with("inbox/") || k == ".gitignore" || k == MARK {
                        continue;
                    }
                    assert!(versions.contains(v.as_str()), "step {at}: {k} = {v:?}");
                }
            }
        }
        CRASH_AT.with(|c| c.set(None));
        // The next syncs finish the work: one, a later one for a delete
        // that waits again, and one for the pulls that it left. Then a
        // sync has nothing.
        sync(&loft, &a, "a").unwrap();
        later();
        sync(&loft, &a, "a").unwrap();
        sync(&loft, &a, "a").unwrap();
        let again = sync(&loft, &a, "a").unwrap();
        assert!(!again.changed(), "step {at}: {again:?}");
        let mut local = tree(&a.join(NOTES));
        local.retain(|k, _| !k.starts_with("inbox/") && k != ".gitignore" && k != MARK);
        let store = tree(&loft.notes());
        assert_eq!(local, store, "step {at}");
        assert!(
            !store.keys().any(|k| k.contains(".paddock-tmp-")),
            "step {at}: {store:?}"
        );
        // Every version is still somewhere.
        let have: BTreeSet<&str> = store.values().map(String::as_str).collect();
        for v in ["s", "new in a", "deep", "a", "loft", "pull me", "k2"] {
            assert!(have.contains(v), "step {at}: {v} is lost: {store:?}");
        }
        assert_eq!(store.get("both.md").map(String::as_str), Some("loft"));
        assert!(!store.contains_key("gone.md"), "step {at}");
        let bin: Vec<_> = crate::loft::trash::list(&loft)
            .into_iter()
            .filter(|e| e.path == "gone.md")
            .collect();
        assert_eq!(bin.len(), 1, "step {at}: gone.md is not in the trash");
        if !crashed {
            // Past the last step: every step had its crash.
            assert!(at >= 10, "only {at} steps");
            return;
        }
    }
    panic!("the sync never finished without a crash");
}

#[test]
fn a_concurrent_write_during_a_replace_stays_as_a_conflict_copy() {
    let t = tempfile::tempdir().unwrap();
    let root = Root::open(t.path())
        .unwrap()
        .dir(Path::new(""), false)
        .unwrap();
    std::fs::write(t.path().join("n.md"), "an agent wrote this").unwrap();
    // The sync planned to replace the version "old".
    let r = put(&root, "n.md", b"new", Some(&hash(b"old")), "tag").unwrap();
    let Put::Raced(Some(c)) = r else {
        panic!("no conflict copy")
    };
    assert_eq!(read(&t.path().join("n.md")), "new");
    assert_eq!(read(&t.path().join(&c)), "an agent wrote this");
    // A delete of a changed file puts it back.
    std::fs::write(t.path().join("d.md"), "changed").unwrap();
    assert!(matches!(
        del(&root, "d.md", &hash(b"before"), "tag").unwrap(),
        Del::Kept
    ));
    assert_eq!(read(&t.path().join("d.md")), "changed");
}

#[test]
fn symlinks_in_the_folder_are_never_followed() {
    let (t, env) = temp_env();
    let (loft, a, _) = two(t.path(), &env);
    let secret = t.path().join("secret");
    std::fs::write(&secret, "key").unwrap();
    std::os::unix::fs::symlink(&secret, a.join(".paddock/leak.md")).unwrap();
    std::os::unix::fs::symlink(t.path(), a.join(".paddock/dir")).unwrap();
    write(&loft.notes().join("leak.md"), "loft file");
    let r = sync(&loft, &a, "a").unwrap();
    assert!(r.skipped.iter().any(|s| s.contains("leak.md")), "{r:?}");
    assert_eq!(read(&loft.notes().join("leak.md")), "loft file");
    assert!(!loft.notes().join("dir").exists());
    assert_eq!(read(&secret), "key");
    assert!(std::fs::symlink_metadata(a.join(".paddock/leak.md"))
        .unwrap()
        .file_type()
        .is_symlink());
    // `.paddock` itself as a symlink is refused.
    std::fs::remove_dir_all(a.join(".paddock")).unwrap();
    std::os::unix::fs::symlink(t.path(), a.join(".paddock")).unwrap();
    assert!(sync(&loft, &a, "a").is_err());
}

#[test]
fn mode_switches_move_the_content_without_loss_and_git_stays_clean() {
    let (t, env) = temp_env();
    let (repo, wt) = repo_and_worktree(t.path());
    // Link mode first, the default.
    host::init(&env, &repo).unwrap();
    assert!(host::linked(&env, &repo));
    let loft = Loft::of(&env, &repo);
    write(&loft.notes().join("tickets/T1.md"), "t1");
    // Link to sync: a real folder with the loft's files.
    write_cfg(&env, "[loft]\nmode = \"sync\"\n");
    let msg = apply_mode(&env, &repo, "host").unwrap().unwrap();
    assert!(msg.contains("it was a link"), "{msg}");
    let m = std::fs::symlink_metadata(repo.join(NOTES)).unwrap();
    assert!(m.is_dir() && !m.file_type().is_symlink());
    assert_eq!(read(&repo.join(".paddock/tickets/T1.md")), "t1");
    assert!(apply_mode(&env, &repo, "host").unwrap().is_none());
    // The other work tree joins at its first run.
    let mut s = spec(&wt, t.path());
    mount_with(&env, &mut s, &repo, true).unwrap();
    assert!(
        !s.mounts.iter().any(|m| m.origin == "loft notes"),
        "{:?}",
        s.mounts
    );
    assert_eq!(read(&wt.join(".paddock/tickets/T1.md")), "t1");
    write(&repo.join(".paddock/new.md"), "new");
    // Git sees nothing in either work tree, not even the inbox.
    crate::broker::inbox(&repo).unwrap();
    for w in [&repo, &wt] {
        assert_eq!(git(w, &["status", "--porcelain", "--ignored=no"]), "");
        assert_eq!(
            git(w, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
    }
    assert!(!repo.join(".gitignore").exists());
    // Sync to link refuses while a file differs from the loft and cannot
    // sync (here: a symlink), and nothing changes.
    write_cfg(&env, "[loft]\nmode = \"link\"\n");
    std::os::unix::fs::symlink("/etc/hostname", repo.join(".paddock/odd")).unwrap();
    let e = apply_mode(&env, &repo, "host").unwrap_err();
    assert!(format!("{e:#}").contains("odd"), "{e:#}");
    assert!(synced(&loft, &repo));
    assert_eq!(read(&repo.join(".paddock/new.md")), "new");
    std::fs::remove_file(repo.join(".paddock/odd")).unwrap();
    // Then it syncs first, and the link replaces the folder.
    write(&repo.join(".paddock/last.md"), "last");
    std::fs::write(repo.join(".paddock/inbox/shot.png"), "png").unwrap();
    let msg = apply_mode(&env, &repo, "host").unwrap().unwrap();
    assert!(msg.contains("leads to the loft"), "{msg}");
    assert!(host::linked(&env, &repo));
    assert_eq!(read(&loft.notes().join("last.md")), "last");
    assert_eq!(read(&loft.notes().join("new.md")), "new");
    assert_eq!(read(&loft.notes().join("inbox/shot.png")), "png");
    assert!(!base_file(&loft, &repo).exists());
    for w in [&repo, &wt] {
        assert_eq!(
            git(w, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
    }
    // The other work tree is still in sync mode and gets the new files.
    write_cfg(&env, "[loft]\nmode = \"sync\"\n");
    sync(&loft, &wt, "wt").unwrap();
    assert_eq!(read(&wt.join(".paddock/last.md")), "last");
}

#[test]
fn a_sandbox_run_in_sync_mode_writes_notes_that_reach_the_loft() {
    let (t, env) = temp_env();
    write_cfg(&env, "[loft]\nmode = \"sync\"\n");
    let (repo, wt) = repo_and_worktree(t.path());
    host::init(&env, &repo).unwrap();
    let loft = Loft::of(&env, &repo);
    write(&loft.notes().join("tickets/T1.md"), "t1");
    // The build step of a run of the worktree: no mount over .paddock;
    // the sandbox sees the real folder of the project.
    let mut s = spec(&wt, t.path());
    mount_with(&env, &mut s, &repo, true).unwrap();
    assert!(
        !s.mounts.iter().any(|m| m.dst.starts_with(wt.join(NOTES))
            || m.src.as_ref().is_some_and(|s| s.starts_with(loft.notes()))),
        "{:?}",
        s.mounts
    );
    assert_eq!(read(&wt.join(".paddock/tickets/T1.md")), "t1");
    // The run's watcher syncs what the agent writes, while it runs.
    let w = Watch::start(&env, &wt, "s-1").unwrap();
    write(&wt.join(".paddock/notes/design.md"), "design");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !loft.notes().join("notes/design.md").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher did not sync"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // A write just before the end reaches the loft at the end, and the
    // loft commits it.
    write(&wt.join(".paddock/notes/last.md"), "last");
    drop(w);
    crate::loft::end_of_run(&env, &wt, "s-1", "fake");
    assert_eq!(read(&loft.notes().join("notes/design.md")), "design");
    assert_eq!(read(&loft.notes().join("notes/last.md")), "last");
    let log = crate::loft::git::log(&loft, 1).unwrap();
    assert!(log.contains("Run s-1 (fake)"), "{log}");
    assert!(log.contains("last.md"), "{log}");
    assert_eq!(
        git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
}

#[test]
fn the_mode_comes_from_the_project_file_first() {
    let (t, env) = temp_env();
    let p = t.path().join("p");
    std::fs::create_dir_all(&p).unwrap();
    assert_eq!(mode(&env, &p), Mode::Link);
    write_cfg(&env, "[loft]\nmode = \"sync\"\n");
    assert_eq!(mode(&env, &p), Mode::Sync);
    std::fs::write(p.join("paddock.toml"), "[loft]\nmode = \"link\"\n").unwrap();
    assert_eq!(mode(&env, &p), Mode::Link);
    // The project file may have [loft] mode, and nothing else of [loft].
    let f = p.join("paddock.toml");
    std::fs::write(&f, "backend = \"bwrap\"\n[loft]\nmode = \"sync\"\n").unwrap();
    let m = crate::manifest::Manifest::read(&f).unwrap();
    assert_eq!(m.backend.as_deref(), Some("bwrap"));
    std::fs::write(&f, "[loft]\nmode = \"sync\"\nauto = true\n").unwrap();
    assert!(crate::manifest::Manifest::read(&f).is_err());
    std::fs::write(&f, "[loft]\n").unwrap();
    assert!(crate::manifest::Manifest::read(&f).is_err());
    assert!(crate::loft::Cfg::parse("[loft]\nmode = \"other\"\n").is_err());
}

/// Every note of the store and of the other work tree is still there.
pub(super) fn assert_all_notes(loft: &Loft, other: &Path, notes: &[(&str, &str)], at: &str) {
    for (k, v) in notes {
        assert_eq!(
            std::fs::read_to_string(loft.notes().join(k))
                .ok()
                .as_deref(),
            Some(*v),
            "{at}: {k} is gone from the loft"
        );
    }
    sync(loft, other, "other").unwrap();
    for (k, v) in notes {
        assert_eq!(
            std::fs::read_to_string(other.join(NOTES).join(k))
                .ok()
                .as_deref(),
            Some(*v),
            "{at}: {k} is gone from the other work tree"
        );
    }
}

#[test]
fn a_folder_made_again_never_deletes_notes_with_a_stale_base() {
    let notes = [
        ("plan.md", "plan"),
        ("tickets/T1.md", "t1"),
        ("from-b.md", "b"),
    ];
    let scenarios = [
        "git clean -fdx",
        "rm -rf .paddock",
        "rm -rf, then an agent writes a note with mkdir -p",
        "the worktree comes back at the same path",
        "a stop of to_link between the link and the base delete",
    ];
    for at in scenarios {
        let (t, env) = temp_env();
        let (loft, a, b) = two(t.path(), &env);
        write(&a.join(".paddock/plan.md"), "plan");
        write(&a.join(".paddock/tickets/T1.md"), "t1");
        write(&b.join(".paddock/from-b.md"), "b");
        sync(&loft, &a, "a").unwrap();
        sync(&loft, &b, "b").unwrap();
        sync(&loft, &a, "a").unwrap();
        // b has a base with every note.
        assert!(base_file(&loft, &b).is_file());
        let dot = b.join(NOTES);
        match at {
            "git clean -fdx" => {
                git(&b, &["clean", "-fdxq"]);
                assert!(!dot.exists(), "{at}");
                start_of_run(&env, &b, "run-1");
            }
            "rm -rf .paddock" => {
                std::fs::remove_dir_all(&dot).unwrap();
                start_of_run(&env, &b, "run-1");
            }
            "rm -rf, then an agent writes a note with mkdir -p" => {
                // During a run: the watcher syncs the folder that the
                // agent made, without a mode switch.
                std::fs::remove_dir_all(&dot).unwrap();
                write(&dot.join("new.md"), "new");
                let r = sync(&loft, &b, "run-1").unwrap();
                assert!(r.deleted.is_empty(), "{at}: {r:?}");
                assert_eq!(read(&loft.notes().join("new.md")), "new");
            }
            "the worktree comes back at the same path" => {
                git(&a, &["worktree", "remove", "--force", b.to_str().unwrap()]);
                assert!(!b.exists());
                git(&a, &["worktree", "add", "-q", b.to_str().unwrap(), "side"]);
                start_of_run(&env, &b, "run-1");
            }
            _ => {
                // The old state of a stop there: the link, and the base.
                let saved = read(&base_file(&loft, &b));
                write_cfg(&env, "[loft]\nmode = \"link\"\n");
                apply_mode(&env, &b, "host").unwrap().unwrap();
                assert!(host::linked(&env, &b));
                write(&base_file(&loft, &b), &saved);
                write_cfg(&env, "[loft]\nmode = \"sync\"\n");
                start_of_run(&env, &b, "run-1");
            }
        }
        assert!(synced(&loft, &b), "{at}");
        let r = sync(&loft, &b, "b").unwrap();
        assert!(r.deleted.is_empty(), "{at}: {r:?}");
        for (k, v) in notes {
            assert_eq!(read(&dot.join(k)), v, "{at}: {k} did not come back");
        }
        assert_all_notes(&loft, &a, &notes, at);
        // From now on, a delete in b syncs again as usual.
        std::fs::remove_file(dot.join("plan.md")).unwrap();
        sync_delete(&loft, &b, "b", &["plan.md"], at);
    }
}

#[test]
fn the_switch_to_link_waits_for_the_runs_of_the_work_tree() {
    let (t, env) = temp_env();
    let (loft, a, b) = two(t.path(), &env);
    write(&a.join(".paddock/plan.md"), "plan");
    sync(&loft, &a, "a").unwrap();
    // A run of a (in a subfolder) is active.
    std::fs::create_dir_all(a.join("src")).unwrap();
    let run = crate::state::Running {
        id: "s-7".into(),
        project: a.join("src"),
        pid: std::process::id(),
        ..Default::default()
    };
    run.register(&env).unwrap();
    write_cfg(&env, "[loft]\nmode = \"link\"\n");
    let e = apply_mode(&env, &a, "host").unwrap_err();
    assert!(format!("{e:#}").contains("s-7"), "{e:#}");
    assert!(synced(&loft, &a));
    assert_eq!(read(&a.join(".paddock/plan.md")), "plan");
    // The run itself may switch its own work tree at its start, and a run
    // of another work tree does not count.
    assert!(apply_mode(&env, &b, "host").unwrap().is_some());
    assert!(apply_mode(&env, &a, "s-7").unwrap().is_some());
    assert!(host::linked(&env, &a));
}

#[test]
fn a_write_during_the_switch_to_link_stays() {
    let (t, env) = temp_env();
    let (loft, a, _) = two(t.path(), &env);
    write(&a.join(".paddock/plan.md"), "plan");
    write(&a.join(".paddock/other.md"), "other");
    sync(&loft, &a, "a").unwrap();
    // An agent saves plan.md through an open fd after the scan: the file
    // moved aside with the folder.
    let top = a.clone();
    AFTER_SCAN.with(|f| {
        *f.borrow_mut() = Some(Box::new(move || {
            let aside = std::fs::read_dir(&top)
                .unwrap()
                .flatten()
                .find(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(".paddock.to-link-")
                })
                .unwrap()
                .path();
            std::fs::write(aside.join("plan.md"), "the agent's version").unwrap();
        }))
    });
    write_cfg(&env, "[loft]\nmode = \"link\"\n");
    let msg = apply_mode(&env, &a, "host").unwrap().unwrap();
    assert!(msg.contains("plan.md"), "{msg}");
    assert!(host::linked(&env, &a));
    // The loft keeps its version, and the agent's version stays aside.
    assert_eq!(read(&loft.notes().join("plan.md")), "plan");
    let kept: Vec<String> = std::fs::read_dir(&a)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".paddock.to-link-")
        })
        .flat_map(|e| tree(&e.path()).into_values())
        .collect();
    assert_eq!(kept, vec!["the agent's version".to_string()]);
}

/// A sparse file larger than MAX_FILE.
pub(super) fn big(p: &Path) {
    let f = std::fs::File::create(p).unwrap();
    f.set_len(MAX_FILE + 1).unwrap();
}

#[test]
fn a_version_that_cannot_be_read_back_becomes_a_conflict_copy() {
    let t = tempfile::tempdir().unwrap();
    let root = Root::open(t.path())
        .unwrap()
        .dir(Path::new(""), false)
        .unwrap();
    let no_tmp = |d: &Path| {
        let names: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.contains(TMP)), "{names:?}");
    };
    // An agent made the file too large just before the replace.
    big(&t.path().join("log.md"));
    let Put::Raced(Some(c)) = put(&root, "log.md", b"new", Some(&hash(b"old")), "tag").unwrap()
    else {
        panic!("no conflict copy")
    };
    assert_eq!(read(&t.path().join("log.md")), "new");
    assert_eq!(
        std::fs::metadata(t.path().join(&c)).unwrap().len(),
        MAX_FILE + 1
    );
    no_tmp(t.path());
    // The same before a delete: it stays at its name.
    big(&t.path().join("d.md"));
    assert!(matches!(
        del(&root, "d.md", &hash(b"before"), "tag").unwrap(),
        Del::Kept
    ));
    assert_eq!(
        std::fs::metadata(t.path().join("d.md")).unwrap().len(),
        MAX_FILE + 1
    );
    no_tmp(t.path());
}

#[test]
fn an_interrupted_copy_that_cannot_be_read_is_kept_and_listed() {
    let (t, env) = temp_env();
    let (loft, a, _) = two(t.path(), &env);
    let in_store = format!("log.md{TMP}1-sync-0");
    let in_tree = format!("notes.md{TMP}1-sync-1");
    big(&loft.notes().join(&in_store));
    big(&a.join(NOTES).join(&in_tree));
    // Listed before a sync.
    let open = conflicts(&loft);
    assert!(open.iter().any(|c| c.starts_with(&in_store)), "{open:?}");
    let r = sync(&loft, &a, "a").unwrap();
    assert!(
        r.skipped.iter().any(|s| s.contains(&in_store))
            && r.skipped.iter().any(|s| s.contains(&in_tree)),
        "{r:?}"
    );
    // Now conflict copies, never a temporary name.
    assert!(!loft.notes().join(&in_store).exists());
    assert!(!a.join(NOTES).join(&in_tree).exists());
    let open = conflicts(&loft);
    assert!(
        open.iter().any(|c| c.starts_with("log.md.conflict-")),
        "{open:?}"
    );
    let s = host::status(&env, &a).unwrap();
    assert!(s.contains("log.md.conflict-"), "{s}");
    assert!(s.contains("notes.md.conflict-"), "{s}");
}

#[test]
fn the_watcher_sums_up_its_conflicts_at_the_end() {
    let (t, env) = temp_env();
    let (loft, a, _) = two(t.path(), &env);
    write(&a.join(".paddock/plan.md"), "base");
    sync(&loft, &a, "a").unwrap();
    write(&a.join(".paddock/plan.md"), "from a");
    write(&loft.notes().join("plan.md"), "from the loft");
    let w = Watch::start(&env, &a, "s-1").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while conflicts(&loft).is_empty() {
        assert!(std::time::Instant::now() < deadline, "no sync");
        std::thread::sleep(Duration::from_millis(50));
    }
    let lines = w.stop();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("both sides changed plan.md"))
            .count(),
        1,
        "{lines:?}"
    );
    let out = summary(&lines);
    assert!(
        out[0].starts_with("paddock: loft sync during the run: conflict"),
        "{out:?}"
    );
    let many: Vec<String> = (0..25).map(|i| format!("conflict {i}")).collect();
    let out = summary(&many);
    assert_eq!(out.len(), 21);
    assert!(out[20].contains("5 more"), "{out:?}");
}
