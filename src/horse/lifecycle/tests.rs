use super::*;
use crate::{
    build::{build_with, Options},
    profile::{Harness, Profile},
    spec::Network,
};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path};

/// The run events of a horse: the DNA events are left out.
pub(super) fn run_events(store: &Store, horse: &str) -> anyhow::Result<Vec<serde_json::Value>> {
    Ok(run::events(store, horse)?
        .into_iter()
        .filter(|e| !e["type"].as_str().unwrap_or("").starts_with("dna."))
        .collect())
}

pub(super) fn setup(dir: &Path, harness: &str, version: &str) -> (HostEnv, Built) {
    let env = HostEnv::from_parts(dir.join("host"), "test".into(), 1000, 1000, BTreeMap::new());
    std::fs::create_dir_all(&env.home).unwrap();
    let exe = dir.join("fake-version");
    std::fs::write(&exe, format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n")).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let profile: Profile = toml::from_str(&format!(
        "name = \"{harness}\"\n[launch]\nargs = [\"web\"]\n"
    ))
    .unwrap();
    let h = Harness {
        profile,
        exe,
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let project = dir.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let opts = Options {
        harness: harness.into(),
        project: Some(project),
        network: Some(Network::None),
        inherit_env: Some(false),
        assume_yes: true,
        ..Default::default()
    };
    let mut built = build_with(&env, &opts, h).unwrap();
    built.spec.command = vec!["fake".into(), "web".into()];
    (env, built)
}

#[test]
fn disabled_never_opens_store_and_unsupported_fails_closed() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "agy", "1");
    assert!(start(&env, &mut b).unwrap().is_none());
    assert!(!super::super::root(&env).exists());
    b.horse = true;
    assert!(start(&env, &mut b)
        .err()
        .unwrap()
        .to_string()
        .contains("no pinned horse codec for agy"));
    assert!(!super::super::root(&env).exists());
}

#[test]
fn claude_attributes_multiple_transcripts() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "claude", claude::VERSION);
    b.horse = true;
    let lifecycle = start(&env, &mut b).unwrap().unwrap();
    let dir = b.spec.home_src.join(".claude/projects/project");
    std::fs::create_dir_all(&dir).unwrap();
    for id in [
        "01934b20-1111-7777-8888-123456789abc",
        "01934b20-1111-7777-8888-123456789abd",
    ] {
        std::fs::write(dir.join(format!("{id}.jsonl")), format!("{{\"type\":\"user\",\"uuid\":\"{id}\",\"sessionId\":\"{id}\",\"version\":\"{}\",\"message\":{{\"role\":\"user\",\"content\":\"hello\"}}}}\n", claude::VERSION)).unwrap();
    }
    let horse = lifecycle.run.horse.clone();
    drop(lifecycle);
    let store = Store::open(&super::super::root(&env)).unwrap();
    assert_eq!(store.horses().unwrap().len(), 1);
    assert_eq!(store.horse(&horse).unwrap().sessions.len(), 2);
    let events = run_events(&store, &horse).unwrap();
    assert_eq!(events[0]["type"], "run.begin");
    assert_eq!(events[1]["type"], "run.end");
    assert_eq!(events[1]["nodes"], 2);
}

#[test]
fn dsh_final_ingest_uses_the_live_horse() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "dsh", dsh::VERSION);
    b.horse = true;
    let lifecycle = start(&env, &mut b).unwrap().unwrap();
    let store = Store::open(&super::super::root(&env)).unwrap();
    let native = store
        .session(&lifecycle.run.session)
        .unwrap()
        .binding
        .native_id
        .unwrap();
    let rows = super::super::live::fake::turn_rows(0, 1, "hello", "m1");
    super::super::live::fake::write_file(&b.spec.home_src.join(".dsh/sessions"), &native, &rows);
    let horse = lifecycle.run.horse.clone();
    drop(lifecycle);
    assert_eq!(store.horses().unwrap().len(), 1);
    assert_eq!(store.horse(&horse).unwrap().sessions.len(), 1);
    let events = run_events(&store, &horse).unwrap();
    assert_eq!(events[0]["type"], "run.begin");
    assert_eq!(events[1]["type"], "run.end");
    assert_eq!(events[1]["nodes"], rows.len());
}

