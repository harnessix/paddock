//! Two children of one project start at the same moment: the worktree of
//! the second one appears between the detectors and the policy check of the
//! first one. Without bwrap: the worktrees come from `spawn`, the spec from
//! the git detector, and the check is `policy::enforce`.

use super::writable;
use crate::detect::{Context, Detector, Git};
use crate::env::HostEnv;
use crate::spawn::{create_worktree, ChildPlan, Role};
use crate::spec::{Mount, SandboxSpec};
use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

struct World {
    _tmp: tempfile::TempDir,
    env: HostEnv,
    repo: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let env = HostEnv::from_parts(
        tmp.path().join("home"),
        "demo".into(),
        1000,
        1000,
        Default::default(),
    );
    World {
        _tmp: tmp,
        env,
        repo,
    }
}

fn spawn(w: &World, id: &str) -> PathBuf {
    let plan = ChildPlan {
        id: id.into(),
        manifest: Default::default(),
        branch: format!("paddock/spawn-claude-{id}"),
        base_from: w.repo.clone(),
        base: None,
        role: Role::Implementer,
        trainer: false,
    };
    create_worktree(&w.env, &w.repo, &plan).unwrap().path
}

/// The spec of a child as far as git goes: its worktree read-write and
/// what the git detector adds.
fn child_spec(w: &World, worktree: &Path) -> SandboxSpec {
    let mut spec = SandboxSpec::new(
        "t".into(),
        "x",
        worktree,
        w.env.home.join("sandbox"),
        &w.env.home,
        1000,
        1000,
        "demo",
    );
    spec.add_mount(Mount::same(worktree, true, "project"));
    let manifest = crate::manifest::Manifest::default();
    let cx = Context {
        env: &w.env,
        project: worktree,
        manifest: &manifest,
    };
    Git.apply(&cx, &mut spec).unwrap();
    spec
}

/// The git directory of the worktree at `path`.
fn git_dir(path: &Path) -> PathBuf {
    crate::hostgit::Layout::discover(path)
        .unwrap()
        .unwrap()
        .git_dir
}

fn assert_cannot_change(spec: &SandboxSpec, dir: &Path) {
    for f in [
        "",
        "commondir",
        "gitdir",
        "HEAD",
        "index",
        "config.worktree",
    ] {
        let p = if f.is_empty() {
            dir.to_path_buf()
        } else {
            dir.join(f)
        };
        assert!(!writable(spec, &p), "{} is writable", p.display());
    }
}

#[test]
fn a_sibling_worktree_that_appears_after_the_detectors_passes_the_policy() {
    let w = world();
    let a = spawn(&w, "aaaa0001");
    let spec_a = child_spec(&w, &a);
    super::super::enforce(&w.env, &spec_a).unwrap();
    // The second child starts now: its worktree is new to the spec of the first.
    let b = spawn(&w, "bbbb0002");
    super::super::enforce(&w.env, &spec_a).unwrap();
    // The first child cannot change the git files of the second one, and it
    // still writes its own index and HEAD.
    assert_cannot_change(&spec_a, &git_dir(&b));
    assert!(writable(&spec_a, &git_dir(&a).join("index")));
    assert!(!writable(&spec_a, &git_dir(&a).join("commondir")));
    let spec_b = child_spec(&w, &b);
    super::super::enforce(&w.env, &spec_b).unwrap();
    assert_cannot_change(&spec_b, &git_dir(&a));
}

#[test]
fn two_children_that_start_together_pass_the_policy() {
    let w = std::sync::Arc::new(world());
    for round in 0..3 {
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let (w, gate) = (w.clone(), gate.clone());
                std::thread::spawn(move || {
                    let path = spawn(&w, &format!("{name}{round:07}"));
                    let spec = child_spec(&w, &path);
                    super::super::enforce(&w.env, &spec).unwrap();
                    gate.wait();
                    // Both worktrees exist now: the check at the sandbox
                    // start sees the other one too.
                    super::super::enforce(&w.env, &spec).unwrap();
                    (path, spec)
                })
            })
            .collect();
        let done: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_cannot_change(&done[0].1, &git_dir(&done[1].0));
        assert_cannot_change(&done[1].1, &git_dir(&done[0].0));
    }
}

#[test]
fn the_main_checkout_cannot_change_the_worktrees_of_its_children() {
    let w = world();
    let mut spec = child_spec(&w, &w.repo);
    super::super::enforce(&w.env, &spec).unwrap();
    let a = spawn(&w, "cccc0003");
    super::super::enforce(&w.env, &spec).unwrap();
    assert_cannot_change(&spec, &git_dir(&a));
    // A spec without the read-only `worktrees` folder is refused.
    spec.mounts
        .retain(|m| m.dst != w.repo.join(".git/worktrees"));
    let e = super::super::enforce(&w.env, &spec)
        .unwrap_err()
        .to_string();
    assert!(e.contains(".git/worktrees is writable"), "{e}");
}
