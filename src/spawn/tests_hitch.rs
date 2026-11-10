//! Tests for `--hitch`: a child that joins the horse of its parent.
use super::tests::{broker, ceiling, cfg, child_that_runs, req, setup, start, Recorder, Setup};
use super::*;
use crate::broker::{Answer, Broker, Status as RStatus};
use crate::horse::hitch::{edges, wire, EdgeKind};
use crate::horse::{claude, device::Access, run, run::RunHorse, Store};

const RESULT: &str = "{\"exit_code\":0,\"branch\":\"b\",\"base\":\"c\",\"commits\":2,\"uncommitted\":0,\"stat\":\"\",\"output\":\"all done\",\"output_truncated\":false,\"started\":1,\"ended\":2}";

fn child_that_writes(secs: &str) -> Vec<OsString> {
    let script =
        format!("sleep {secs}; printf '%s' '{RESULT}' > \"$(dirname \"$1\")/result.json\"");
    ["sh", "-c", &script, "fake-child"]
        .iter()
        .map(OsString::from)
        .collect()
}

/// A parent run in horse mode: its store and its run.
fn parent_run(s: &Setup) -> (Store, RunHorse) {
    let store = Store::open_host(&s.env).unwrap();
    let parent = run::begin(&store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    (store, parent)
}

fn horse_broker(s: &Setup, parent: &RunHorse, store: &Store, program: &[&str]) -> Broker {
    let b = broker(
        s,
        "p-1",
        vec![],
        cfg(&["codex"], 2),
        program,
        Recorder::new(Answer::Once),
    );
    b.horse
        .set(Access {
            store: store.clone(),
            horse: parent.horse.clone(),
            session: parent.session.clone(),
        })
        .ok()
        .unwrap();
    b
}

fn start_hitched(harness: &str) -> crate::broker::Request {
    req(serde_json::json!({
        "action": "start", "harness": harness, "prompt_file": "task.md", "hitch": true
    }))
}

fn kinds(store: &Store, horse: &str) -> Vec<EdgeKind> {
    edges(store, horse)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

fn child_session(store: &Store, parent: &RunHorse) -> String {
    edges(store, &parent.horse)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == EdgeKind::SpawnedFrom && e.to == parent.session)
        .map(|e| e.from)
        .expect("no SpawnedFrom edge")
}

#[test]
fn a_hitched_child_joins_the_parents_horse_and_its_end_reaches_the_parent_once() {
    let s = setup();
    let (store, parent) = parent_run(&s);
    let program = child_that_writes("0");
    let program: Vec<&str> = program.iter().map(|a| a.to_str().unwrap()).collect();
    let b = horse_broker(&s, &parent, &store, &program);
    let resp = b.handle(&start_hitched("codex"));
    assert_eq!(resp.status, RStatus::Done, "{}", resp.message);
    assert!(resp.result.get("hitch").is_none(), "{}", resp.result);
    let id = resp.result["id"].as_str().unwrap().to_string();
    let sp = b.spawner.clone().unwrap();
    let done = sp.wait(&id, Duration::from_secs(10)).unwrap();
    assert_eq!(done.state, State::Exited, "{}", done.message);

    // Behavior 1: the parent's horse, a new session, a SpawnedFrom edge.
    let sessions = store.horse(&parent.horse).unwrap().sessions;
    assert_eq!(sessions.len(), 2);
    let child = child_session(&store, &parent);
    assert_ne!(child, parent.session);
    assert!(sessions.contains(&child));
    assert_eq!(store.session(&child).unwrap().binding.harness, "codex");
    // Behavior 2: the end queues one append and records one Append edge.
    assert_eq!(
        kinds(&store, &parent.horse),
        vec![EdgeKind::SpawnedFrom, EdgeKind::Append]
    );
    // The first entry is the DNA at the start of the session.
    let inbox = store.inbox(&parent.session).unwrap()[1..].to_vec();
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        inbox[0].text,
        format!(
            "hitched {} (codex) finished\ndone: codex exited with code 0; 2 commits on paddock/spawn-codex-{id}\nall done",
            &child[..8]
        )
    );
    assert_eq!(inbox[0].role, "note");
    // The job file carried the link on the host; the parent's status does not.
    assert!(!serde_json::to_string(&done).unwrap().contains(&child));

    // Behavior 5: plain spawn is unchanged, it touches no horse.
    let resp = b.handle(&start("codex"));
    assert_eq!(resp.status, RStatus::Done, "{}", resp.message);
    let id = resp.result["id"].as_str().unwrap().to_string();
    let done = sp.wait(&id, Duration::from_secs(10)).unwrap();
    assert_eq!(done.state, State::Exited, "{}", done.message);
    assert_eq!(store.horse(&parent.horse).unwrap().sessions.len(), 2);
    assert_eq!(store.inbox(&parent.session).unwrap().len(), 2);
    assert_eq!(kinds(&store, &parent.horse).len(), 2);
}

