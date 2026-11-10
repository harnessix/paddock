//! agy in horse mode: a headless child under live stream-json custody
//! with the fake agy of `tests/fixtures/horse/agy-stream` as a real
//! process on pipes, the pty custody of a TUI run or one line, and the
//! final ingest of a run without either.
use super::live_tests::{fixture, headless, spawn_armed, wait_until, ARMS};
use super::*;
use crate::horse::agy;

/// A spawned agy child whose task is in its prompt file, with the
/// built-in profile: `agy --dangerously-skip-permissions --print TEXT`.
fn child(t: &std::path::Path, task: &str) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "agy", agy::VERSION);
    headless(&mut b, task);
    b.stdin = None;
    b.spec.id = "child".into();
    b.profile = crate::profile::builtin("agy").unwrap();
    b.harness_exe = fixture("agy-stream");
    let rel = crate::spawn::prompt_rel(&b.spec.id);
    let file = b.spec.home_src.join(&rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, task).unwrap();
    let one = b.profile.oneshot.clone().unwrap();
    b.spec.command = vec![fixture("agy-stream").display().to_string()];
    b.spec.command.extend(one.render(&b.spec.home.join(rel)));
    b.spec
        .command
        .extend(["--model".into(), "gemini-3-pro".into()]);
    (env, b)
}

#[test]
fn a_headless_agy_run_is_held_live_and_gets_an_entry_after_its_turn() {
    if !crate::horse::live::agy::tests::python() {
        eprintln!("skipped: needs python3");
        return;
    }
    let _armed = ARMS.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = child(t.path(), "do the task\n");
    assert!(run::setup::headless(&b));
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::Live);
    assert_eq!(
        b.spec.command[1..],
        [
            "--dangerously-skip-permissions",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--model",
            "gemini-3-pro"
        ]
    );
    let store = Store::open_host(&env).unwrap();
    let session = l.run.session.clone();
    let inbox = store.inbox(&session).unwrap();
    assert_eq!(inbox.len(), 2);
    assert_eq!(
        (inbox[1].role.as_str(), inbox[1].text.as_str()),
        ("user", "do the task")
    );
    let mut child = spawn_armed(&b, &[("FAKE_AGY_HOLD", "1")]);
    let home = b.spec.home_src.clone();
    // The task turn runs and holds: its conversation is bound.
    wait_until("the bound conversation", || {
        store.session(&session).unwrap().binding.native_id.is_some()
    });
    wait_until("the task line", || {
        std::fs::read_to_string(home.join("agy-input.log")).is_ok_and(|s| s.contains("do the task"))
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
    let native = store.session(&session).unwrap().binding.native_id.unwrap();
    assert!(
        out.contains(&format!("paddock: agy conversation {native} in custody")),
        "{out}"
    );
    assert!(store.inbox(&session).unwrap().is_empty());
    let s = store.session(&session).unwrap();
    let users: Vec<String> = store
        .session_nodes(&s)
        .unwrap()
        .iter()
        .filter_map(|n| crate::horse::live::agy::user_text(&n.raw))
        .collect();
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(users[0].starts_with("<paddock-append role=\"system\">"));
    // The text of the prompt file goes in, not the `--print` text that
    // names the file.
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
fn a_headless_agy_command_that_sets_the_stream_fails_before_the_store() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = child(t.path(), "do the task\n");
    b.spec
        .command
        .extend(["--conversation".into(), agy::fixture::ID.into()]);
    let before = b.spec.command.clone();
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("must not set --conversation"), "{e}");
    assert!(!crate::horse::root(&env).exists());
    assert_eq!(b.spec.command, before);
    let (env, mut b) = child(t.path(), " \n");
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("the task of the run is empty"), "{e}");
    assert!(!crate::horse::root(&env).exists());
}

/// An interactive agy run with the built-in profile.
fn tui(t: &std::path::Path) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "agy", agy::VERSION);
    b.horse = true;
    b.profile = crate::profile::builtin("agy").unwrap();
    b.spec.command = ["agy", "--dangerously-skip-permissions"]
        .map(String::from)
        .to_vec();
    (env, b)
}

fn hook_settings(b: &Built) -> Vec<&crate::spec::HomeSetting> {
    b.spec
        .home_settings
        .iter()
        .filter(|s| s.origin.contains("agy TUI"))
        .collect()
}

#[test]
fn an_interactive_agy_run_gets_the_pty_custody_and_its_hooks_or_one_line() {
    let t = tempfile::tempdir().unwrap();
    let v = agy::VERSION;
    let choose = run::setup::choose_with;
    let (env, mut b) = tui(t.path());
    assert!(!run::setup::headless(&b));
    assert_eq!(choose(&b, v, true), (Custody::Pty, None));
    let (k, l) = choose(&b, v, false);
    assert_eq!(k, Custody::FinalOnly);
    assert_eq!(
        l.as_deref(),
        Some("horse: no safe-point delivery for agy 1.4.12 in the TUI (the run has no terminal for paddock's relay); entries stay queued")
    );
    let before = b.spec.command.clone();
    assert!(run::setup::prepare(&mut b, Custody::Pty, v)
        .unwrap()
        .is_some());
    assert_eq!(b.spec.command, before, "the command line stays");
    let hooks = hook_settings(&b);
    assert_eq!(hooks.len(), 1);
    let s = hooks[0];
    assert_eq!(s.file, std::path::Path::new(".gemini/config/hooks.json"));
    assert_eq!(s.path, ["paddock-horse"]);
    assert_eq!(
        s.value["PreInvocation"][0]["command"],
        "paddock-host horse safe-point prompt --stdin || true"
    );
    assert_eq!(
        s.value["Stop"][0]["command"],
        "paddock-host horse safe-point stop --stdin || true"
    );
    // A profile without the table for this version.
    let (_env, d) = super::tests::setup(t.path(), "agy", v);
    let (k, l) = choose(&d, v, true);
    assert_eq!(k, Custody::FinalOnly);
    assert!(l
        .unwrap()
        .contains("no pty delivery pattern for agy 1.4.12"));
    // Horse mode off: no store, no hooks.
    let (_env, mut off) = tui(t.path());
    off.horse = false;
    assert!(start(&env, &mut off).unwrap().is_none());
    assert!(hook_settings(&off).is_empty());
}

#[test]
fn a_final_only_agy_run_ingests_its_conversation_database_at_the_end() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = tui(t.path());
    // No terminal in a test: final-only.
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::FinalOnly);
    let mut f = agy::fixture::Fixture::new(agy::fixture::ID);
    f.user("hi").reply("hello");
    f.write(&b.spec.home_src.join(".gemini/antigravity-cli"));
    let store = Store::open_host(&env).unwrap();
    let (horse, session) = (l.run.horse.clone(), l.run.session.clone());
    assert_eq!(end(Some(l), 0, None), None);
    let s = store.session(&session).unwrap();
    assert_eq!(s.binding.native_id.as_deref(), Some(agy::fixture::ID));
    // Two steps and the gen_metadata row of the reply.
    assert_eq!(store.session_nodes(&s).unwrap().len(), 3);
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["custody"], "final-only");
    assert_eq!(e["nodes"], 3);
}
