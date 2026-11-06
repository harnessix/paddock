use super::*;
use serde_json::json;

/// The run events of a horse: the DNA events are left out.
fn events(store: &Store, horse: &str) -> Result<Vec<Value>> {
    Ok(super::events(store, horse)?
        .into_iter()
        .filter(|e| !e["type"].as_str().unwrap_or("").starts_with("dna."))
        .collect())
}

#[test]
fn begin_creates_a_fresh_horse_and_an_unbound_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let first = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let second = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    assert_ne!(first.horse, second.horse);
    let session = store.session(&first.session).unwrap();
    assert_eq!(session.horse, first.horse);
    assert_eq!(session.binding.native_id, None);
    assert_eq!(
        store.horse(&first.horse).unwrap().sessions,
        vec![first.session]
    );
}

#[test]
fn version_failure_writes_no_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert!(begin(&store, dsh::HARNESS, "unsupported", None, None).is_err());
    assert!(store.horses().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(dir.path().join("objects"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn events_are_ordered_and_separate_from_session_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let rh = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    record_event(&store, &rh, "run.begin", "sandbox", json!({})).unwrap();
    record_event(&store, &rh, "run.end", "sandbox", json!({"nodes": 0})).unwrap();
    let events = events(&store, &rh.horse).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "run.begin");
    assert_eq!(events[1]["type"], "run.end");
    assert!(store.session(&rh.session).unwrap().head.is_none());
}

#[test]
fn one_lock_covers_native_ingest_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let rh = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let native = "session-test";
    let root = dir.path().join("sessions");
    let rows = super::super::live::fake::turn_rows(0, 1, "hello", "m1");
    super::super::live::fake::write_file(&root, native, &rows);
    let path = dsh::session_dir(&root, Some(super::super::live::fake::CWD), native);
    let locked = store.lock().unwrap();
    locked
        .update_session(&rh.session, |s| s.binding.native_id = Some(native.into()))
        .unwrap();
    let result = dsh::ingest_locked(&locked, &path, dsh::VERSION, Some(&rh.horse)).unwrap();
    append_event_locked(&locked, &rh, &json!({"type": "run.end"})).unwrap();
    assert_eq!(result.session, rh.session);
    assert_eq!(result.added, rows.len());
    drop(locked);
    assert_eq!(
        store
            .session_nodes(&store.session(&rh.session).unwrap())
            .unwrap()
            .len(),
        rows.len()
    );
    assert_eq!(events(&store, &rh.horse).unwrap().len(), 1);
}

#[test]
fn binding_rejects_native_ids_owned_by_other_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let first = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let second = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let locked = store.lock().unwrap();
    locked.bind_native(&first.session, "one").unwrap();
    assert!(locked.bind_native(&second.session, "one").is_err());
    assert!(locked.bind_native(&first.session, "two").is_err());
    assert!(store
        .session(&second.session)
        .unwrap()
        .binding
        .native_id
        .is_none());
}

#[test]
fn end_dsh_reconciles_bound_session_and_records_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let rh = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let root = dir.path().join("sessions");
    let rows = super::super::live::fake::turn_rows(0, 1, "hello", "m1");
    super::super::live::fake::write_file(&root, "session-test", &rows);
    store
        .lock()
        .unwrap()
        .bind_native(&rh.session, "session-test")
        .unwrap();
    record_event(&store, &rh, "run.begin", "sandbox", json!({})).unwrap();
    end_dsh(&store, &rh, "sandbox", &root, true).unwrap();
    assert_eq!(
        store
            .session_nodes(&store.session(&rh.session).unwrap())
            .unwrap()
            .len(),
        rows.len()
    );
    let events = events(&store, &rh.horse).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["nodes"], rows.len());
    assert_eq!(events[1]["custody"], "live");
}

