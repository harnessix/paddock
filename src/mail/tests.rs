use super::*;
use std::io::Write;
use std::time::{Duration, Instant};

fn env() -> (tempfile::TempDir, HostEnv) {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    (t, env)
}

const RUN: &str = "fake-proj-1a2b3c";

fn made(env: &HostEnv) {
    create(env, RUN, &Starter::host()).unwrap();
}

#[test]
fn one_sequence_counts_both_directions() {
    let (_t, env) = env();
    made(&env);
    let a = append(&env, RUN, Dir::Down, Message::new(HOST, Kind::Say, "one")).unwrap();
    let b = append(
        &env,
        RUN,
        Dir::Up,
        Message::new(FROM_AGENT, Kind::Say, "two"),
    )
    .unwrap();
    let c = append(&env, RUN, Dir::Down, Message::new(HOST, Kind::Ask, "three")).unwrap();
    assert_eq!((a.seq, b.seq, c.seq), (1, 2, 3));
    assert_eq!(read(&env, RUN, Dir::Down).len(), 2);
    assert_eq!(find(&env, RUN, 2).unwrap().0, Dir::Up);
    assert_eq!(last_seq(&env, RUN), 3);
    // Without the seq file, the logs give the last one.
    std::fs::remove_file(dir(&env, RUN).unwrap().join("seq")).unwrap();
    assert_eq!(last_seq(&env, RUN), 3);
}

#[test]
fn a_cursor_survives_and_never_moves_back() {
    let (_t, env) = env();
    made(&env);
    for t in ["a", "b", "c"] {
        append(&env, RUN, Dir::Down, Message::new(HOST, Kind::Say, t)).unwrap();
    }
    assert_eq!(unread_down(&env, RUN).len(), 3);
    set_cursor(&env, RUN, Dir::Down, AGENT, 2).unwrap();
    set_cursor(&env, RUN, Dir::Down, AGENT, 1).unwrap();
    assert_eq!(cursor(&env, RUN, Dir::Down, AGENT), 2);
    let left = unread_down(&env, RUN);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].text, "c");
    assert!(set_cursor(&env, RUN, Dir::Down, "../x", 1).is_err());
}

#[test]
fn a_half_written_line_waits_and_the_next_write_starts_clean() {
    let (_t, env) = env();
    made(&env);
    append(
        &env,
        RUN,
        Dir::Up,
        Message::new(FROM_AGENT, Kind::Say, "whole"),
    )
    .unwrap();
    let f = dir(&env, RUN).unwrap().join("up.jsonl");
    let mut log = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
    log.write_all(b"{\"seq\":99,\"ts\":1,\"fr").unwrap();
    assert_eq!(read(&env, RUN, Dir::Up).len(), 1);
    let m = append(
        &env,
        RUN,
        Dir::Up,
        Message::new(FROM_AGENT, Kind::Say, "next"),
    )
    .unwrap();
    let all = read(&env, RUN, Dir::Up);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1].seq, m.seq);
}

#[test]
fn a_long_message_is_refused_with_the_loft_route() {
    let (_t, env) = env();
    made(&env);
    let big = "x".repeat(65 * 1024);
    let e = append(&env, RUN, Dir::Up, Message::new(FROM_AGENT, Kind::Say, big)).unwrap_err();
    let text = e.to_string();
    assert!(matches!(e, SendError::TooLarge(66560)), "{text}");
    assert!(
        text.contains("loft") && text.contains("--ref .paddock/"),
        "{text}"
    );
    // 64 KiB exactly is fine.
    let ok = "y".repeat(MAX_TEXT);
    append(&env, RUN, Dir::Up, Message::new(FROM_AGENT, Kind::Say, ok)).unwrap();
}

#[test]
fn refs_stay_in_the_project() {
    for bad in ["/etc/passwd", "../x", "a/../../b", ""] {
        assert!(check_ref(bad).is_err(), "{bad}");
    }
    assert!(check_ref(".paddock/mail/report.md").is_ok());
}

#[test]
fn the_rate_limit_stops_a_flood_but_not_events() {
    let (_t, env) = env();
    made(&env);
    for i in 0..RATE_PER_MINUTE {
        append(
            &env,
            RUN,
            Dir::Up,
            Message::new(FROM_AGENT, Kind::Say, format!("{i}")),
        )
        .unwrap();
    }
    let e = append(
        &env,
        RUN,
        Dir::Up,
        Message::new(FROM_AGENT, Kind::Say, "more"),
    );
    assert!(matches!(e, Err(SendError::Rate)), "{e:?}");
    append(&env, RUN, Dir::Up, Message::event("idle", "x")).unwrap();
    // The other direction has its own count.
    append(&env, RUN, Dir::Down, Message::new(HOST, Kind::Say, "hi")).unwrap();
}

