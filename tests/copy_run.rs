//! `paddock run --copy` with a fake harness in a real sandbox (bwrap): the
//! agent works on a copy at the path of the project, the folder of the
//! user changes only at `paddock apply`, and `paddock undo` takes the apply
//! back. A temp home only. Skipped where bwrap cannot start.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn cmd(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("PADDOCK_NO_PROMOTE", "1")
        .env("PADDOCK_NO_DEDUPE", "1")
        .env_remove("TMUX")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(Stdio::null());
    for v in paddock::proxy::PROXY_VARS {
        c.env_remove(v);
    }
    c
}

/// The agent: it changes one file, makes one, deletes one, and records
/// where it works and what its notes folder holds.
const SCRIPT: &str = r#"
pwd > where.txt
printf 'by the agent\n' > a.txt
printf 'new\n' > made.txt
rm b.txt
ls -A .paddock > notes-seen.txt
printf 'a note\n' > .paddock/plan.md
"#;

struct World {
    _tmp: common::TestDir,
    root: PathBuf,
    project: PathBuf,
}

/// A temp host with the fake harness `fake`, and a project without git.
/// None where bwrap cannot start.
fn world(script: &str, config: &str) -> Option<World> {
    if cfg!(target_os = "macos") || !common::bwrap_works() {
        eprintln!("skipped: bwrap cannot start here");
        return None;
    }
    let tmp = common::tempdir();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    for d in ["cfg/paddock/harness.d", "home", "p", "run"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        format!("[devices]\nmount = false\n[kv]\nenabled = false\n{config}"),
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/harness.d/fake.toml"),
        format!(
            "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n",
            serde_json::to_string(script).unwrap()
        ),
    )
    .unwrap();
    let project = root.join("p");
    std::fs::write(project.join("a.txt"), "a\n").unwrap();
    std::fs::write(project.join("b.txt"), "b\n").unwrap();
    Some(World {
        _tmp: tmp,
        root,
        project,
    })
}