#[test]
fn a_plain_spawn_makes_a_child_horse_and_a_hitched_one_writes_no_dna_event() {
    use crate::horse::dna;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let parent = begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    dna::hot_load(&store, &parent.horse, "[traits]\nstyle = \"short\"\n").unwrap();
    let dna_events = |horse: &str| -> Vec<String> {
        dna::history(&store, horse)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e["type"].as_str().unwrap().to_string())
            .collect()
    };
    let parent_dna = store.horse(&parent.horse).unwrap().dna;
    let child = begin(
        &store,
        dsh::HARNESS,
        dsh::VERSION,
        Some(&parent.horse),
        None,
    )
    .unwrap();
    assert_ne!(child.horse, parent.horse);
    let d = dna::get(&store, &store.horse(&child.horse).unwrap().dna).unwrap();
    assert_eq!(d.origin.parent.as_deref(), Some(parent.horse.as_str()));
    assert_eq!(d.origin.parent_dna.as_ref(), Some(&parent_dna));
    assert!(matches!(
        &d.traits["style"].from,
        dna::Provenance::Inherit { from_horse, .. } if *from_horse == parent.horse
    ));
    assert_eq!(dna_events(&child.horse), ["dna.inherit"]);
    assert_eq!(
        dna_events(&parent.horse),
        ["dna.default", "dna.edit", "dna.inherit"]
    );
    // The first entry of the child tells it the inherited trait.
    let inbox = store.inbox(&child.session).unwrap();
    assert!(inbox[0].text.contains("style = short"), "{}", inbox[0].text);
    // A hitched child stays in the parent's horse: no DNA event.
    let hitched = begin(&store, dsh::HARNESS, dsh::VERSION, None, Some(&parent)).unwrap();
    assert_eq!(hitched.horse, parent.horse);
    assert_eq!(
        dna_events(&parent.horse),
        ["dna.default", "dna.edit", "dna.inherit"]
    );
}

#[test]
fn setup_holds_dsh_and_headless_claude_and_codex_live() {
    use setup::{custody, Custody};
    let channel = claude::CHANNEL_VERSIONS[0].version;
    assert_eq!(custody(dsh::HARNESS, false, dsh::VERSION), Custody::Live);
    assert_eq!(custody(dsh::HARNESS, true, dsh::VERSION), Custody::Live);
    assert_eq!(custody(claude::HARNESS, true, channel), Custody::Live);
    assert_eq!(custody(codex::HARNESS, true, codex::VERSION), Custody::Live);
    // An interactive run keeps its terminal: a Claude Code TUI on a
    // version with a checked channel gets the channel, any other Claude
    // Code TUI and a Codex TUI the pty relay (when the profile has its
    // table, see `choose`), the rest is final-only.
    assert_eq!(custody(claude::HARNESS, false, channel), Custody::Channel);
    assert_eq!(custody(claude::HARNESS, false, "2.1.1"), Custody::Pty);
    assert_eq!(custody(codex::HARNESS, false, codex::VERSION), Custody::Pty);
    assert_eq!(custody("agy", true, "1"), Custody::FinalOnly);
    assert_eq!(Custody::Live.label(), "live");
    assert_eq!(Custody::FinalOnly.label(), "final-only");
    assert_eq!(Custody::Channel.label(), "channel");
    assert_eq!(Custody::Pty.label(), "pty");
}

#[test]
fn the_claude_command_gets_the_custody_flags_in_place_of_print() {
    use crate::horse::live::claude::Start;
    let start = Start {
        native: "01934b20-1111-7777-8888-123456789abc".into(),
        session: None,
        resume: false,
    };
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut c = v(&[
        "claude",
        "-p",
        "--dangerously-skip-permissions",
        "--model",
        "m",
    ]);
    setup::claude_command(&mut c, &start).unwrap();
    assert_eq!(
        c,
        v(&[
            "claude",
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--session-id",
            "01934b20-1111-7777-8888-123456789abc",
            "--dangerously-skip-permissions",
            "--model",
            "m"
        ])
    );
    let mut c = v(&["claude", "--dangerously-skip-permissions"]);
    let e = setup::claude_command(&mut c, &start)
        .unwrap_err()
        .to_string();
    assert!(e.contains("cannot find -p"), "{e}");
    for owned in ["--resume", "--output-format=json", "--continue"] {
        let mut c = v(&["claude", "-p", owned]);
        let e = setup::claude_command(&mut c, &start)
            .unwrap_err()
            .to_string();
        assert!(e.contains("must not set"), "{e}");
        assert_eq!(c, v(&["claude", "-p", owned]), "the command stays");
    }
}