#[test]
fn one_end_event_only() {
    let (_t, env) = env();
    made(&env);
    assert!(events::ended(&env, RUN, Some(0)).is_some());
    assert!(events::ended(&env, RUN, Some(1)).is_none());
    assert!(append_end(&env, RUN, Message::event("gone", "x")).is_none());
    let ends: Vec<_> = read(&env, RUN, Dir::Up)
        .into_iter()
        .filter(|m| m.is_end())
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].code, Some(0));
    assert_eq!(end(&env, RUN).unwrap().seq, ends[0].seq);
    // Nobody reads the box of an ended run.
    let e = send_down(&env, RUN, Message::new(HOST, Kind::Say, "late"));
    assert!(matches!(e, Err(SendError::Ended)), "{e:?}");
}

#[test]
fn recovery_does_not_overtake_a_live_end_writer_holding_the_mailbox_lock() {
    let (_t, env) = env();
    made(&env);
    let pid = std::process::id();
    set_run_info(
        &env,
        &RunInfo {
            id: RUN.into(),
            harness: "fake".into(),
            project: env.home.join("project"),
            pid,
            pid_start: events::proc_start(pid),
            started: 1,
            detached: false,
            delivery: None,
            horse_session: None,
        },
    )
    .unwrap();
    let d = dir(&env, RUN).unwrap();
    let guard = lock(&d).unwrap();
    let mut pending = Message::event("ended", "complete");
    pending.code = Some(0);
    crate::privfs::write_atomic(&d.join("end"), serde_json::to_vec(&pending).unwrap()).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let reader_env = env.clone();
    let reader = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx.send(inspect_end(&reader_env, RUN)).unwrap();
    });
    started_rx.recv().unwrap();
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        EndState::Pending
    );
    let written = append_locked(&env, RUN, &d, Dir::Up, pending).unwrap();
    crate::privfs::write_atomic(&d.join("end"), serde_json::to_vec(&written).unwrap()).unwrap();
    drop(guard);
    reader.join().unwrap();
    assert_eq!(
        inspect_end(&env, RUN),
        EndState::Complete(Box::new(written))
    );
    assert_eq!(
        read(&env, RUN, Dir::Up)
            .iter()
            .filter(|m| m.is_end())
            .count(),
        1
    );
}

#[test]
fn a_dead_process_is_gone_once_and_a_live_one_is_not() {
    let (_t, env) = env();
    made(&env);
    let me = std::process::id();
    let info = RunInfo {
        id: RUN.into(),
        harness: "fake".into(),
        project: "/p".into(),
        pid: me,
        pid_start: events::proc_start(me),
        started: 1,
        detached: false,
        horse_session: None,
        delivery: None,
    };
    set_run_info(&env, &info).unwrap();
    assert!(events::check_gone(&env, RUN).is_none());
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    set_run_info(
        &env,
        &RunInfo {
            pid: dead,
            pid_start: None,
            ..info
        },
    )
    .unwrap();
    let g = events::check_gone(&env, RUN).expect("gone");
    assert_eq!(g.event.as_deref(), Some("gone"));
    assert!(events::check_gone(&env, RUN).is_none());
    assert!(events::ended(&env, RUN, Some(0)).is_none());
}

#[test]
fn a_reused_pid_is_not_alive() {
    let me = std::process::id();
    let start = events::proc_start(me);
    assert!(events::alive(me, start));
    if let Some(s) = start {
        assert!(!events::alive(me, Some(s + 1)));
    }
}

#[test]
fn a_child_tells_its_parent_that_it_started_and_ended() {
    let (_t, env) = env();
    let parent = "claude-proj-0000aa";
    create(&env, parent, &Starter::host()).unwrap();
    let s = events::Start {
        id: RUN,
        harness: "fake",
        project: Path::new("/p"),
        detached: false,
        delivery: None,
        parent: Some((parent, "s1")),
    };
    let run = events::start(&env, &s).unwrap();
    assert_eq!(starter(&env, RUN).unwrap().parent.as_deref(), Some(parent));
    run.finish(Some(3));
    let up = read(&env, parent, Dir::Up);
    let names: Vec<_> = up.iter().map(|m| m.event.clone().unwrap()).collect();
    assert_eq!(names, ["child-started", "child-ended"]);
    assert_eq!(up[1].code, Some(3));
    assert_eq!(up[1].child.as_deref(), Some(RUN));
    let own = read(&env, RUN, Dir::Up);
    assert_eq!(own.first().unwrap().event.as_deref(), Some("started"));
    assert_eq!(own.last().unwrap().event.as_deref(), Some("ended"));
}

#[test]
fn idle_with_unread_mail_is_one_event_per_new_mail() {
    let (_t, env) = env();
    made(&env);
    events::idle(&env, RUN, 600);
    assert!(read(&env, RUN, Dir::Up).is_empty(), "no mail, no event");
    append(
        &env,
        RUN,
        Dir::Down,
        Message::new(HOST, Kind::Say, "hello?"),
    )
    .unwrap();
    events::idle(&env, RUN, 30);
    assert!(read(&env, RUN, Dir::Up).is_empty(), "not quiet long enough");
    events::idle(&env, RUN, 180);
    events::idle(&env, RUN, 200);
    let up = read(&env, RUN, Dir::Up);
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(
        up[0]
            .text
            .contains("3 minutes with 1 unread message(s) (#1)"),
        "{}",
        up[0].text
    );
    append(&env, RUN, Dir::Down, Message::new(HOST, Kind::Say, "again")).unwrap();
    events::idle(&env, RUN, 260);
    assert_eq!(read(&env, RUN, Dir::Up).len(), 2);
}

