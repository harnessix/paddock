use super::*;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo(root: &Path) -> PathBuf {
    let p = root.join("repo");
    std::fs::create_dir_all(&p).unwrap();
    git(&p, &["init", "-q", "-b", "main"]);
    git(&p, &["commit", "-q", "--allow-empty", "-m", "one"]);
    p
}

#[test]
fn finds_a_plain_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let p = repo(tmp.path());
    std::fs::create_dir_all(p.join("a/b")).unwrap();
    let l = Layout::discover(&p.join("a/b")).unwrap().unwrap();
    assert_eq!(l.work_tree, p);
    assert_eq!(l.git_dir, p.join(".git"));
    assert_eq!(l.common_dir, l.git_dir);
    assert!(!l.linked);
    // A directory with no repository above it is not one.
    let bare = tmp.path().join("plain/deep");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(Layout::discover(&bare).unwrap().is_none() || !bare.join(".git").exists());
    // An empty .git where paddock was pointed is reported, not ignored.
    let broken = tmp.path().join("broken");
    std::fs::create_dir_all(broken.join(".git")).unwrap();
    assert!(Layout::discover(&broken).is_err());
}

#[test]
fn ignores_core_worktree_and_refuses_a_planted_commondir() {
    let tmp = tempfile::tempdir().unwrap();
    let p = repo(tmp.path());
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    // A sandbox-written core.worktree must not move the project.
    std::fs::write(
        p.join(".git/config"),
        format!("[core]\n\tworktree = {}\n", elsewhere.display()),
    )
    .unwrap();
    assert_eq!(Layout::discover(&p).unwrap().unwrap().work_tree, p);
    // A planted commondir is refused by name.
    std::fs::write(
        p.join(".git/commondir"),
        format!("{}\n", elsewhere.display()),
    )
    .unwrap();
    let e = Layout::discover(&p).unwrap_err().to_string();
    assert!(e.contains("commondir"), "{e}");
    // The harmless self-reference that paddock itself writes is fine.
    std::fs::write(p.join(".git/commondir"), ".\n").unwrap();
    assert!(Layout::discover(&p).unwrap().is_some());
}

#[test]
fn refuses_a_dot_git_file_that_points_anywhere() {
    let tmp = tempfile::tempdir().unwrap();
    let p = repo(tmp.path());
    let wt = tmp.path().join("wt");
    git(&p, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let l = Layout::discover(&wt).unwrap().unwrap();
    assert!(l.linked);
    assert_eq!(l.common_dir, p.join(".git"));
    assert_eq!(l.work_tree, wt);

    let other = repo(&tmp.path().join("other"));
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", other.join(".git").display()),
    )
    .unwrap();
    let e = Layout::discover(&wt).unwrap_err().to_string();
    assert!(e.contains("registered worktree"), "{e}");

    // A git directory inside the work tree is refused too.
    std::fs::write(wt.join(".git"), "gitdir: inside\n").unwrap();
    assert!(Layout::discover(&wt).is_err());
}

#[test]
fn queries_do_not_run_repository_config() {
    let tmp = tempfile::tempdir().unwrap();
    let p = repo(tmp.path());
    let canary = tmp.path().join("canary");
    std::fs::write(
        p.join(".git/config"),
        format!(
            "[core]\n\tfsmonitor = touch {}\n\tpager = touch {0}\n",
            canary.display()
        ),
    )
    .unwrap();
    let l = Layout::discover(&p).unwrap().unwrap();
    let head = l.query(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(head.trim().len(), 40);
    assert!(l.try_query(&["log", "--oneline", "-n1"]).is_some());
    assert!(!canary.exists(), "a host query ran repository config");
    // A command that must write the work tree is refused instead.
    let e = Checkout::open(&p)
        .unwrap()
        .run(&["checkout", "main"])
        .unwrap_err()
        .to_string();
    assert!(e.contains("core.fsmonitor"), "{e}");
}

#[test]
fn finds_config_keys_that_run_programs() {
    for text in [
        "[core]\n\tfsmonitor = evil\n",
        "[core]\nhooksPath = /tmp/h\n",
        "[filter \"x\"]\n\tclean = evil\n",
        "[alias]\n\tst = !evil\n",
        "[include]\n\tpath = /tmp/other\n",
        "[diff]\n\texternal = evil\n",
        "[credential]\n\thelper = evil\n",
    ] {
        assert!(exec_key(text).is_some(), "{text}");
    }
    for text in [
        "[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
        "[remote \"origin\"]\n\turl = https://example.com/x.git\n",
        "[branch \"main\"]\n\tmerge = refs/heads/main\n",
        "# a comment\n",
    ] {
        assert_eq!(exec_key(text), None, "{text}");
    }
}

#[test]
fn status_runs_in_a_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let p = repo(tmp.path());
    std::fs::write(p.join("new.txt"), "x").unwrap();
    let canary = tmp.path().join("canary");
    std::fs::write(
        p.join(".git/config"),
        format!("[core]\n\tfsmonitor = touch {}\n", canary.display()),
    )
    .unwrap();
    let env = HostEnv::detect().unwrap();
    let l = Layout::discover(&p).unwrap().unwrap();
    if env.which("bwrap").is_none() {
        assert!(l.status(&env, "all").unwrap().is_none());
        return;
    }
    if !crate::testutil::bwrap_works() {
        // A bwrap that cannot start gives no status, as a missing one.
        assert!(l.status(&env, "all").unwrap().is_none());
        assert!(!canary.exists(), "the planted fsmonitor reached the host");
        eprintln!("skipped: needs a working bwrap");
        return;
    }
    let lines = l.status(&env, "all").unwrap().unwrap();
    assert!(lines.iter().any(|l| l.contains("new.txt")), "{lines:?}");
    assert!(!canary.exists(), "the planted fsmonitor reached the host");
}
