use super::fixtures::tree;
use super::hooks::{self, Removed};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Home {
    _t: tempfile::TempDir,
    home: PathBuf,
    backups: PathBuf,
}

fn home() -> Home {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let backups = t.path().join("state/horse/hooks-backup");
    Home {
        _t: t,
        home,
        backups,
    }
}

const EXE: &str = "/opt/paddock bin/paddock";

/// A settings file of a user, in the user's own layout.
const SETTINGS: &str = r#"{
    "model": "opus",
    "hooks": {
        "Stop": [
            { "matcher": "", "hooks": [ { "type": "command", "command": "notify-send done" } ] }
        ],
        "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "audit" } ] } ]
    },
    "permissions": { "allow": ["Bash(ls)"] }
}
"#;

fn write(h: &Home, rel: &str, text: &str, mode: u32) -> PathBuf {
    let p = h.home.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    p
}

fn doc(p: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn adoption_adds_marked_entries_after_the_users_and_off_restores_the_bytes() {
    let h = home();
    let p = write(&h, ".claude/settings.json", SETTINGS, 0o644);
    let i = hooks::install(&h.home, &h.backups, "claude", Path::new(EXE))
        .unwrap()
        .unwrap();
    assert!(!i.created);
    assert_eq!(i.file, PathBuf::from(".claude/settings.json"));
    // The backup is the file before the change, byte for byte, and private.
    let b = i.backup.clone().unwrap();
    assert!(b.starts_with(&h.backups));
    assert_eq!(std::fs::read_to_string(&b).unwrap(), SETTINGS);
    assert_eq!(
        std::fs::metadata(&b).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // The mode of the file stays.
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let d = doc(&p);
    let old: Value = serde_json::from_str(SETTINGS).unwrap();
    assert_eq!(d["model"], old["model"]);
    assert_eq!(d["permissions"], old["permissions"]);
    assert_eq!(d["hooks"]["PreToolUse"], old["hooks"]["PreToolUse"]);
    // The user's Stop group keeps its place; paddock's group comes after it.
    let stop = d["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0], old["hooks"]["Stop"][0]);
    let cmd = stop[1]["hooks"][0]["command"].as_str().unwrap();
    assert_eq!(
        cmd,
        "'/opt/paddock bin/paddock' horse hook claude Stop --mark paddock-horse"
    );
    assert_eq!(cmd, hooks::command(Path::new(EXE), "claude", "Stop"));
    for e in ["SessionStart", "UserPromptSubmit"] {
        assert_eq!(d["hooks"][e].as_array().unwrap().len(), 1, "{e}");
    }
    assert_eq!(i.entries.len(), 3);

    // A second adoption refuses: the entries are there.
    assert!(hooks::install(&h.home, &h.backups, "claude", Path::new(EXE)).is_err());

    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Restored);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), SETTINGS);
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Nothing);
}

#[test]
fn off_after_a_change_of_the_user_takes_out_only_paddocks_entries() {
    let h = home();
    let p = write(&h, ".claude/settings.json", SETTINGS, 0o600);
    let i = hooks::install(&h.home, &h.backups, "claude", Path::new(EXE))
        .unwrap()
        .unwrap();
    // The user changes the file: a new key, and a new Stop group after
    // paddock's, and a command that only looks like paddock's.
    let mut d = doc(&p);
    d["theme"] = json!("dark");
    let look_alike =
        json!({"hooks": [{"type": "command", "command": "paddock horse hook claude Stop"}]});
    d["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(look_alike.clone());
    std::fs::write(&p, serde_json::to_string(&d).unwrap()).unwrap();

    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Stripped);
    let after = doc(&p);
    let old: Value = serde_json::from_str(SETTINGS).unwrap();
    assert_eq!(after["theme"], "dark");
    assert_eq!(after["model"], old["model"]);
    assert_eq!(after["hooks"]["PreToolUse"], old["hooks"]["PreToolUse"]);
    assert_eq!(
        after["hooks"]["Stop"],
        json!([old["hooks"]["Stop"][0], look_alike])
    );
    assert!(after["hooks"].get("SessionStart").is_none());
    assert!(after["hooks"].get("UserPromptSubmit").is_none());
    assert!(!serde_json::to_string(&after)
        .unwrap()
        .contains("--mark paddock-horse"));
}

#[test]
fn a_file_that_paddock_made_goes_again() {
    let h = home();
    let i = hooks::install(&h.home, &h.backups, "grok", Path::new("/usr/bin/paddock"))
        .unwrap()
        .unwrap();
    assert!(i.created);
    assert!(i.backup.is_none());
    let p = h.home.join(".grok/hooks/paddock-horse.json");
    assert_eq!(
        doc(&p)["hooks"]["Stop"][0]["hooks"][0]["command"],
        "/usr/bin/paddock horse hook grok Stop --mark paddock-horse"
    );
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Deleted);
    assert!(!p.exists());
    // Without its own file (dsh), a harness gets no hooks.
    assert!(hooks::install(&h.home, &h.backups, "dsh", Path::new(EXE))
        .unwrap()
        .is_none());
}

