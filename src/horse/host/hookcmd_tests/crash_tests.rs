//! Adoptions after a crash, a moved program or another path of the home.

use super::*;

#[test]
fn a_crash_after_the_record_leaves_no_entry_that_off_cannot_take_out() {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let state = t.path().join("state");
    let p = home.join(".claude/settings.json");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "{\"model\": \"opus\"}\n").unwrap();
    // paddock dies after the state is saved, before the hook file changes.
    adopt::CRASH_BEFORE_HOOK_FILE.with(|c| c.set(true));
    let r = adopt::adopt(
        &store,
        &state,
        claude::HARNESS,
        &home,
        &[],
        Some(Path::new(EXE)),
    );
    adopt::CRASH_BEFORE_HOOK_FILE.with(|c| c.set(false));
    assert!(r.is_err());
    // The state knows the entries before any is in the file.
    let a = adopt::load(&state).unwrap().harnesses[claude::HARNESS].clone();
    assert_eq!(a.hooks.unwrap().entries.len(), 3);
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "{\"model\": \"opus\"}\n"
    );
    // So the order of the old code (file first, record after) cannot leave
    // orphans: --off works, and a new adoption is not refused.
    assert!(adopt::off(&state, claude::HARNESS).unwrap().is_some());
    adopt::adopt(
        &store,
        &state,
        claude::HARNESS,
        &home,
        &[],
        Some(Path::new(EXE)),
    )
    .unwrap();
    assert_eq!(
        adopt::off(&state, claude::HARNESS).unwrap().unwrap().1,
        Some(Removed::Restored)
    );
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "{\"model\": \"opus\"}\n"
    );
}

/// The commands of paddock's Claude Code entries in `settings`.
fn commands_in(settings: &Path) -> Vec<String> {
    let d: Value = serde_json::from_slice(&std::fs::read(settings).unwrap()).unwrap();
    ["SessionStart", "UserPromptSubmit", "Stop"]
        .iter()
        .flat_map(|e| {
            d["hooks"][e]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                .filter_map(|h| h["command"].as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn an_adopt_after_a_crash_between_the_record_and_the_file_puts_the_entries_in() {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let state = t.path().join("state");
    // A program that is there: the record is right in all but the file.
    let exe = t.path().join("bin/paddock");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    let p = home.join(".claude/settings.json");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let settings = "{\"model\": \"opus\"}\n";
    std::fs::write(&p, settings).unwrap();
    write_claude(&home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    // paddock dies after the save of the record, before the hook file.
    adopt::CRASH_BEFORE_HOOK_FILE.with(|c| c.set(true));
    let r = adopt::adopt(&store, &state, claude::HARNESS, &home, &[], Some(&exe));
    adopt::CRASH_BEFORE_HOOK_FILE.with(|c| c.set(false));
    assert!(r.is_err());
    let a = adopt::load(&state).unwrap().harnesses[claude::HARNESS].clone();
    let rec = a.hooks.unwrap();
    assert_eq!(rec.entries.len(), 3);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), settings);
    assert!(!rec.program_missing());
    assert_eq!(hooks::missing(&home, &rec).unwrap().len(), 3);

    // The user adopts again: the record matches the program, but the file
    // has no entries. The adopt reads the file and puts them in.
    adopt::adopt(&store, &state, claude::HARNESS, &home, &[], Some(&exe)).unwrap();
    let want: Vec<String> = ["SessionStart", "UserPromptSubmit", "Stop"]
        .iter()
        .map(|e| hooks::command(&exe, claude::HARNESS, e))
        .collect();
    assert_eq!(commands_in(&p), want);
    let a = adopt::load(&state).unwrap().harnesses[claude::HARNESS].clone();
    assert!(hooks::missing(&home, &a.hooks.unwrap()).unwrap().is_empty());
    // A third adopt changes nothing: the entries are there once.
    let before = std::fs::read(&p).unwrap();
    adopt::adopt(&store, &state, claude::HARNESS, &home, &[], Some(&exe)).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), before);
    // Off gives the file back as it was.
    assert_eq!(
        adopt::off(&state, claude::HARNESS).unwrap().unwrap().1,
        Some(Removed::Restored)
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), settings);
}

#[test]
fn an_adopt_after_a_crash_in_the_repair_puts_the_entries_in() {
    let t = tempfile::tempdir().unwrap();
    let old = t.path().join("old/paddock");
    let new = t.path().join("new/paddock");
    for p in [&old, &new] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
    }
    let h = adopted(claude::HARNESS);
    let settings = h.home.join(".claude/settings.json");
    adopt::off(&h.state, claude::HARNESS).unwrap();
    let before_adoption = "{\n  \"model\": \"opus\"\n}\n";
    std::fs::write(&settings, before_adoption).unwrap();
    adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[],
        Some(&old),
    )
    .unwrap();
    std::fs::remove_file(&old).unwrap();
    // The repair for the moved program takes the old entries out, saves
    // the record of the new ones, and dies before the hook file changes.
    adopt::CRASH_BEFORE_HOOK_FILE.with(|c| c.set(true));
    let r = adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[],
        Some(&new),
    );
    adopt::CRASH_BEFORE_HOOK_FILE.with(|c| c.set(false));
    assert!(r.is_err());
    assert!(commands_in(&settings).is_empty());
    // The record names the new program already; the file has nothing.
    adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[],
        Some(&new),
    )
    .unwrap();
    let cmds = commands_in(&settings);
    assert_eq!(cmds.len(), 3, "{cmds:?}");
    assert!(cmds.iter().all(|c| c.starts_with(new.to_str().unwrap())));
    adopt::off(&h.state, claude::HARNESS).unwrap();
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), before_adoption);
}

#[test]
fn a_second_adopt_puts_the_entries_in_again_for_a_moved_program() {
    let t = tempfile::tempdir().unwrap();
    let old = t.path().join("old/paddock");
    let new = t.path().join("new/paddock");
    for p in [&old, &new] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
    }
    let h = adopted(claude::HARNESS);
    let settings = h.home.join(".claude/settings.json");
    let before_adoption = "{\n  \"model\": \"opus\"\n}\n";
    adopt::off(&h.state, claude::HARNESS).unwrap();
    std::fs::write(&settings, before_adoption).unwrap();
    adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[],
        Some(&old),
    )
    .unwrap();
    // An upgrade moves the program.
    std::fs::remove_file(&old).unwrap();
    let a = adopt::load(&h.state).unwrap().harnesses[claude::HARNESS].clone();
    assert!(a.hooks.unwrap().program_missing());
    adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &h.home,
        &[],
        Some(&new),
    )
    .unwrap();
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(!text.contains(old.to_str().unwrap()), "{text}");
    assert_eq!(text.matches(new.to_str().unwrap()).count(), 3, "{text}");
    let a = adopt::load(&h.state).unwrap().harnesses[claude::HARNESS].clone();
    let i = a.hooks.unwrap();
    assert!(!i.program_missing());
    assert!(i
        .entries
        .iter()
        .all(|e| e.command.starts_with(new.to_str().unwrap())));
    // Off gives the file back as it was before the adoption.
    adopt::off(&h.state, claude::HARNESS).unwrap();
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), before_adoption);
}

#[test]
fn a_home_by_another_path_is_the_same_adoption() {
    let h = adopted(claude::HARNESS);
    let other = h.home.join("..").join(h.home.file_name().unwrap());
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    let s = adopt::adopt(
        &h.store,
        &h.state,
        claude::HARNESS,
        &other,
        &[],
        Some(Path::new(EXE)),
    )
    .unwrap();
    assert_eq!(s.imported.len(), 1);
}