#[test]
fn idle_after_comes_from_the_mail_table() {
    assert_eq!(
        events::Cfg::parse("").unwrap().idle_after,
        events::IDLE_AFTER
    );
    assert_eq!(
        events::Cfg::parse("[mail]\nidle_after = \"5m\"\n")
            .unwrap()
            .idle_after,
        300
    );
    assert!(events::Cfg::parse("[mail]\nidle_after = 5\n").is_err());
}

#[test]
fn the_agent_reads_down_and_writes_only_its_own_up_box() {
    let (_t, env) = env();
    made(&env);
    send_down(&env, RUN, Message::new(HOST, Kind::Ask, "branch?")).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    let got = serve::read(&env, RUN, end, &|| false).unwrap();
    assert_eq!(got.len(), 1);
    // Read once: the next read waits.
    let end = Instant::now() + Duration::from_millis(300);
    assert!(serve::read(&env, RUN, end, &|| false).is_none());
    let o = serve::parse_outgoing(br#"{"kind":"say","text":"main","reply_to":1}"#).unwrap();
    let m = serve::send(&env, RUN, o).unwrap();
    assert_eq!((m.from.as_str(), m.reply_to), (FROM_AGENT, Some(1)));
    let o = serve::parse_outgoing(br#"{"text":"x","to":"other-run-ffffff"}"#).unwrap();
    assert!(matches!(
        serve::send(&env, RUN, o),
        Err(SendError::Denied(_))
    ));
    let o = serve::parse_outgoing(br#"{"kind":"event","text":"x"}"#).unwrap();
    assert!(matches!(
        serve::send(&env, RUN, o),
        Err(SendError::Invalid(_))
    ));
    assert!(serve::parse_outgoing(br#"{"text":"x","from":"paddock"}"#).is_err());
}

#[test]
fn the_host_request_mail_reads_sends_and_reports() {
    use crate::broker::{Request, Status};
    let (_t, env) = env();
    let req = |args: serde_json::Value| Request {
        v: crate::broker::PROTOCOL,
        cap: "mail".into(),
        args: serde_json::from_value(args).unwrap(),
        reason: String::new(),
    };
    let r = serve::handle(&env, RUN, &req(serde_json::json!({"op": "status"})));
    assert_eq!(r.status, Status::Disabled, "no mailbox yet");
    made(&env);
    send_down(&env, RUN, Message::new(HOST, Kind::Say, "go")).unwrap();
    let r = serve::handle(&env, RUN, &req(serde_json::json!({"op": "status"})));
    assert_eq!(r.result["unread"], 1);
    let r = serve::handle(
        &env,
        RUN,
        &req(serde_json::json!({"op": "read", "timeout": 1})),
    );
    assert_eq!(r.status, Status::Done);
    assert_eq!(r.result["messages"][0]["text"], "go");
    let r = serve::handle(
        &env,
        RUN,
        &req(serde_json::json!({"op": "read", "timeout": 1})),
    );
    assert_eq!(r.status, Status::Waiting, "{}", r.message);
    let msg = serde_json::json!({"kind": "status", "text": "done: shipped"});
    let r = serve::handle(
        &env,
        RUN,
        &req(serde_json::json!({"op": "send", "message": msg})),
    );
    // #2 is the `delivered` event of the read.
    assert_eq!(
        (r.status, r.result["seq"].as_u64()),
        (Status::Done, Some(3))
    );
    assert_eq!(read(&env, RUN, Dir::Up)[0].via.as_deref(), Some("read"));
    // The hooks: `deliver` with a text for the agent, or none.
    send_down(&env, RUN, Message::new(HOST, Kind::Say, "more")).unwrap();
    let deliver = |via: &str| {
        serve::handle(
            &env,
            RUN,
            &req(serde_json::json!({"op": "deliver", "via": via})),
        )
    };
    assert!(deliver("tool").result["text"].is_null());
    let r = deliver("stop");
    assert_eq!(r.status, Status::Done);
    assert!(r.result["text"].as_str().unwrap().contains("\nmore\n"));
    assert!(deliver("stop").result["text"].is_null());
    assert_eq!(deliver("read").status, Status::Invalid);
    let msg = serde_json::json!({"text": "x", "to": "other-p-ffffff"});
    let r = serve::handle(
        &env,
        RUN,
        &req(serde_json::json!({"op": "send", "message": msg})),
    );
    assert_eq!(r.status, Status::Denied);
    let msg = serde_json::json!({"text": "y".repeat(MAX_TEXT + 1)});
    let r = serve::handle(
        &env,
        RUN,
        &req(serde_json::json!({"op": "send", "message": msg})),
    );
    assert_eq!(r.status, Status::Invalid);
    assert!(r.message.contains("loft"), "{}", r.message);
}