#[test]
fn a_file_that_paddock_made_stays_when_the_user_wrote_into_it() {
    let h = home();
    let i = hooks::install(&h.home, &h.backups, "codex", Path::new(EXE))
        .unwrap()
        .unwrap();
    let p = h.home.join(".codex/hooks.json");
    let mut d = doc(&p);
    d["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({"hooks": [{"type": "command", "command": "mine"}]}));
    std::fs::write(&p, serde_json::to_string(&d).unwrap()).unwrap();
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Stripped);
    assert_eq!(
        doc(&p),
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "mine"}]}]}})
    );
}

#[test]
fn codex_entries_of_the_user_keep_their_positions() {
    let h = home();
    let user = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"a"},{"type":"command","command":"b"}]}],"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"c"}]}]}}"#;
    write(&h, ".codex/hooks.json", user, 0o600);
    hooks::install(&h.home, &h.backups, "codex", Path::new(EXE)).unwrap();
    let d = doc(&h.home.join(".codex/hooks.json"));
    let old: Value = serde_json::from_str(user).unwrap();
    for e in ["Stop", "SessionStart"] {
        assert_eq!(d["hooks"][e][0], old["hooks"][e][0], "{e}");
        assert_eq!(d["hooks"][e].as_array().unwrap().len(), 2, "{e}");
    }
}

#[test]
fn agy_gets_a_named_entry_of_its_own() {
    let h = home();
    let user = r#"{"mine":{"Stop":[{"type":"command","command":"x"}]}}"#;
    let p = write(&h, ".gemini/config/hooks.json", user, 0o600);
    let i = hooks::install(&h.home, &h.backups, "agy", Path::new(EXE))
        .unwrap()
        .unwrap();
    let d = doc(&p);
    assert_eq!(
        d["mine"],
        serde_json::from_str::<Value>(user).unwrap()["mine"]
    );
    assert_eq!(
        d["paddock-horse"]["Stop"][0]["command"],
        hooks::command(Path::new(EXE), "agy", "Stop")
    );
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Restored);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), user);
    // A name of the user that is paddock's name: refused.
    write(
        &h,
        ".gemini/config/hooks.json",
        r#"{"paddock-horse":{}}"#,
        0o600,
    );
    assert!(hooks::install(&h.home, &h.backups, "agy", Path::new(EXE)).is_err());
}

#[test]
fn files_that_paddock_does_not_know_stay_as_they_are() {
    let h = home();
    let target = write(&h, "dotfiles/settings.json", SETTINGS, 0o600);
    std::fs::create_dir_all(h.home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&target, h.home.join(".claude/settings.json")).unwrap();
    type Case = Box<dyn Fn(&Home)>;
    let cases: Vec<Case> = vec![
        // A symlink (a dotfiles manager).
        Box::new(|_| {}),
        // Not JSON.
        Box::new(|h| {
            std::fs::remove_file(h.home.join(".claude/settings.json")).unwrap();
            write(h, ".claude/settings.json", "{ \"model\": opus, }", 0o600);
        }),
        // Not an object.
        Box::new(|h| {
            write(h, ".claude/settings.json", "[1, 2]", 0o600);
        }),
        // Hooks that are not an object, or an event that is not a list.
        Box::new(|h| {
            write(h, ".claude/settings.json", r#"{"hooks": []}"#, 0o600);
        }),
        Box::new(|h| {
            write(
                h,
                ".claude/settings.json",
                r#"{"hooks": {"Stop": {}}}"#,
                0o600,
            );
        }),
        // Entries of an adoption that paddock does not know.
        Box::new(|h| {
            write(
                h,
                ".claude/settings.json",
                r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/x/paddock horse hook claude Stop --mark paddock-horse"}]}]}}"#,
                0o600,
            );
        }),
    ];
    for (n, set) in cases.iter().enumerate() {
        set(&h);
        let before = tree(&h.home);
        assert!(
            hooks::install(&h.home, &h.backups, "claude", Path::new(EXE)).is_err(),
            "case {n}"
        );
        assert_eq!(tree(&h.home), before, "case {n}");
        assert!(!h.backups.exists() || std::fs::read_dir(&h.backups).unwrap().count() == 0);
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(), SETTINGS);
}

/// The bytes of `new` without the texts that paddock inserted.
fn without_inserts(new: &str, i: &hooks::Installed) -> String {
    let mut t = new.to_string();
    for ins in i.inserts.iter().rev() {
        assert_eq!(t.matches(ins.as_str()).count(), 1, "{ins}");
        t = t.replacen(ins.as_str(), "", 1);
    }
    t
}

/// A settings file in a layout of its own: keys in no order, four spaces,
/// a user Stop list that is empty, short lines on one line.
const ODD: &str = "{\n    \"zeta\": 1,\n    \"hooks\": {\n        \"PreToolUse\": [ { \"matcher\": \"Bash\", \"hooks\": [ { \"type\": \"command\", \"command\": \"audit\" } ] } ],\n        \"Stop\": []\n    },\n    \"alpha\": { \"b\": 2, \"a\": 1 }\n}\n";

#[test]
fn adoption_changes_no_byte_of_the_users_file_but_its_insertions() {
    let h = home();
    let p = write(&h, ".claude/settings.json", ODD, 0o600);
    let i = hooks::install(&h.home, &h.backups, "claude", Path::new(EXE))
        .unwrap()
        .unwrap();
    let new = std::fs::read_to_string(&p).unwrap();
    // Every other byte is the same: the keys keep their order and layout.
    assert_eq!(without_inserts(&new, &i), ODD);
    assert!(new.find("\"zeta\"").unwrap() < new.find("\"alpha\"").unwrap());
    assert!(new.contains("\"alpha\": { \"b\": 2, \"a\": 1 }"));
    // paddock's entries are in the layout of the file (four spaces).
    assert!(
        new.contains("\n            {\n                \"hooks\": [\n"),
        "{new}"
    );
    let d = doc(&p);
    assert_eq!(
        d["hooks"]["Stop"][0]["hooks"][0]["command"],
        hooks::command(Path::new(EXE), "claude", "Stop")
    );
    // The user's empty Stop list was not paddock's: it stays at --off.
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Restored);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), ODD);
}

