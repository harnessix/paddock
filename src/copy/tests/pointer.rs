//! The pointer folder and the states of a project folder.

use super::*;
use crate::copy::backup;
use crate::copy::pointer::{self, State};
use crate::copy::store::Store;

fn state(w: &World) -> State {
    pointer::status(&w.env, &w.cfg, &w.project, false)
        .unwrap()
        .state
}

fn start_with(w: &World, id: &str, answer: bool) -> anyhow::Result<(Run, Vec<State>)> {
    let mut asked = Vec::new();
    let r = start::create(
        &w.env,
        &w.cfg,
        &w.project,
        (id, "claude"),
        true,
        &mut |s, _| {
            asked.push(s);
            answer
        },
    )?;
    Ok((r.run, asked))
}

#[test]
fn a_folder_without_a_pointer_gets_one_at_the_first_run() {
    let w = world();
    assert_eq!(state(&w), State::NoPointer);
    let run = w.start("claude-webshop-1a2b3c");
    assert_eq!(state(&w), State::Linked);
    assert_eq!(run.store.meta.path, w.project);
    // The second run uses the same store and asks nothing.
    let second = w.start("claude-webshop-222222");
    assert_eq!(second.store.meta.id, run.store.meta.id);
}

#[test]
fn a_moved_folder_keeps_its_store_and_the_store_learns_the_new_path() {
    let mut w = world();
    let run = w.start("claude-webshop-1a2b3c");
    let moved = w.t.path().join("home/shop-renamed");
    std::fs::rename(&w.project, &moved).unwrap();
    w.project = moved.clone();
    assert_eq!(state(&w), State::Moved);
    // A look does not write. A run does.
    let before = Store::find(&w.cfg.roots(&w.env), &run.store.meta.id)
        .unwrap()
        .unwrap();
    assert_ne!(before.meta.path, moved);
    let (second, asked) = start_with(&w, "claude-webshop-222222", false).unwrap();
    assert!(asked.is_empty());
    assert_eq!(second.store.meta.id, run.store.meta.id);
    assert_eq!(second.store.meta.path, moved);
    assert_eq!(state(&w), State::Linked);
    assert_eq!(backup::list(&second.store).len(), 2);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

#[test]
fn a_copy_of_the_folder_carries_only_the_pointer_and_gets_its_own_store() {
    let mut w = world();
    let first = w.start("claude-webshop-1a2b3c");
    put(&first.tree(), "src/cart.ts", "the agent\n");
    let original = w.project.clone();
    let dup = w.t.path().join("home/webshop-copy");
    copy_dir(&original, &dup);
    // The copy of the folder has no file of the store.
    let names: Vec<_> = std::fs::read_dir(dup.join(".paddock"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names, ["pointer.json"]);
    assert_eq!(get(&dup, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    w.project = dup.clone();
    assert_eq!(state(&w), State::Copy);
    // "No" starts nothing and names the command.
    let e = start_with(&w, "claude-copy-333333", false).err().unwrap();
    let e = format!("{e:#}");
    assert!(
        e.contains("paddock pointer new") && e.contains("--direct"),
        "{e}"
    );
    // "Yes" makes a new, empty store. The old one stays with its folder.
    let (run, asked) = start_with(&w, "claude-copy-333333", true).unwrap();
    assert_eq!(asked, [State::Copy]);
    assert_ne!(run.store.meta.id, first.store.meta.id);
    assert_eq!(backup::list(&run.store).len(), 1);
    assert_eq!(state(&w), State::Linked);
    w.project = original;
    assert_eq!(state(&w), State::Linked);
    assert_eq!(status(&first).changes.len(), 1);
}

#[test]
fn a_restored_folder_asks_before_it_takes_the_store() {
    let mut w = world();
    let first = w.start("claude-webshop-1a2b3c");
    let restored = w.t.path().join("home/webshop-restored");
    copy_dir(&w.project, &restored);
    std::fs::remove_dir_all(&w.project).unwrap();
    w.project = restored.clone();
    assert_eq!(state(&w), State::Restored);
    let e = start_with(&w, "claude-webshop-444444", false)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("paddock pointer link"), "{e:#}");
    let (run, asked) = start_with(&w, "claude-webshop-444444", true).unwrap();
    assert_eq!(asked, [State::Restored]);
    assert_eq!(run.store.meta.id, first.store.meta.id);
    assert_eq!(run.store.meta.path, restored);
    assert_eq!(
        backup::list(&run.store).len(),
        2,
        "the old backups came along"
    );
    assert_eq!(state(&w), State::Linked);
}

#[test]
fn a_pointer_without_its_store_and_a_store_folder_that_is_away() {
    let mut w = world();
    let run = w.start("claude-webshop-1a2b3c");
    std::fs::remove_dir_all(&run.store.dir).unwrap();
    assert_eq!(state(&w), State::NoStore);
    assert!(start_with(&w, "claude-webshop-222222", false).is_err());
    w.cfg.store_dir = Some(w.t.path().join("not-mounted"));
    assert_eq!(state(&w), State::DiskAway);
    let e = start_with(&w, "claude-webshop-222222", true).err().unwrap();
    assert!(format!("{e:#}").contains("Connect the disk"), "{e:#}");
    w.cfg.store_dir = None;
    let (again, asked) = start_with(&w, "claude-webshop-222222", true).unwrap();
    assert_eq!(asked, [State::NoStore]);
    assert_ne!(again.store.meta.id, run.store.meta.id);
}

#[test]
fn a_broken_pointer_is_an_error_and_not_a_new_store() {
    let w = world();
    put(&w.project, ".paddock/pointer.json", "{ not json");
    let e = pointer::status(&w.env, &w.cfg, &w.project, false)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("is invalid"), "{e:#}");
    put(
        &w.project,
        ".paddock/pointer.json",
        r#"{"format":1,"store":"../../etc","created":1}"#,
    );
    let e = pointer::status(&w.env, &w.cfg, &w.project, false)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("names no store id"), "{e:#}");
    put(
        &w.project,
        ".paddock/pointer.json",
        r#"{"format":99,"store":"00000000-0000-4000-8000-000000000000","created":1}"#,
    );
    let e = pointer::status(&w.env, &w.cfg, &w.project, false)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("newer paddock"), "{e:#}");
}