#[test]
fn done_then_end_queues_the_result_once() {
    let s = setup();
    let (store, parent) = parent_run(&s);
    let program = child_that_writes("2");
    let program: Vec<&str> = program.iter().map(|a| a.to_str().unwrap()).collect();
    let b = horse_broker(&s, &parent, &store, &program);
    let resp = b.handle(&start_hitched("codex"));
    assert_eq!(resp.status, RStatus::Done, "{}", resp.message);
    let id = resp.result["id"].as_str().unwrap().to_string();
    // The child's `paddock-host done` comes first, through its own broker.
    let child = child_session(&store, &parent);
    let child_access = Access {
        store: store.clone(),
        horse: parent.horse.clone(),
        session: child.clone(),
    };
    assert!(wire::report_done(&child_access, "ok", "hello"));
    let sp = b.spawner.clone().unwrap();
    let done = sp.wait(&id, Duration::from_secs(15)).unwrap();
    assert_eq!(done.state, State::Exited, "{}", done.message);
    // The first entry is the DNA at the start of the session.
    let inbox = store.inbox(&parent.session).unwrap()[1..].to_vec();
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        inbox[0].text,
        format!("hitched {} (codex) finished\nhello", &child[..8])
    );
    assert_eq!(
        kinds(&store, &parent.horse),
        vec![EdgeKind::SpawnedFrom, EdgeKind::Append]
    );
}

#[test]
fn a_timeout_without_done_queues_the_result_once() {
    let s = setup();
    let (store, parent) = parent_run(&s);
    let mut c = cfg(&["codex"], 2);
    c.on_timeout = OnTimeout::Stop;
    let sp = Arc::new(Spawner::with_program(
        &s.env,
        c,
        ceiling("p-1", 0, &s.project),
        &s.project,
        child_that_runs("30"),
    ));
    let o = StartOpts {
        limits: Limits {
            timeout: 1,
            ..Default::default()
        },
        hitch: Some(parent.clone()),
        ..Default::default()
    };
    let st = sp.start("codex", b"task", o).unwrap();
    let done = sp.wait(&st.id, Duration::from_secs(40)).unwrap();
    assert_eq!(done.state, State::TimedOut, "{done:?}");
    let child = child_session(&store, &parent);
    // The first entry is the DNA at the start of the session.
    let inbox = store.inbox(&parent.session).unwrap()[1..].to_vec();
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        inbox[0].text,
        format!(
            "hitched {} (codex) finished\ntimed_out: stopped by paddock after the time limit of 1 seconds",
            &child[..8]
        )
    );
    // A second end of the same child changes nothing.
    assert!(wire::finish_once(
        &store,
        &wire::link_of(&Access {
            store: store.clone(),
            horse: parent.horse.clone(),
            session: child,
        })
        .unwrap()
        .unwrap(),
        "again"
    )
    .unwrap()
    .is_none());
    // The DNA entry at the start and the one result.
    assert_eq!(store.inbox(&parent.session).unwrap().len(), 2);
    assert_eq!(
        kinds(&store, &parent.horse),
        vec![EdgeKind::SpawnedFrom, EdgeKind::Append]
    );
}

#[test]
fn hitch_without_horse_mode_is_refused_and_plain_spawn_still_works() {
    let s = setup();
    let program = child_that_writes("0");
    let program: Vec<&str> = program.iter().map(|a| a.to_str().unwrap()).collect();
    let b = broker(
        &s,
        "p-1",
        vec![],
        cfg(&["codex"], 2),
        &program,
        Recorder::new(Answer::Once),
    );
    let resp = b.handle(&start_hitched("codex"));
    assert_eq!(resp.status, RStatus::Invalid, "{}", resp.message);
    assert!(
        resp.message.contains("--hitch needs horse mode"),
        "{}",
        resp.message
    );
    let sp = b.spawner.clone().unwrap();
    assert!(sp.list().is_empty());
    let resp = b.handle(&start("codex"));
    assert_eq!(resp.status, RStatus::Done, "{}", resp.message);
    let id = resp.result["id"].as_str().unwrap().to_string();
    let done = sp.wait(&id, Duration::from_secs(10)).unwrap();
    assert_eq!(done.state, State::Exited, "{}", done.message);
    assert!(!crate::horse::root(&s.env).join("lock").exists());
}

/// Two hitched children of a parent held live.
mod live;

#[test]
fn a_hitched_child_that_does_not_start_still_reports_to_the_parent() {
    let s = setup();
    let (store, parent) = parent_run(&s);
    let b = horse_broker(&s, &parent, &store, &["/home/demo/no-such-program"]);
    let resp = b.handle(&start_hitched("codex"));
    assert_ne!(resp.status, RStatus::Done, "{}", resp.message);
    let child = child_session(&store, &parent);
    assert_eq!(
        kinds(&store, &parent.horse),
        vec![EdgeKind::SpawnedFrom, EdgeKind::Append]
    );
    let inbox = store.inbox(&parent.session).unwrap();
    let head = format!("hitched {} (codex) finished\nfailed: ", &child[..8]);
    assert!(inbox.iter().any(|e| e.text.starts_with(&head)), "{inbox:?}");
}
