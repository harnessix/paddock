//! Grok Build in horse mode: a headless child under live ACP custody with
//! the fake grok of `tests/fixtures/horse/grok-acp` as a real process on
//! pipes, the pty custody of a TUI run or one line, and the final ingest
//! of a run without either.
use super::live_tests::{fixture, headless, spawn_armed, wait_until, ARMS};
use super::*;
use crate::horse::grok;

/// A spawned Grok child whose task is in its prompt file, with the
/// built-in profile: `grok --always-approve --prompt-file FILE`.
fn child(t: &std::path::Path, task: &str) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "grok", grok::VERSION);
    headless(&mut b, task);
    b.stdin = None;
    b.spec.id = "child".into();
    b.profile = crate::profile::builtin("grok").unwrap();
    b.harness_exe = fixture("grok-acp");
    let rel = crate::spawn::prompt_rel(&b.spec.id);
    let file = b.spec.home_src.join(&rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, task).unwrap();
    let one = b.profile.oneshot.clone().unwrap();
    b.spec.command = vec![fixture("grok-acp").display().to_string()];
    b.spec.command.extend(one.render(&b.spec.home.join(rel)));
    (env, b)
}

#[test]
fn a_headless_grok_run_is_held_live_and_gets_an_entry_after_its_turn() {
    let _armed = ARMS.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = child(t.path(), "do the task\n");
    assert!(run::setup::headless(&b));
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::Live);
    assert_eq!(
        b.spec.command[1..],
        ["agent", "--always-approve", "--no-leader", "stdio"]
    );
    let store = Store::open_host(&env).unwrap();
    let session = l.run.session.clone();
    let inbox = store.inbox(&session).unwrap();
    assert_eq!(inbox.len(), 2);
    assert_eq!(
        (inbox[1].role.as_str(), inbox[1].text.as_str()),
        ("user", "do the task")
    );
    let mut child = spawn_armed(&b, &[("FAKE_GROK_HOLD", "1")]);
    let home = b.spec.home_src.clone();
    // The task turn runs and holds: its user row is in the folder.
    wait_until("the bound session", || {
        store.session(&session).unwrap().binding.native_id.is_some()
    });
    let native = store.session(&session).unwrap().binding.native_id.unwrap();
    let dir = grok::session_dir(
        &home.join(".grok"),
        &b.spec.workdir.display().to_string(),
        &native,
    );
    wait_until("the task row", || {
        std::fs::read_to_string(dir.join("updates.jsonl")).is_ok_and(|s| s.contains("do the task"))
    });
    store
        .queue_append(&session, "user", "from the test")
        .unwrap();
    std::fs::write(home.join("go"), "").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "the fake failed: {status}");
    assert!(!home.join("early").exists(), "an entry came during a turn");
    let horse = l.run.horse.clone();
    let out = end(Some(l), 0, None).expect("the custody output");
    assert!(
        out.contains(&format!("paddock: grok session {native} in custody")),
        "{out}"
    );
    assert!(store.inbox(&session).unwrap().is_empty());
    let s = store.session(&session).unwrap();
    let users: Vec<String> = store
        .session_nodes(&s)
        .unwrap()
        .iter()
        .filter_map(|n| {
            let v: serde_json::Value = serde_json::from_slice(&n.raw).ok()?;
            (v["type"] == "user").then(|| v["content"][0]["text"].as_str().unwrap().to_string())
        })
        .collect();
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(users[0].starts_with("<paddock-append role=\"system\">"));
    assert_eq!(users[1], "do the task");
    assert_eq!(users[2], "from the test");
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["type"], "run.end");
    assert_eq!(e["custody"], "live");
    assert!(e.get("error").is_none(), "{e}");
    assert_eq!(e["added"], 0, "the custody ingested every turn: {e}");
    assert_eq!(e["turns"], 3, "{e}");
    assert_eq!(e["deliveries"], 3, "{e}");
    assert_eq!(e["landed"], 3, "{e}");
    assert_eq!(e["session_id"], native.as_str(), "{e}");
    assert!(crate::horse::live::records(&store).unwrap().is_empty());
}