#[test]
fn the_notes_link_of_the_loft_becomes_the_pointer_folder() {
    let w = world();
    let notes = crate::loft::Loft::of(&w.env, &w.project).notes();
    put(&notes, "plan.md", "a note\n");
    std::os::unix::fs::symlink(&notes, w.project.join(".paddock")).unwrap();
    assert_eq!(state(&w), State::NoPointer);
    let r = start::create(
        &w.env,
        &w.cfg,
        &w.project,
        ("claude-webshop-1a2b3c", "claude"),
        true,
        &mut |_, _| false,
    )
    .unwrap();
    assert!(r
        .notices
        .iter()
        .any(|n| n.contains("was a link to the notes")));
    let m = std::fs::symlink_metadata(w.project.join(".paddock")).unwrap();
    assert!(m.is_dir() && !m.is_symlink());
    assert!(!w.project.join(".paddock/plan.md").exists());
    // The notes are where they were, and nothing of them is in the copy's
    // change list.
    assert_eq!(get(&notes, "plan.md").unwrap(), "a note\n");
    assert!(status(&r.run).changes.is_empty());
    // The link step of the loft keeps the pointer folder.
    crate::loft::host::link(&crate::loft::Loft::of(&w.env, &w.project), &w.project).unwrap();
    assert!(w.project.join(".paddock/pointer.json").is_file());
}

#[test]
fn a_link_to_another_place_is_refused_and_never_followed() {
    let w = world();
    let other = w.t.path().join("other");
    put(
        &other,
        "pointer.json",
        r#"{"format":1,"store":"00000000-0000-4000-8000-000000000000","created":1}"#,
    );
    std::os::unix::fs::symlink(&other, w.project.join(".paddock")).unwrap();
    // The pointer file behind the link is not read.
    assert_eq!(state(&w), State::NoPointer);
    let r = start::create(
        &w.env,
        &w.cfg,
        &w.project,
        ("claude-webshop-1a2b3c", "claude"),
        true,
        &mut |_, _| true,
    );
    let e = format!("{:#}", r.err().unwrap());
    assert!(e.contains("symlink to another place"), "{e}");
    assert!(
        Store::all(&w.cfg.roots(&w.env)).is_empty(),
        "no store stays behind"
    );
}

#[test]
fn a_notes_folder_of_the_project_keeps_its_files_next_to_the_pointer() {
    let w = world();
    put(&w.project, ".paddock/plan.md", "a note in the project\n");
    let run = w.start("claude-webshop-1a2b3c");
    assert_eq!(
        get(&w.project, ".paddock/plan.md").unwrap(),
        "a note in the project\n"
    );
    assert!(w.project.join(".paddock/pointer.json").is_file());
    // The notes are not project files: not in the backup, not in the copy.
    assert!(run
        .base()
        .unwrap()
        .meta
        .files
        .keys()
        .all(|p| !p.starts_with(".paddock")));
    assert!(!run.tree().join(".paddock/plan.md").exists());
}

