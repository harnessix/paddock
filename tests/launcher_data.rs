//! The data of the launcher motions and the file screens, with the built
//! paddock on temporary homes: `unread_ask` of `mail ls --json`, the loft
//! fields of `board --json`, `backups --files`, `restore --plan` and the
//! merge preview of `changes RUN --json`. Each output also has the shape
//! of its fixture in `tests/fixtures/launcher/json/`, which the launcher
//! reads.

mod common;

use common::copy_world::{get, put, strs, text, world, ID};
use paddock::env::HostEnv;
use paddock::mail::{self, Dir, Kind, Message, Starter};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A fixture of `tests/fixtures/launcher/json/`.
fn fixture(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/launcher/json")
        .join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every field of the fixture is in the output, with the same JSON type.
/// The first item of a list stands for every item. A `null` of the
/// fixture takes any value.
fn has_shape(want: &Value, got: &Value, at: &str) {
    match (want, got) {
        (Value::Null, _) => {}
        (Value::Object(w), Value::Object(g)) => {
            for (k, wv) in w {
                let gv = g
                    .get(k)
                    .unwrap_or_else(|| panic!("{at}.{k} is missing: {got}"));
                has_shape(wv, gv, &format!("{at}.{k}"));
            }
        }
        (Value::Array(w), Value::Array(g)) => {
            if let Some(w0) = w.first() {
                assert!(!g.is_empty(), "{at} is empty, the fixture has items");
                for (i, gv) in g.iter().enumerate() {
                    has_shape(w0, gv, &format!("{at}[{i}]"));
                }
            }
        }
        (Value::Bool(_), Value::Bool(_))
        | (Value::String(_), Value::String(_))
        | (Value::Number(_), Value::Number(_)) => {}
        _ => panic!("{at}: the fixture has {want}, paddock prints {got}"),
    }
}

/// Temporary folders for the built paddock and the library.
struct Home {
    _tmp: common::TestDir,
    root: PathBuf,
}

const VARS: [(&str, &str); 5] = [
    ("XDG_CONFIG_HOME", "cfg"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_RUNTIME_DIR", "run"),
];

impl Home {
    fn new() -> Home {
        let tmp = common::tempdir();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for d in ["home", "cfg", "data", "state", "cache"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let run = root.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        Home { _tmp: tmp, root }
    }

    fn env(&self) -> HostEnv {
        let mut vars: std::collections::BTreeMap<String, String> = VARS
            .iter()
            .map(|(k, d)| (k.to_string(), self.root.join(d).display().to_string()))
            .collect();
        vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        HostEnv::from_parts(self.root.join("home"), "t".into(), uid, gid, vars)
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.args(args)
            .env("HOME", self.root.join("home"))
            .env_remove("PADDOCK_CALLER")
            .env_remove("TMUX")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .current_dir(self.root.join("home"))
            .stdin(Stdio::null());
        for (k, d) in VARS {
            c.env(k, self.root.join(d));
        }
        let o = c.output().unwrap();
        let (out, err) = (text(&o.stdout), text(&o.stderr));
        assert!(o.status.success(), "{args:?}: {out}{err}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }
}

#[test]
fn mail_ls_counts_the_unread_questions_of_the_agent() {
    let h = Home::new();
    let env = h.env();
    let run = "fake-proj-a1b2c3";
    mail::create(&env, run, &Starter::host()).unwrap();
    let up = |kind: Kind, body: &str| {
        mail::append(&env, run, Dir::Up, Message::new("fake", kind, body))
            .unwrap()
            .seq
    };
    let first_ask = up(Kind::Ask, "which port?");
    up(Kind::Say, "I start the server");
    up(Kind::Status, "working");
    let rows = h.json(&["mail", "ls", "--json"]);
    let row = &rows[0];
    assert_eq!(row["id"], run);
    assert_eq!(row["unread_up"], 3);
    assert_eq!(row["unread_ask"], 1);
    has_shape(&fixture("mail-ls.json"), &rows, "mail ls");
    // The starter read the first question; a second one comes after.
    mail::set_cursor(&env, run, Dir::Up, mail::STARTER, first_ask).unwrap();
    up(Kind::Ask, "and the host?");
    up(Kind::Ask, "and the user?");
    let row = &h.json(&["mail", "ls", "--json"])[0];
    assert_eq!(row["unread_up"], 4);
    assert_eq!(row["unread_ask"], 2);
    // A run without unread mail has 0, not a missing field.
    let last = mail::last_seq(&env, run);
    mail::set_cursor(&env, run, Dir::Up, mail::STARTER, last).unwrap();
    let row = &h.json(&["mail", "ls", "--json"])[0];
    assert_eq!(row["unread_ask"], 0);
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn board_counts_the_loft_commits_of_each_run() {
    let h = Home::new();
    let env = h.env();
    let project = h.root.join("src/alpha");
    std::fs::create_dir_all(&project).unwrap();
    // Two running runs of this test process: one with commits, one without.
    let ids = ["claude-alpha-71c3ac", "codex-alpha-9d8e7f"];
    for id in ids {
        let info = serde_json::json!({
            "id": id, "harness": id.split('-').next().unwrap(), "project": project,
            "backend": "bwrap", "network": "egress", "gui": "none",
            "pid": std::process::id(), "started": 1,
        });
        write(
            &h.root.join(format!("run/paddock/{id}/info.json")),
            &info.to_string(),
        );
    }
    let loft = paddock::loft::Loft::of(&env, &project);
    paddock::loft::git::init(&loft).unwrap();
    for (n, body) in ["one", "two"].iter().enumerate() {
        std::fs::create_dir_all(loft.notes()).unwrap();
        std::fs::write(loft.notes().join("plan.md"), body).unwrap();
        assert!(
            paddock::loft::git::commit(&loft, &format!("Run {} (claude)", ids[0])).unwrap(),
            "commit {n}"
        );
    }
    // A commit by hand and one of another run do not count for it.
    std::fs::write(loft.notes().join("plan.md"), "three").unwrap();
    assert!(paddock::loft::git::commit(&loft, "by hand").unwrap());
    std::fs::write(loft.notes().join("plan.md"), "four").unwrap();
    assert!(paddock::loft::git::commit(&loft, "Run claude-alpha-71c3acd (claude)").unwrap());

    let v = h.json(&["board", "--json"]);
    // The fixture shows a run with a commit: `loft_last` is absent without.
    let mut with_commits = v.clone();
    with_commits["projects"][0]["runs_without_ticket"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r["loft_commits"] != 0);
    has_shape(&fixture("board.json"), &with_commits, "board");
    let runs = v["projects"][0]["runs_without_ticket"].as_array().unwrap();
    let by_id = |short: &str| runs.iter().find(|r| r["id"] == short).unwrap().clone();
    let with = by_id("71c3ac");
    assert_eq!(with["loft_commits"], 2);
    let last = with["loft_last"].as_str().unwrap();
    assert!(last.ends_with('Z') && last.len() == 20, "{last}");
    let without = by_id("9d8e7f");
    assert_eq!(without["loft_commits"], 0);
    assert!(without.get("loft_last").is_none(), "{without}");
}

#[test]
fn backups_files_lists_each_path_of_a_backup() {
    let w = world();
    let run = w.start(ID);
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "docs/new.md", "new\n");
    w.json(&["apply", "1a2b3c", "--yes", "--json"]);
    let list = w.json(&["backups", "--json"]);
    let backups = list["backups"].as_array().unwrap();
    let id_of = |kind: &str| {
        backups.iter().find(|b| b["kind"] == kind).unwrap()["short"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // The start of the run: every file of the project.
    let v = w.json(&["backups", "--files", &id_of("start"), "--json"]);
    assert_eq!(v["api_version"], "paddock.backup-files/v1");
    assert_eq!(v["backup"]["kind"], "start");
    has_shape(&fixture("backup-files.json"), &v, "backups --files");
    let paths: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["README.md", "src/cart.ts", "src/price.ts"]);
    assert_eq!(v["files"][1]["size"], "one\ntwo\nthree\n".len());
    assert_eq!(v["files"][1]["kind"], "file");
    // Before the apply: the replaced file, and the new file as absent.
    let v = w.json(&["backups", "--files", &id_of("apply"), "--json"]);
    let files = v["files"].as_array().unwrap();
    let of = |p: &str| files.iter().find(|f| f["path"] == p).unwrap().clone();
    assert_eq!(of("src/cart.ts")["kind"], "file");
    assert_eq!(of("docs/new.md")["kind"], "absent");
    assert!(of("docs/new.md")["size"].is_null());
    // The text form, and an unknown id.
    let o = w.run(&["backups", "--files", &id_of("start")]);
    assert!(o.status.success());
    assert!(
        text(&o.stdout).contains("src/cart.ts"),
        "{}",
        text(&o.stdout)
    );
    let o = w.run(&["backups", "--files", "ffffff", "--json"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("no backup ffffff"),
        "{}",
        text(&o.stderr)
    );
}

/// Every file of a folder with its bytes.
fn files_of(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(files_of(&p));
        } else {
            out.push((p.clone(), std::fs::read(&p).unwrap()));
        }
    }
    out.sort();
    out
}

#[test]
fn restore_plan_names_the_paths_and_changes_nothing() {
    let w = world();
    let run = w.start(ID);
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "docs/new.md", "new\n");
    w.json(&["apply", "1a2b3c", "--yes", "--json"]);
    let start = paddock::ids::short(&run.rec.base).to_string();
    let before = files_of(&w.root);
    let v = w.json(&["restore", &start, "--plan", "--json"]);
    assert_eq!(v["api_version"], "paddock.restore-plan/v1");
    assert_eq!(v["of"], run.rec.base.as_str());
    assert_eq!(v["kind"], "start");
    assert_eq!(v["exact"], false);
    assert_eq!(strs(&v["write"]), ["src/cart.ts"]);
    assert_eq!(strs(&v["remove"]), Vec::<&str>::new());
    assert_eq!(strs(&v["keep"]), ["docs/new.md"]);
    has_shape(&fixture("restore-plan.json"), &v, "restore --plan");
    let v = w.json(&["restore", &start, "--plan", "--exact", "--json"]);
    assert_eq!(strs(&v["remove"]), ["docs/new.md"]);
    assert_eq!(strs(&v["keep"]), Vec::<&str>::new());
    // The text form, too, without --yes and without a terminal.
    let o = w.run(&["restore", &start, "--plan"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("1 come back: src/cart.ts"));
    assert_eq!(files_of(&w.root), before, "a plan changes no file");
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    // --plan and --yes do not go together; --json still needs one of them.
    assert!(!w
        .run(&["restore", &start, "--plan", "--yes"])
        .status
        .success());
    let o = w.run(&["restore", &start, "--json"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
}

#[test]
fn a_conflict_that_merges_clean_has_a_preview() {
    let w = world();
    let run = w.start(ID);
    // Both sides change src/cart.ts, on different lines: a clean merge.
    put(&run.tree(), "src/cart.ts", "ONE\ntwo\nthree\n");
    put(&w.project, "src/cart.ts", "one\ntwo\nTHREE\n");
    // Both change src/price.ts on the same line: no clean merge.
    put(&run.tree(), "src/price.ts", "the copy\n");
    put(&w.project, "src/price.ts", "the user\n");
    let v = w.json(&["changes", "1a2b3c", "--json"]);
    let by_path = |p: &str| {
        v["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == p)
            .unwrap()
            .clone()
    };
    let clean = by_path("src/cart.ts");
    assert_eq!(clean["state"], "conflict");
    assert_eq!(clean["merge"], "clean");
    assert_eq!(clean["merge_preview"], "ONE\ntwo\nTHREE\n");
    assert_eq!(clean["merge_preview_cut"], false);
    has_shape(&fixture("changes-merge.json"), &clean, "changes");
    let hard = by_path("src/price.ts");
    assert_eq!(hard["merge"], "conflicts");
    assert!(hard["merge_preview"].is_null() && hard["merge_preview_cut"].is_null());
    // Nothing was written: the preview is only text.
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nTHREE\n");
    // --brief reads no file, so it has no preview.
    let v = w.json(&["changes", "1a2b3c", "--json", "--brief"]);
    assert!(v["changes"][0].get("merge_preview").is_none());
}

#[test]
fn a_long_merge_preview_is_cut_at_a_character() {
    use paddock::copy::json::{preview, MERGE_PREVIEW_MAX};
    assert_eq!(preview(b"short"), (Some("short"), false));
    let mut long = "a".repeat(MERGE_PREVIEW_MAX - 1).into_bytes();
    long.extend("é and more".as_bytes());
    let (text, cut) = preview(&long);
    assert!(cut);
    assert_eq!(
        text.unwrap().len(),
        MERGE_PREVIEW_MAX - 1,
        "the é is not split"
    );
    assert_eq!(preview(b"\xff\xfe"), (None, false));
}