#[test]
fn resumed_transcript_belongs_to_each_runs_own_horse() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "claude", claude::VERSION);
    b.horse = true;
    let id = "01934b20-1111-7777-8888-123456789abc";
    let dir = b.spec.home_src.join(".claude/projects/project");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    let row = |uuid: &str| {
        format!("{{\"type\":\"user\",\"uuid\":\"{uuid}\",\"sessionId\":\"{id}\",\"version\":\"{}\",\"message\":{{\"role\":\"user\",\"content\":\"hello\"}}}}\n", claude::VERSION)
    };
    let first = start(&env, &mut b).unwrap().unwrap();
    std::fs::write(&path, row("one")).unwrap();
    let horse1 = first.run.horse.clone();
    drop(first);
    let second = start(&env, &mut b).unwrap().unwrap();
    std::fs::write(&path, format!("{}{}", row("one"), row("two"))).unwrap();
    let horse2 = second.run.horse.clone();
    drop(second);
    let store = Store::open(&super::super::root(&env)).unwrap();
    for (horse, count) in [(horse1, 1), (horse2, 2)] {
        let sessions = store.horse(&horse).unwrap().sessions;
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            store
                .session_nodes(&store.session(&sessions[0]).unwrap())
                .unwrap()
                .len(),
            count
        );
        assert_eq!(run_events(&store, &horse).unwrap()[1]["nodes"], count);
    }
}

#[test]
fn codex_attributes_new_transcript() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "codex", codex::VERSION);
    b.horse = true;
    let lifecycle = start(&env, &mut b).unwrap().unwrap();
    let dir = b.spec.home_src.join(".codex/sessions/2026/11/04");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "01934b20-1111-7777-8888-123456789abc";
    std::fs::write(dir.join(format!("rollout-2026-11-04T00-00-00-{id}.jsonl")), format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cli_version\":\"{}\"}}}}\n", codex::VERSION)).unwrap();
    let horse = lifecycle.run.horse.clone();
    drop(lifecycle);
    let store = Store::open(&super::super::root(&env)).unwrap();
    let sessions = store.horse(&horse).unwrap().sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        store
            .session(&sessions[0])
            .unwrap()
            .binding
            .native_id
            .as_deref(),
        Some(id)
    );
    assert_eq!(run_events(&store, &horse).unwrap()[1]["nodes"], 1);
}

#[test]
fn spawned_run_gets_its_own_horse_and_device_grant() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut parent) = setup(t.path(), "claude", claude::VERSION);
    parent.horse = true;
    let parent_lifecycle = start(&env, &mut parent).unwrap().unwrap();
    let ceiling = crate::spawn::Ceiling::of(&parent, "test", &BTreeMap::new());
    let (_, mut child) = setup(t.path(), "claude", claude::VERSION);
    child.horse = ceiling.horse;
    child.spec.id = "child".into();
    let child_lifecycle = start(&env, &mut child).unwrap().unwrap();
    let parent_access = parent_lifecycle.access().unwrap();
    let child_access = child_lifecycle.access().unwrap();
    assert_ne!(parent_access.horse, child_access.horse);
    let parent_session = &parent_lifecycle.run.session;
    let child_session = &child_lifecycle.run.session;
    for (access, own, other) in [
        (&parent_access, parent_session, child_session),
        (&child_access, child_session, parent_session),
    ] {
        let tree = access.tree().unwrap();
        assert!(tree.paths.contains_key(&format!("self/sessions/{own}")));
        assert!(!tree.paths.contains_key(&format!("self/sessions/{other}")));
    }
    drop(child_lifecycle);
    drop(parent_lifecycle);
    for access in [parent_access, child_access] {
        let events = run_events(&access.store, &access.horse).unwrap();
        assert_eq!(events[0]["type"], "run.begin");
        assert_eq!(events[1]["type"], "run.end");
    }
}

#[test]
fn a_hitched_child_runs_in_the_session_its_parent_made() {
    use crate::horse::hitch::wire;
    let t = tempfile::tempdir().unwrap();
    let (env, mut parent) = setup(t.path(), "claude", claude::VERSION);
    parent.horse = true;
    let parent_lifecycle = start(&env, &mut parent).unwrap().unwrap();
    let store = Store::open(&super::super::root(&env)).unwrap();
    let hitched = wire::begin(&store, &parent_lifecycle.run, "claude").unwrap();
    let (_, mut child) = setup(t.path(), "claude", claude::VERSION);
    child.horse = true;
    child.spec.id = "child".into();
    child.spawn = Some(crate::spawn::ChildInfo {
        spawn_id: "s1".into(),
        parent: parent.spec.id.clone(),
        chain: vec![parent.spec.id.clone()],
        depth: 1,
        modes: BTreeMap::new(),
        role: Default::default(),
        hitch: Some(hitched.clone()),
    });
    let child_lifecycle = start(&env, &mut child).unwrap().unwrap();
    assert_eq!(child_lifecycle.run, hitched.run());
    let child_access = child_lifecycle.access().unwrap();
    assert_eq!(child_access.horse, parent_lifecycle.run.horse);
    assert_eq!(store.horse(&child_access.horse).unwrap().sessions.len(), 2);
    assert_eq!(wire::link_of(&child_access).unwrap(), Some(hitched.clone()));
    let tree = child_access.tree().unwrap();
    assert!(tree
        .paths
        .contains_key(&format!("self/sessions/{}", hitched.session)));
    assert!(!tree
        .paths
        .contains_key(&format!("self/sessions/{}", parent_lifecycle.run.session)));
    drop(child_lifecycle);
    drop(parent_lifecycle);
    let kinds: Vec<String> = run_events(&store, &child_access.horse)
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        [
            "run.begin",
            "edge.spawned_from",
            "run.begin",
            "run.end",
            "run.end"
        ]
    );
    // A session of another horse is refused.
    let (_, mut other) = setup(t.path(), "claude", claude::VERSION);
    other.horse = true;
    other.spawn = child.spawn.clone();
    if let Some(c) = other.spawn.as_mut() {
        c.hitch.as_mut().unwrap().horse = "nope".into();
    }
    assert!(start(&env, &mut other).is_err());
}

