//! The build of a run on a copy: which folder the sandbox gets, and that
//! no mount lets it write the folder of the user.

use super::*;
use crate::build::{build_with, Built, Options};
use crate::copy::Ask;
use crate::profile::Harness;
use crate::spec::MountKind;

struct World {
    _tmp: tempfile::TempDir,
    env: HostEnv,
    project: PathBuf,
}

fn world(git: bool) -> Option<World> {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let project = root.join("proj");
    std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        home.join(".config/paddock/config.toml"),
        "[compaction]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(project.join("src/a.txt"), "a\n").unwrap();
    if git {
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&project)
            .env("HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipped: git is not here");
            return None;
        }
    }
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
    Some(World {
        _tmp: tmp,
        env,
        project,
    })
}

fn build_copy(w: &World, backend: &str, ask: Ask) -> Result<Built> {
    let h = Harness {
        profile: crate::profile::builtin("codex").unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "codex".into(),
        project: Some(w.project.clone()),
        backend: Some(backend.into()),
        assume_yes: true,
        copy: ask,
        ..Default::default()
    };
    build_with(&w.env, &opts, h)
}

const COPY: Ask = Ask {
    allowed: true,
    choice: Some(true),
    launcher: false,
    plan_only: false,
};

/// The mounts with a source on the host that the sandbox can write.
fn writable_sources(b: &Built) -> Vec<&Path> {
    b.spec
        .mounts
        .iter()
        .filter(|m| m.kind == MountKind::Rw)
        .filter_map(|m| m.src.as_deref())
        .collect()
}

#[test]
fn the_copy_is_mounted_at_the_path_of_the_project_and_nothing_else_comes_from_the_folder() {
    let Some(w) = world(true) else { return };
    let b = build_copy(&w, "bwrap", COPY).unwrap();
    let run = b.copy.run.as_ref().expect("a copy");
    let tree = run.tree();
    // The sandbox sees the project path, and gets the copy there.
    assert_eq!(b.spec.project, w.project);
    assert_eq!(b.spec.workdir, w.project);
    let main = b.spec.mounts.iter().find(|m| m.dst == w.project).unwrap();
    assert_eq!(main.src.as_deref(), Some(tree.as_path()));
    assert_eq!(main.kind, MountKind::Rw);
    assert!(main.origin.contains("a copy"), "{}", main.origin);
    // No mount takes anything from the folder of the user: not the git
    // directory, not its hooks, not a file.
    for m in &b.spec.mounts {
        if let Some(src) = &m.src {
            assert!(
                !src.starts_with(&w.project),
                "{} comes from the folder of the user ({})",
                src.display(),
                m.origin
            );
        }
    }
    // git works in the copy, on the git directory of the copy.
    let git = b
        .spec
        .mounts
        .iter()
        .find(|m| m.dst == w.project.join(".git"))
        .expect("the git directory is a mount point");
    assert_eq!(git.src.as_deref(), Some(tree.join(".git").as_path()));
    assert!(tree.join(".git/HEAD").is_file());
    // Every source below the copy exists, so the sandbox can start.
    for m in &b.spec.mounts {
        if let Some(src) = m.src.as_ref().filter(|s| s.starts_with(&tree)) {
            assert!(src.exists(), "{} is missing ({})", src.display(), m.origin);
        }
    }
    assert!(!writable_sources(&b)
        .iter()
        .any(|s| s.starts_with(&w.project)));
    // The record of the spec, for `paddock explain` and the backends.
    let c = b.spec.copy.as_ref().unwrap();
    assert_eq!(
        (c.original.as_path(), c.tree.as_path()),
        (w.project.as_path(), tree.as_path())
    );
    assert!(c.in_place);
    assert_eq!(c.why, "--copy");
    assert!(b
        .spec
        .notices
        .iter()
        .any(|n| n.contains("the agent works on a copy of")));
    // A run on a copy has the backup of its start: no second snapshot.
    assert!(!b.takes_snapshot());
    // The folder of the user: its files, and the pointer with nothing else.
    assert_eq!(
        std::fs::read_to_string(w.project.join("src/a.txt")).unwrap(),
        "a\n"
    );
    let names: Vec<_> = std::fs::read_dir(w.project.join(".paddock"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        names,
        ["pointer.json"],
        "no inbox and no note next to the pointer"
    );
    // The notes and the inbox of host requests are in the loft.
    let notes = crate::loft::Loft::of(&w.env, &w.project).notes();
    let at = b
        .spec
        .mounts
        .iter()
        .find(|m| m.dst == w.project.join(".paddock"))
        .unwrap();
    assert_eq!(at.src.as_deref(), Some(notes.as_path()));
    let inbox = crate::broker::inbox_for(&w.env, &w.project).unwrap();
    assert_eq!(inbox.path(), notes.join("inbox"));
    assert!(b
        .spec
        .mounts
        .iter()
        .any(|m| m.dst == notes && m.src.as_deref() == Some(notes.as_path())));
}

#[test]
fn a_backend_that_cannot_mount_gets_the_copy_at_its_own_path() {
    let Some(w) = world(false) else { return };
    let b = build_copy(&w, "srt", COPY).unwrap();
    let run = b.copy.run.as_ref().expect("a copy");
    let tree = run.tree();
    assert!(!run.rec.in_place);
    assert_eq!(b.spec.project, tree);
    assert_eq!(b.spec.workdir, tree);
    assert_eq!(b.spec.copy.as_ref().unwrap().original, w.project);
    assert!(!writable_sources(&b)
        .iter()
        .any(|s| s.starts_with(&w.project)));
    // The settings and the sandbox home belong to the folder of the user.
    assert_eq!(b.home_key, w.project);
    // Its `.paddock` leads to the notes, and the inbox is found through the
    // record of the run, not through that link.
    let notes = crate::loft::Loft::of(&w.env, &w.project).notes();
    assert_eq!(std::fs::read_link(tree.join(".paddock")).unwrap(), notes);
    assert_eq!(
        crate::copy::run::original_of_tree(&w.env, &tree),
        Some(w.project.clone())
    );
    let inbox = crate::broker::inbox_for(&w.env, &tree).unwrap();
    assert_eq!(inbox.path(), notes.join("inbox"));
    // A path that only looks like a copy names no original.
    assert_eq!(
        crate::copy::run::original_of_tree(&w.env, &w.project.join("tree")),
        None
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("src/a.txt")).unwrap(),
        "a\n"
    );
}