#[test]
fn store_ids_are_uuids_and_nothing_else_names_a_store() {
    let id = crate::copy::store::new_id().unwrap();
    assert!(crate::copy::store::valid_id(&id), "{id}");
    assert_eq!(id.as_bytes()[14], b'4');
    for bad in [
        "",
        "..",
        "../x",
        "00000000-0000-4000-8000-00000000000",
        "0000000G-0000-4000-8000-000000000000",
        "00000000-0000-4000-8000-000000000000/x",
    ] {
        assert!(!crate::copy::store::valid_id(bad), "{bad}");
    }
    let w = world();
    assert!(Store::open(&w.cfg.root(&w.env), "../..").is_err());
}

#[test]
fn the_decision_for_a_copy() {
    use crate::copy::config::Default_;
    use crate::copy::start::{decide, Why};
    use crate::copy::Ask;
    let cfg = Config::default();
    let run = Ask {
        allowed: true,
        ..Default::default()
    };
    // Launcher runs and folders without git, by default.
    assert_eq!(decide(&cfg, &run, false, None).unwrap(), Some(Why::NoGit));
    assert_eq!(decide(&cfg, &run, true, None).unwrap(), None);
    let launcher = Ask {
        launcher: true,
        ..run
    };
    assert_eq!(
        decide(&cfg, &launcher, true, None).unwrap(),
        Some(Why::Launcher)
    );
    // The flags win.
    let copy = Ask {
        choice: Some(true),
        ..run
    };
    assert_eq!(decide(&cfg, &copy, true, None).unwrap(), Some(Why::Flag));
    let direct = Ask {
        choice: Some(false),
        ..launcher
    };
    assert_eq!(decide(&cfg, &direct, false, None).unwrap(), None);
    // The setting.
    let never = Config {
        default: Default_::Never,
        ..Config::default()
    };
    assert_eq!(decide(&never, &launcher, false, None).unwrap(), None);
    assert_eq!(decide(&never, &copy, false, None).unwrap(), Some(Why::Flag));
    let always = Config {
        default: Default_::Always,
        ..Config::default()
    };
    assert_eq!(
        decide(&always, &run, true, None).unwrap(),
        Some(Why::Config)
    );
    // A run that cannot have a copy: an error only when the user asked.
    assert_eq!(
        decide(&always, &run, false, Some("--worktree")).unwrap(),
        None
    );
    assert!(decide(&cfg, &copy, false, Some("--worktree")).is_err());
    // A command that never asks: doctor, apps, spawned runs.
    assert_eq!(decide(&always, &Ask::default(), false, None).unwrap(), None);
}

#[test]
fn where_the_sandbox_sees_the_copy() {
    use crate::copy::start::in_place;
    use crate::spec::Network;
    assert!(in_place("auto", Network::Egress, false));
    assert!(in_place("bwrap", Network::None, false));
    assert!(in_place("podman", Network::Egress, true));
    // srt and Seatbelt cannot mount: the copy has its own path.
    assert!(!in_place("auto", Network::Allowlist, false));
    assert!(!in_place("auto", Network::Egress, true));
    assert!(!in_place("seatbelt", Network::Egress, true));
    assert!(!in_place("srt", Network::Egress, false));
}

#[test]
fn the_copy_table_of_the_configuration() {
    use crate::copy::config::{Apply, Default_};
    let c = Config::parse("").unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(c.max, 1_000_000_000);
    let c = Config::parse(
        "[copy]\ndefault = \"never\"\napply = \"auto\"\nmax = \"2G\"\nstore_dir = \"/mnt/big/copies\"\n",
    )
    .unwrap();
    assert_eq!(c.default, Default_::Never);
    assert_eq!(c.apply, Apply::Auto);
    assert_eq!(c.max, 2_000_000_000);
    assert_eq!(c.store_dir.as_deref(), Some(Path::new("/mnt/big/copies")));
    for bad in [
        "[copy]\ndefault = \"sometimes\"\n",
        "[copy]\napply = true\n",
        "[copy]\nstore_dir = \"relative\"\n",
        "[copy]\nmax = \"big\"\n",
        "[copy]\nunknown = 1\n",
        "copy = 1\n",
    ] {
        assert!(Config::parse(bad).is_err(), "{bad}");
    }
    // A project file cannot set the table: the agent can edit the project.
    assert!(crate::manifest::USER_ONLY.contains(&"copy"));
}