#[test]
fn a_headless_grok_command_with_arguments_after_its_task_fails_before_the_store() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = child(t.path(), "do the task\n");
    b.spec.command.extend(["--model".into(), "grok-4.6".into()]);
    let before = b.spec.command.clone();
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("arguments after its one-shot arguments"), "{e}");
    assert!(!crate::horse::root(&env).exists());
    assert_eq!(b.spec.command, before);
    // An empty task.
    let (env, mut b) = child(t.path(), " \n");
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("the task of the run is empty"), "{e}");
    assert!(!crate::horse::root(&env).exists());
}

/// An interactive Grok run with the built-in profile.
fn tui(t: &std::path::Path) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "grok", grok::VERSION);
    b.horse = true;
    b.profile = crate::profile::builtin("grok").unwrap();
    b.spec.command = ["grok", "--always-approve"].map(String::from).to_vec();
    (env, b)
}

fn hook_settings(b: &Built) -> Vec<&crate::spec::HomeSetting> {
    b.spec
        .home_settings
        .iter()
        .filter(|s| s.origin.contains("Grok Build TUI"))
        .collect()
}

#[test]
fn an_interactive_grok_run_gets_the_pty_custody_and_its_hooks_or_one_line() {
    let t = tempfile::tempdir().unwrap();
    let v = grok::VERSION;
    let choose = run::setup::choose_with;
    let (env, mut b) = tui(t.path());
    assert!(!run::setup::headless(&b));
    assert_eq!(choose(&b, v, true), (Custody::Pty, None));
    let (k, l) = choose(&b, v, false);
    assert_eq!(k, Custody::FinalOnly);
    assert_eq!(
        l.as_deref(),
        Some("horse: no safe-point delivery for grok 1.0.70 in the TUI (the run has no terminal for paddock's relay); entries stay queued")
    );
    let before = b.spec.command.clone();
    assert!(run::setup::prepare(&mut b, Custody::Pty, v)
        .unwrap()
        .is_some());
    assert_eq!(b.spec.command, before, "the command line stays");
    let hooks = hook_settings(&b);
    assert_eq!(hooks.len(), 5);
    for s in &hooks {
        assert_eq!(
            s.file,
            std::path::Path::new(".grok/hooks/paddock-compaction.json")
        );
        let cmd = s.value[0]["hooks"][0]["command"].as_str().unwrap();
        let kind = if s.path[1] == "UserPromptSubmit" {
            "prompt"
        } else {
            "stop"
        };
        assert_eq!(
            cmd,
            format!("paddock-host horse safe-point {kind} --stdin || true")
        );
    }
    // A profile without the table for this version.
    let (_env, d) = super::tests::setup(t.path(), "grok", v);
    let (k, l) = choose(&d, v, true);
    assert_eq!(k, Custody::FinalOnly);
    assert!(l
        .unwrap()
        .contains("no pty delivery pattern for grok 1.0.70"));
    // Horse mode off: no store, no hooks.
    let (_env, mut off) = tui(t.path());
    off.horse = false;
    assert!(start(&env, &mut off).unwrap().is_none());
    assert!(hook_settings(&off).is_empty());
}

#[test]
fn a_final_only_grok_run_ingests_its_session_folder_at_the_end() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = tui(t.path());
    // No terminal in a test: final-only.
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::FinalOnly);
    let mut f =
        grok::fixture::Fixture::new(grok::fixture::ID, &b.spec.workdir.display().to_string());
    f.prompt(0, "hi").reply("hello");
    f.write(&b.spec.home_src.join(".grok"));
    let store = Store::open_host(&env).unwrap();
    let (horse, session) = (l.run.horse.clone(), l.run.session.clone());
    assert_eq!(end(Some(l), 0, None), None);
    let s = store.session(&session).unwrap();
    assert_eq!(s.binding.native_id.as_deref(), Some(grok::fixture::ID));
    assert_eq!(
        store.session_nodes(&s).unwrap().len(),
        f.updates.len() + f.chat.len()
    );
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["custody"], "final-only");
    assert_eq!(e["nodes"], f.updates.len() + f.chat.len());
}