#[test]
fn a_run_without_the_flag_in_a_git_project_works_in_the_folder() {
    let Some(w) = world(true) else { return };
    let ask = Ask {
        choice: None,
        ..COPY
    };
    let b = build_copy(&w, "bwrap", ask).unwrap();
    assert!(b.copy.run.is_none() && b.spec.copy.is_none());
    assert!(b.takes_snapshot());
    let main = b.spec.mounts.iter().find(|m| m.dst == w.project).unwrap();
    assert_eq!(main.src.as_deref(), Some(w.project.as_path()));
    assert!(!w.project.join(".paddock/pointer.json").exists());
    // A command that never asks for a copy gets none, also outside git.
    let Some(w) = world(false) else { return };
    let b = build_copy(&w, "bwrap", Ask::default()).unwrap();
    assert!(b.copy.run.is_none());
    // A run of the user in a folder without git gets one by default.
    let b = build_copy(&w, "bwrap", ask).unwrap();
    assert_eq!(
        b.spec.copy.as_ref().unwrap().why,
        "the folder has no git history"
    );
}

#[test]
fn an_explain_plans_the_copy_and_makes_nothing() {
    let Some(w) = world(false) else { return };
    let ask = Ask {
        plan_only: true,
        ..COPY
    };
    let b = build_copy(&w, "bwrap", ask).unwrap();
    assert!(b.copy.run.is_none() && b.spec.copy.is_none());
    assert!(b
        .copy
        .plan
        .as_ref()
        .unwrap()
        .contains("at the path of the project"));
    assert!(!w.project.join(".paddock/pointer.json").exists());
    assert!(!w.env.paddock_data().join("copies").exists());
}