impl World {
    fn start(&self, flags: &[&str]) -> Output {
        cmd(&self.root)
            .args([
                "run",
                "fake",
                "--backend",
                "bwrap",
                "--network",
                "none",
                "-y",
            ])
            .args(flags)
            .arg(&self.project)
            .current_dir(&self.project)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let o = cmd(&self.root)
            .args(args)
            .current_dir(&self.project)
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        assert!(
            o.status.success(),
            "{args:?}: {out}{}",
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn read(&self, f: &str) -> Option<String> {
        std::fs::read_to_string(self.project.join(f)).ok()
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn the_agent_works_on_a_copy_and_the_folder_changes_only_at_the_apply() {
    let Some(w) = world(SCRIPT, "") else { return };
    let o = w.start(&["--copy"]);
    let e = err(&o);
    assert!(o.status.success(), "{e}");
    // The folder of the user is as it was, with the pointer and nothing else.
    assert_eq!(w.read("a.txt").unwrap(), "a\n");
    assert_eq!(w.read("b.txt").unwrap(), "b\n");
    assert!(w.read("made.txt").is_none() && w.read("where.txt").is_none());
    let pointer: Vec<_> = std::fs::read_dir(w.project.join(".paddock"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        pointer,
        ["pointer.json"],
        "no note and no project data next to the pointer"
    );
    // The notes of the start and of the end.
    assert!(e.contains("the agent works on a copy of"), "{e}");
    assert!(
        e.contains("the copy has 1 changed, 3 new, 1 deleted"),
        "{e}"
    );
    assert!(
        e.contains("Your folder") && e.contains("did not change"),
        "{e}"
    );
    assert!(e.contains("  ~ changed   a.txt"), "{e}");
    assert!(e.contains("`paddock apply "), "{e}");
    // The list.
    let all = w.json(&["changes", "--json"]);
    let run = &all["runs"][0];
    assert_eq!(run["state"], "ended");
    assert_eq!(run["in_place"], true);
    assert!(run["ended"].is_u64());
    let id = run["id"].as_str().unwrap().to_string();
    let tree = PathBuf::from(run["copy"].as_str().unwrap());
    // The sandbox saw the copy at the path of the project.
    assert_eq!(
        std::fs::read_to_string(tree.join("where.txt"))
            .unwrap()
            .trim(),
        w.project.display().to_string()
    );
    // Its notes folder is the store of the notes, not the pointer.
    let seen = std::fs::read_to_string(tree.join("notes-seen.txt")).unwrap();
    assert!(!seen.contains("pointer.json"), "{seen}");
    let v = w.json(&["changes", &id, "--json"]);
    let rows: Vec<(String, String)> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["path"].as_str().unwrap().into(),
                c["kind"].as_str().unwrap().into(),
            )
        })
        .collect();
    let want = [
        ("a.txt", "changed"),
        ("b.txt", "deleted"),
        ("made.txt", "new"),
        ("notes-seen.txt", "new"),
        ("where.txt", "new"),
    ];
    assert_eq!(
        rows,
        want.map(|(p, k)| (p.to_string(), k.to_string())),
        "the note in .paddock is no change"
    );
    // The apply, and its undo.
    let done = w.json(&["apply", &id, "--yes", "--json"]);
    assert_eq!(done["written"].as_array().unwrap().len(), 4);
    assert_eq!(done["deleted"], serde_json::json!(["b.txt"]));
    assert_eq!(w.read("a.txt").unwrap(), "by the agent\n");
    assert_eq!(w.read("made.txt").unwrap(), "new\n");
    assert!(w.read("b.txt").is_none());
    let back = w.json(&["undo", "--yes", "--json"]);
    assert_eq!(back["api_version"], "paddock.undo/v1");
    assert_eq!(w.read("a.txt").unwrap(), "a\n");
    assert_eq!(w.read("b.txt").unwrap(), "b\n");
    assert!(w.read("made.txt").is_none());
    assert_eq!(w.json(&["changes", &id, "--json"])["counts"]["ready"], 5);
}

#[test]
fn a_folder_without_git_gets_a_copy_by_default_and_direct_turns_it_off() {
    let Some(w) = world("printf 'by the agent\\n' > a.txt\n", "") else {
        return;
    };
    let o = w.start(&[]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(
        err(&o).contains("the agent works on a copy of"),
        "{}",
        err(&o)
    );
    assert_eq!(w.read("a.txt").unwrap(), "a\n");
    assert_eq!(
        w.json(&["changes", "--json"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // `--direct`: the run writes the folder itself, as before, and makes no
    // copy.
    let o = w.start(&["--direct"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(!err(&o).contains("works on a copy"), "{}", err(&o));
    assert_eq!(w.read("a.txt").unwrap(), "by the agent\n");
    assert_eq!(
        w.json(&["changes", "--json"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_run_without_changes_leaves_no_copy_behind() {
    let Some(w) = world("cat a.txt > /dev/null\n", "") else {
        return;
    };
    let o = w.start(&["--copy"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(
        err(&o).contains("the copy has no open change"),
        "{}",
        err(&o)
    );
    assert_eq!(
        w.json(&["changes", "--json"])["runs"],
        serde_json::json!([])
    );
    // The backup of the start stays.
    let b = w.json(&["backups", "--json"]);
    assert_eq!(b["backups"].as_array().unwrap().len(), 1);
    assert_eq!(b["backups"][0]["kind"], "start");
}

#[test]
fn the_setting_applies_at_the_end_of_a_run_without_a_conflict() {
    let Some(w) = world(SCRIPT, "[copy]\napply = \"auto\"\n") else {
        return;
    };
    let o = w.start(&["--copy"]);
    let e = err(&o);
    assert!(o.status.success(), "{e}");
    assert!(e.contains("applied 5 change(s)"), "{e}");
    assert!(e.contains("`paddock undo` takes the apply back"), "{e}");
    assert_eq!(w.read("a.txt").unwrap(), "by the agent\n");
    assert!(w.read("b.txt").is_none());
    let kinds: Vec<String> = w.json(&["backups", "--json"])["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["apply", "start"], "a backup came before the apply");
}

#[test]
fn a_git_project_stays_direct_until_the_user_asks_for_a_copy() {
    let Some(w) = world("printf 'by the agent\\n' > a.txt\n", "") else {
        return;
    };
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&w.project)
            .env("HOME", w.root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !git(&["init", "-q"]) {
        eprintln!("skipped: git is not here");
        return;
    }
    let o = w.start(&[]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(
        w.read("a.txt").unwrap(),
        "by the agent\n",
        "the run wrote the folder"
    );
    std::fs::write(w.project.join("a.txt"), "a\n").unwrap();
    let o = w.start(&["--copy"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(w.read("a.txt").unwrap(), "a\n");
    let all = w.json(&["changes", "--json"]);
    assert_eq!(all["runs"][0]["git"], true);
    // `.git` came along, so git works in the copy.
    let tree = PathBuf::from(all["runs"][0]["copy"].as_str().unwrap());
    assert!(tree.join(".git/HEAD").is_file());
}