#[test]
fn a_plain_spawn_from_a_horse_run_inherits_the_parent_dna() {
    use crate::horse::dna;
    let t = tempfile::tempdir().unwrap();
    let (env, mut parent) = setup(t.path(), "claude", claude::VERSION);
    parent.horse = true;
    let parent_lifecycle = start(&env, &mut parent).unwrap().unwrap();
    let store = Store::open(&super::super::root(&env)).unwrap();
    let parent_horse = parent_lifecycle.run.horse.clone();
    dna::hot_load(&store, &parent_horse, "[traits]\nstyle = \"short\"\n").unwrap();
    let (_, mut child) = setup(t.path(), "claude", claude::VERSION);
    child.horse = true;
    child.spec.id = "child".into();
    child.spawn = Some(crate::spawn::ChildInfo {
        spawn_id: "s1".into(),
        parent: parent.spec.id.clone(),
        chain: vec![parent.spec.id.clone()],
        depth: 1,
        modes: BTreeMap::new(),
        role: Default::default(),
        hitch: None,
    });
    let child_lifecycle = start(&env, &mut child).unwrap().unwrap();
    let child_horse = child_lifecycle.run.horse.clone();
    assert_ne!(child_horse, parent_horse);
    let d = dna::get(&store, &store.horse(&child_horse).unwrap().dna).unwrap();
    assert_eq!(d.origin.parent.as_deref(), Some(parent_horse.as_str()));
    assert_eq!(d.traits["style"].value, "short");
    // The device of the child shows the inherited DNA.
    let tree = child_lifecycle.access().unwrap().tree().unwrap();
    match &tree.paths[crate::horse::device::DNA_PATH] {
        crate::horse::device::Entry::File(b) => {
            assert!(String::from_utf8_lossy(b).contains("short"))
        }
        e => panic!("{e:?}"),
    }
    drop(child_lifecycle);
    drop(parent_lifecycle);
}

#[test]
fn a_headless_codex_run_gets_the_app_server_and_its_task_in_the_inbox() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "codex", codex::VERSION);
    b.horse = true;
    b.profile.oneshot = Some(
        toml::from_str(
            "args = [\"exec\", \"--dangerously-bypass-approvals-and-sandbox\", \"-\"]\nprompt = \"stdin\"\n",
        )
        .unwrap(),
    );
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    b.spec.command = s(&[
        "codex",
        "exec",
        "--dangerously-bypass-approvals-and-sandbox",
        "-",
        "-c",
        "model=\"m\"",
    ]);
    super::live_tests::headless(&mut b, "write the note\n");
    let lifecycle = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(lifecycle.custody, run::setup::Custody::Live);
    assert_eq!(
        b.spec.command,
        s(&[
            "codex",
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "sandbox_mode=\"danger-full-access\"",
            "-c",
            "model=\"m\"",
        ])
    );
    // The task goes to the custody, not to the stdin of the sandbox.
    assert_eq!(b.stdin, None);
    let store = Store::open(&super::super::root(&env)).unwrap();
    let inbox = store.inbox(&lifecycle.run.session).unwrap();
    assert_eq!(inbox.len(), 2, "{inbox:?}");
    assert_eq!(inbox[0].role, "system");
    assert_eq!(
        (inbox[1].role.as_str(), inbox[1].text.as_str()),
        ("user", "write the note")
    );
    let horse = lifecycle.run.horse.clone();
    drop(lifecycle);
    let events = run_events(&store, &horse).unwrap();
    assert_eq!(events[1]["type"], "run.end");
    assert_eq!(events[1]["custody"], "live");
}

#[test]
fn an_interactive_codex_run_keeps_its_command_and_final_only_custody() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = setup(t.path(), "codex", codex::VERSION);
    b.horse = true;
    let before = b.spec.command.clone();
    let lifecycle = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(lifecycle.custody, run::setup::Custody::FinalOnly);
    assert_eq!(b.spec.command, before);
    let store = Store::open(&super::super::root(&env)).unwrap();
    let horse = lifecycle.run.horse.clone();
    drop(lifecycle);
    assert_eq!(
        run_events(&store, &horse).unwrap()[1]["custody"],
        "final-only"
    );
}