#[test]
fn off_after_an_edit_of_the_user_keeps_the_edit_and_the_layout() {
    let h = home();
    let p = write(&h, ".claude/settings.json", SETTINGS, 0o600);
    let i = hooks::install(&h.home, &h.backups, "claude", Path::new(EXE))
        .unwrap()
        .unwrap();
    // The user edits the file by hand while the adoption is on.
    let edited = std::fs::read_to_string(&p)
        .unwrap()
        .replace("\"model\": \"opus\"", "\"model\": \"sonnet\"");
    std::fs::write(&p, &edited).unwrap();
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Stripped);
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        SETTINGS.replace("\"model\": \"opus\"", "\"model\": \"sonnet\"")
    );
}

#[test]
fn off_cuts_a_moved_entry_and_keeps_the_rest_byte_for_byte() {
    let h = home();
    let p = write(
        &h,
        ".codex/hooks.json",
        "{\"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"a\"}]}]}}",
        0o600,
    );
    let i = hooks::install(&h.home, &h.backups, "codex", Path::new(EXE))
        .unwrap()
        .unwrap();
    // The user moves paddock's Stop handler into the own group.
    let stop = hooks::command(Path::new(EXE), "codex", "Stop");
    let start = hooks::command(Path::new(EXE), "codex", "SessionStart");
    let moved = format!(
        "{{\"x\": 1, \"hooks\": {{\"Stop\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"a\"}}, {{\"type\": \"command\", \"command\": \"{stop}\"}}]}}], \"SessionStart\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"{start}\"}}]}}]}}}}"
    );
    std::fs::write(&p, &moved).unwrap();
    assert_eq!(hooks::remove(&h.home, &i).unwrap(), Removed::Stripped);
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "{\"x\": 1, \"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"a\"}]}]}}"
    );
}

#[test]
fn a_plan_writes_nothing_until_it_applies() {
    let h = home();
    let p = write(&h, ".claude/settings.json", SETTINGS, 0o600);
    let before = tree(&h.home);
    let plan = hooks::plan(&h.home, &h.backups, "claude", Path::new(EXE))
        .unwrap()
        .unwrap();
    assert_eq!(tree(&h.home), before);
    assert!(!h.backups.exists());
    let i = plan.installed().clone();
    assert_eq!(plan.apply().unwrap(), i);
    assert_eq!(
        std::fs::read_to_string(i.backup.unwrap()).unwrap(),
        SETTINGS
    );
    assert_ne!(std::fs::read_to_string(&p).unwrap(), SETTINGS);
}

#[test]
fn the_hook_program_is_the_name_on_path_that_leads_to_paddock() {
    let t = tempfile::tempdir().unwrap();
    // A versioned program and a link to it in a folder of PATH.
    let real = t.path().join("Cellar/paddock/1.2.3/bin/paddock");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "#!/bin/sh\n").unwrap();
    let bin = t.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&real, bin.join("paddock")).unwrap();
    let other = t.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("paddock"), "another\n").unwrap();
    assert_eq!(
        hooks::stable_program(&real, &[other.clone(), bin.clone()]),
        bin.join("paddock")
    );
    // No name on PATH leads to it: the program itself.
    assert_eq!(hooks::stable_program(&real, &[other]), real);
    // A moved program shows in the record.
    let h = home();
    let i = hooks::install(&h.home, &h.backups, "claude", &real)
        .unwrap()
        .unwrap();
    assert!(!i.program_missing());
    std::fs::remove_file(&real).unwrap();
    assert!(i.program_missing());
}