#[test]
fn end_after_a_live_claude_run_adds_no_nodes_and_says_live() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let home = dir.path().join("home");
    let before = snapshot::Snapshot::capture(&home, claude::HARNESS).unwrap();
    let rh = begin(&store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    let native = "01934b20-1111-7777-8888-123456789abc";
    store
        .lock()
        .unwrap()
        .bind_native(&rh.session, native)
        .unwrap();
    // The custody ingested the transcript after the turn.
    let file = claude::transcript_path(&home.join(".claude"), "/home/demo/project", native);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let rows = [
        crate::horse::live::claude::fake::chain(
            "user",
            "u1",
            None,
            &crate::horse::live::claude::fake::user_message("hi"),
            native,
        ),
        crate::horse::live::claude::fake::chain(
            "assistant",
            "a1",
            Some("u1"),
            &crate::horse::live::claude::fake::assistant_message(1, "hello"),
            native,
        ),
    ];
    std::fs::write(&file, [rows.join(&b"\n"[..]), b"\n".to_vec()].concat()).unwrap();
    claude::ingest_run_locked(&store.lock().unwrap(), &file, claude::VERSION, &rh.horse).unwrap();
    let head = store.session(&rh.session).unwrap().head;
    record_event(&store, &rh, "run.begin", "sandbox", json!({})).unwrap();
    final_only::end(
        &store,
        &rh,
        "sandbox",
        &home,
        &before,
        true,
        Some((
            setup::Custody::Live,
            json!({"turns": 1, "deliveries": 1, "landed": 1}),
        )),
    )
    .unwrap();
    assert_eq!(store.session(&rh.session).unwrap().head, head);
    let e = events(&store, &rh.horse).unwrap();
    let end = e.last().unwrap();
    assert_eq!(end["type"], "run.end");
    assert_eq!(end["custody"], "live");
    assert_eq!(end["added"], 0);
    assert_eq!(end["nodes"], 2);
    assert_eq!(end["turns"], 1);
    assert_eq!(end["landed"], 1);
}

#[test]
fn the_live_command_replaces_only_the_one_shot_arguments() {
    use super::setup::live_command;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut c = s(&[
        "codex",
        "-c",
        "a=1",
        "exec",
        "--yolo",
        "-",
        "-c",
        "model=\"m\"",
    ]);
    live_command(
        &mut c,
        &s(&["exec", "--yolo", "-"]),
        &s(&["app-server", "--listen", "stdio://"]),
    )
    .unwrap();
    assert_eq!(
        c,
        s(&[
            "codex",
            "-c",
            "a=1",
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "model=\"m\""
        ])
    );
    // A command without them fails closed, and stays as it was.
    let mut c = s(&["codex", "resume"]);
    let e = live_command(&mut c, &s(&["exec", "-"]), &s(&["app-server"])).unwrap_err();
    assert!(e.to_string().contains("one-shot arguments"), "{e:#}");
    assert_eq!(c, s(&["codex", "resume"]));
    assert!(live_command(&mut c, &[], &s(&["app-server"])).is_err());
}

#[test]
fn the_run_end_report_names_the_counts_of_each_custody() {
    use crate::horse::live::{self, Done};
    let dsh = setup::report(&Done::Dsh(live::Report {
        events: 4,
        delivered: 2,
        reconciled: 1,
        sessions: vec!["s".into()],
    }));
    assert_eq!(dsh, json!({"events": 4, "deliveries": 2, "reconciled": 1}));
    let claude = setup::report(&Done::Claude(live::claude::Report {
        turns: 3,
        deliveries: 3,
        landed: 3,
        ..Default::default()
    }));
    assert_eq!(claude["turns"], 3);
    assert_eq!(claude["landed"], 3);
    assert_eq!(claude["bad"], 0);
    let codex = setup::report(&Done::Codex(live::codex::Report {
        turns: 1,
        deliveries: 2,
        malformed: 5,
        exited: true,
        ..Default::default()
    }));
    assert_eq!(codex["deliveries"], 2);
    assert_eq!(codex["bad"], 5, "a line that is not JSON-RPC counts as bad");
    assert_eq!(codex["exited"], true);
}

#[test]
fn the_run_end_report_of_a_pty_custody() {
    use crate::horse::live::{pty, Done};
    let r = setup::report(&Done::Pty(pty::Report {
        thread: Some("t-1".into()),
        stops: 2,
        typed: 1,
        landed: 1,
        not_landed: vec!["e2".into()],
        ..Default::default()
    }));
    assert_eq!(r["thread"], "t-1");
    assert_eq!(r["stops"], 2);
    assert_eq!(r["deliveries"], 1);
    assert_eq!(r["landed"], 1);
    assert_eq!(r["not_landed"], json!(["e2"]));
    assert_eq!(r["other_stops"], 0);
}