#[test]
fn what_cannot_have_a_copy_says_so_when_the_user_asks() {
    let Some(w) = world(false) else { return };
    // The vm backend.
    let e = build_copy(&w, "vm", COPY).err().unwrap();
    assert!(
        format!("{e:#}").contains("--copy does not work with the vm backend"),
        "{e:#}"
    );
    // A work tree whose git directory is in another folder.
    std::fs::write(
        w.project.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/x\n",
    )
    .unwrap();
    let e = build_copy(&w, "bwrap", COPY).err().unwrap();
    assert!(format!("{e:#}").contains("a linked git worktree"), "{e:#}");
    std::fs::remove_file(w.project.join(".git")).unwrap();
    // The sync mode of the loft keeps the notes in the project folder.
    std::fs::write(
        w.env.paddock_config().join("config.toml"),
        "[compaction]\nenabled = false\n[loft]\nmode = \"sync\"\n",
    )
    .unwrap();
    let e = build_copy(&w, "bwrap", COPY).err().unwrap();
    assert!(format!("{e:#}").contains("[loft] mode = \"sync\""), "{e:#}");
    // Without the flag, such a run works in the folder, as before.
    let ask = Ask {
        choice: None,
        ..COPY
    };
    assert!(build_copy(&w, "bwrap", ask).unwrap().copy.run.is_none());
    assert!(!w.project.join(".paddock/pointer.json").exists());
}

#[test]
fn a_build_that_fails_after_the_copy_leaves_no_copy_behind() {
    let Some(w) = world(false) else { return };
    // The project file asks for more than the defaults, and nobody says yes.
    std::fs::write(
        w.project.join("paddock.toml"),
        "[network]\nmode = \"host\"\n",
    )
    .unwrap();
    let h = Harness {
        profile: crate::profile::builtin("codex").unwrap(),
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "codex".into(),
        project: Some(w.project.clone()),
        backend: Some("bwrap".into()),
        copy: COPY,
        ..Default::default()
    };
    assert!(build_with(&w.env, &opts, h).is_err());
    let stores = crate::copy::store::Store::all(&[w.env.paddock_data().join("copies")]);
    for s in &stores {
        assert!(
            crate::copy::run::of_store(s).is_empty(),
            "a run folder stays"
        );
        assert!(crate::copy::backup::list(s).is_empty(), "a backup stays");
    }
}

/// The arguments that follow `flag` in a bwrap command line, in pairs of
/// source and destination.
fn binds<'a>(args: &'a [String], flag: &str) -> Vec<(&'a str, &'a str)> {
    args.windows(3)
        .filter(|w| w[0] == flag)
        .map(|w| (w[1].as_str(), w[2].as_str()))
        .collect()
}

#[test]
fn bwrap_binds_the_copy_at_the_project_path_and_never_the_folder_of_the_user() {
    if cfg!(target_os = "macos") {
        return;
    }
    let Some(w) = world(true) else { return };
    let b = build_copy(&w, "bwrap", COPY).unwrap();
    let tree = b.copy.run.as_ref().unwrap().tree();
    let launch = match crate::backend::by_name("bwrap")
        .unwrap()
        .render(&w.env, &b.spec, false)
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("skipped: bwrap cannot render here: {e:#}");
            return;
        }
    };
    let (project, tree_s) = (w.project.display().to_string(), tree.display().to_string());
    let rw = binds(&launch.args, "--bind");
    assert!(rw.contains(&(tree_s.as_str(), project.as_str())), "{rw:?}");
    let git = (format!("{tree_s}/.git"), format!("{project}/.git"));
    assert!(rw.contains(&(git.0.as_str(), git.1.as_str())), "{rw:?}");
    // No bind of any kind has a source in the folder of the user.
    for flag in [
        "--bind",
        "--ro-bind",
        "--bind-try",
        "--ro-bind-try",
        "--dev-bind",
    ] {
        for (src, dst) in binds(&launch.args, flag) {
            assert!(
                !Path::new(src).starts_with(&w.project),
                "{flag} {src} {dst} takes a path of the folder of the user"
            );
        }
    }
    // The hooks of the copy's git directory stay read-only.
    let ro = binds(&launch.args, "--ro-bind");
    let hooks = (
        format!("{tree_s}/.git/hooks"),
        format!("{project}/.git/hooks"),
    );
    assert!(ro.contains(&(hooks.0.as_str(), hooks.1.as_str())), "{ro:?}");
    // The sandbox starts in the project.
    let at = launch.args.iter().position(|a| a == "--chdir").unwrap();
    assert_eq!(launch.args[at + 1], project);
}
