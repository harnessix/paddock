//! The status screen of `paddock app status`: the four fixtures, the JSON, the
//! next start of each kind, and the counts of the connection log.

use super::*;
use crate::macvm::host::{RecordApp, RecordHealth, RecordStart};
use crate::macvm::proto::Version;
use crate::macvm::session::{AppStamp, Display, Now, Reason};

const GIB: u64 = 1 << 30;
const NOW: u64 = 1_700_000_000;

fn now_values() -> Now {
    Now {
        golden: "50.2 (99B102)".into(),
        host_build: "99B102".into(),
        vz: "vz-1".into(),
        protocol: Version { major: 1, minor: 3 },
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    }
}

fn stamp(clean: bool, saved: bool) -> Stamp {
    let mut s = now_values().stamp("abcd-3".into());
    s.app = Some(AppStamp {
        version: "1.2.3".into(),
        build: "123456".into(),
        copied: Some("2000-01-02".into()),
    });
    s.clean = clean;
    s.data_prepared = true;
    if saved {
        s.state_id = Some("abcd-3".into());
        s.state_len = Some(4 * GIB);
    }
    s
}

fn record() -> Session {
    Session {
        pid: 812,
        app: "sample-notes".into(),
        home: "default".into(),
        profile: PathBuf::from("/p/sample-notes/default"),
        slot: Some(0),
        input_source: String::new(),
        bench: false,
        started: NOW - (2 * 3600 + 14 * 60 + 5),
        control: PathBuf::from("/run/macvm-sample-notes/control.sock"),
        badge: None,
        health: Some(RecordHealth {
            t: NOW - 1,
            load: 0.4,
            mem_free: 19 * GIB / 10,
            data_disk_free: Some(142 * GIB / 10),
            app_running: true,
            windows: 2,
            input_source: Some("com.apple.keylayout.US".into()),
            locked: false,
        }),
        start: Some(RecordStart {
            kind: "warm".into(),
            secs: 4.1,
        }),
        app_info: Some(RecordApp {
            name: "Sample Notes".into(),
            version: "1.2.3".into(),
        }),
        network: None,
        network_mode: "ask".into(),
        shares: vec![host::RecordShare {
            path: PathBuf::from("/Users/u/src/web"),
            read_only: false,
            ..Default::default()
        }],
        menu: None,
        ax: None,
        ..Default::default()
    }
}

fn inputs(record: Option<Session>, stamp: Option<Stamp>, next: Start) -> Inputs {
    let mut connections = Counts::default();
    if record.is_some() {
        connections = count_connections(
            br#"{"t":1,"host":"notes.example.com","port":443,"verdict":"allowed"}
{"t":2,"host":"telemetry.example.com","port":443,"verdict":"denied"}
{"t":3,"host":"notes.example.com","port":443,"verdict":"allowed"}
"#,
        );
        connections.allowed = 14;
        connections.denied = 2;
    }
    Inputs {
        name: "sample-notes".into(),
        profile: Profile::new("/p/sample-notes/default"),
        record,
        stamp,
        next: Ok(next),
        profile_bytes: 63 * GIB / 10,
        data_bytes: 41 * GIB / 10,
        state_bytes: 4 * GIB,
        disk_free: 120 * GIB,
        shares: vec![(PathBuf::from("/Users/u/src/web"), true)],
        connections,
        asks: Default::default(),
        now: NOW,
    }
}

#[test]
fn render_fixtures() {
    let running = build(&inputs(
        Some(record()),
        Some(stamp(false, false)),
        Start::Warm,
    ));
    assert_eq!(
        render(&running),
        "\
sample-notes      running (pid 812, 2h 14m), warm start in 4.1 s
golden            macOS 50.2 (99B102), agent 0.9.0, protocol 1.3
app               Sample Notes 1.2.3 (copied 2000-01-02), running, 2 windows
guest             healthy (last health 1 s ago), load 0.4, 1.9 GiB free RAM, data disk 14.2 GiB free
host              profile 6.3 GiB (data 4.1 GiB, state 4 GiB), disk 120 GiB free
data disk         prepared: the home of the guest is on data.img
network           ask, 14 allowed, 2 denied in this session (last: notes.example.com:443)
shares            /Users/u/src/web (rw), downloads -> /p/sample-notes/default/downloads
logs              /p/sample-notes/default/logs/
questions         none open
"
    );
    assert_eq!(running.state, State::Running);
    assert_eq!(running.health_age_s, Some(1));
    assert_eq!(running.windows, Some(2));
    assert_eq!(running.data_free_gib, Some(14.2));

    // Running, before the first health and without an app of its own.
    let mut r = record();
    r.health = None;
    r.app_info = None;
    r.start = None;
    let mut s = stamp(false, false);
    s.app = None;
    let booting = build(&inputs(Some(r), Some(s), Start::Warm));
    let text = render(&booting);
    assert!(
        text.starts_with("sample-notes      running (pid 812, 2h 14m)\n"),
        "{text}"
    );
    assert!(text.contains("app               none (the session runs the guest without an app)\n"));
    assert!(text.contains("guest             booting (no health yet)\n"));
    assert!(booting.health_age_s.is_none());

    // The guest stopped answering.
    let mut r = record();
    r.health.as_mut().unwrap().t = NOW - 42;
    let silent = build(&inputs(Some(r), Some(stamp(false, false)), Start::Warm));
    assert!(render(&silent)
        .contains("guest             not answering (last health 42 s ago), load 0.4"));

    let clean = build(&inputs(None, Some(stamp(true, true)), Start::Warm));
    assert_eq!(
        render(&clean),
        "\
sample-notes      stopped (quit with save), next start warm
golden            macOS 50.2 (99B102), agent 0.9.0, protocol 1.3
app               sample-notes 1.2.3 (copied 2000-01-02)
guest             stopped
host              profile 6.3 GiB (data 4.1 GiB, state 4 GiB), disk 120 GiB free
data disk         prepared: the home of the guest is on data.img
network           no session
shares            /Users/u/src/web (rw), downloads -> /p/sample-notes/default/downloads
logs              /p/sample-notes/default/logs/
"
    );
    assert_eq!(clean.state, State::Stopped);
    assert_eq!(clean.last_end.as_deref(), Some("quit with save"));
    assert_eq!(clean.next_start, "warm");

    let crashed = build(&inputs(
        None,
        Some(stamp(false, false)),
        Start::Stale(Reason::GoldenChanged),
    ));
    let text = render(&crashed);
    assert!(
        text.starts_with(
            "sample-notes      stopped (crashed), next start cold: the guest image changed\n"
        ),
        "{text}"
    );
    assert_eq!(crashed.last_end.as_deref(), Some("crashed"));
    assert!(text.contains("data disk         prepared:"), "{text}");

    // A start that could not prepare the data disk names the step.
    let mut s = stamp(false, false);
    s.data_prepared = false;
    s.data_error = Some("the data disk was not prepared: copy the home: ditto failed".into());
    let failed = build(&inputs(None, Some(s), Start::Cold));
    assert_eq!(
        failed.data,
        "not prepared: the data disk was not prepared: copy the home: ditto failed"
    );
    assert!(render(&failed)
        .contains("data disk         not prepared: the data disk was not prepared: copy the home"));

    let never = build(&inputs(None, None, Start::Fresh));
    assert_eq!(
        render(&never),
        "\
sample-notes      never started, next start cold: the first start of this profile
golden            not started on this golden yet
app               none (the session runs the guest without an app)
guest             stopped
host              profile 6.3 GiB (data 4.1 GiB, state 4 GiB), disk 120 GiB free
data disk         not prepared yet: the next cold start prepares it
network           no session
shares            /Users/u/src/web (rw), downloads -> /p/sample-notes/default/downloads
logs              /p/sample-notes/default/logs/
"
    );
    assert_eq!(never.state, State::NeverStarted);
    assert!(never.last_end.is_none());
}

#[test]
fn json_has_every_field() {
    let running = build(&inputs(
        Some(record()),
        Some(stamp(false, false)),
        Start::Warm,
    ));
    let v = serde_json::to_value(&running).unwrap();
    let o = v.as_object().unwrap();
    for key in [
        "name",
        "state",
        "pid",
        "uptime_s",
        "start_kind",
        "start_secs",
        "next_start",
        "golden",
        "agent",
        "protocol",
        "app",
        "guest",
        "health_age_s",
        "windows",
        "data_free_gib",
        "host",
        "data",
        "network",
        "shares",
        "downloads",
        "logs",
    ] {
        assert!(o.contains_key(key), "{key} missing in {v}");
    }
    assert_eq!(v["state"], "running");
    assert_eq!(v["windows"], 2);
    assert_eq!(v["health_age_s"], 1);
    assert_eq!(v["app"]["name"], "Sample Notes");
    assert_eq!(v["app"]["copied"], "2000-01-02");
    assert_eq!(v["guest"]["answering"], true);
    assert_eq!(v["guest"]["load"], 0.4);
    assert_eq!(v["host"]["disk_free"], 120 * GIB);
    assert_eq!(v["network"]["mode"], "ask");
    assert_eq!(v["network"]["allowed"], 14);
    assert_eq!(v["shares"][0]["writable"], true);
    for log in ["dir", "shim", "events", "console", "agent", "boundary"] {
        assert!(v["logs"][log].is_string(), "{log}");
    }
    assert_eq!(v["logs"]["agent"], "/p/sample-notes/default/logs/agent.log");
    // A stopped profile: the running fields are absent, the end is there.
    let v =
        serde_json::to_value(build(&inputs(None, Some(stamp(true, false)), Start::Cold))).unwrap();
    assert_eq!(v["state"], "stopped");
    assert_eq!(v["last_end"], "quit without save");
    assert_eq!(v["next_start"], "cold: no saved state");
    assert!(v.get("pid").is_none());
    assert!(v.get("guest").is_none());
}

#[test]
fn next_start_per_kind() {
    assert_eq!(next_start(Start::Warm), "warm");
    assert_eq!(next_start(Start::Cold), "cold: no saved state");
    assert_eq!(
        next_start(Start::Fresh),
        "cold: the first start of this profile"
    );
    assert_eq!(next_start(Start::Repair), "cold: the profile was rebuilt");
    assert_eq!(
        next_start(Start::Stale(Reason::GoldenChanged)),
        "cold: the guest image changed"
    );
    assert_eq!(
        next_start(Start::Stale(Reason::HostChanged)),
        "cold: this Mac runs a new macOS build"
    );
    for r in [
        Reason::ProtocolChanged,
        Reason::DisplayChanged,
        Reason::StateDamaged,
        Reason::Unreadable,
    ] {
        assert_eq!(next_start(Start::Stale(r)), "cold: no saved state", "{r:?}");
    }
}

#[test]
fn connection_counts_by_verdict() {
    let c = count_connections(
        br#"{"host":"a.example","port":443,"verdict":"allowed"}
not json
{"host":"b.example","port":80,"verdict":"denied"}
{"host":"a.example","port":443,"verdict":"asked"}
{"host":"c.example","verdict":"allowed"}
"#,
    );
    assert_eq!(c.allowed, 2);
    assert_eq!(c.denied, 1);
    assert_eq!(c.other, vec![("asked".to_string(), 1)]);
    assert_eq!(c.last.as_deref(), Some("c.example"));
    assert_eq!(c.hosts, ["a.example", "b.example", "c.example"]);
    assert_eq!(
        c.by_verdict(),
        vec![
            ("allowed".to_string(), 2),
            ("denied".to_string(), 1),
            ("asked".to_string(), 1)
        ]
    );
    assert_eq!(count_connections(b""), Counts::default());
    // A merged line of the connection log counts its `count`.
    let c = count_connections(
        br#"{"host":"a.example","port":443,"verdict":"allow","count":3}
{"host":"b.example","port":443,"verdict":"deny","count":2}
"#,
    );
    assert_eq!((c.allowed, c.denied), (3, 2));
}

#[test]
fn read_builds_inputs_from_a_profile_folder() {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        t.path().join("home"),
        "u".into(),
        501,
        20,
        [(
            "XDG_DATA_HOME".to_string(),
            t.path().join("data").display().to_string(),
        )]
        .into_iter()
        .collect(),
    );
    assert!(read(&env, "../x", 1).is_none());
    let i = read(&env, "sample-editor", 1).unwrap();
    assert!(i.record.is_none() && i.stamp.is_none());
    assert_eq!(i.next, Ok(Start::Fresh));
    let s = build(&i);
    assert_eq!(s.state, State::NeverStarted);
    assert_eq!(cmd(&env, "sample-editor", 1, false).unwrap(), 1);
    // A stopped profile with a stamp.
    let layout = Layout::new(&env);
    let p = layout.profile("sample-editor", crate::macvm::host::DEFAULT_HOME);
    std::fs::create_dir_all(p.logs()).unwrap();
    stamp(true, false).write(&p.stamp()).unwrap();
    for f in [p.data(), p.system(), p.aux(), p.machine_id()] {
        std::fs::write(f, "x").unwrap();
    }
    let i = read(&env, "sample-editor", 1).unwrap();
    assert!(i.stamp.is_some());
    assert_eq!(build(&i).last_end.as_deref(), Some("quit without save"));
    assert_eq!(cmd(&env, "sample-editor", 1, true).unwrap(), 0);
    assert_eq!(cmd(&env, "sample-editor", 1, false).unwrap(), 0);
}

#[test]
fn menu_in_json_while_running() {
    use crate::macvm::menu::MenuRecord;
    let mut r = record();
    r.menu = Some(MenuRecord {
        shown: "tree".into(),
        items: 42,
        away: false,
    });
    let v = serde_json::to_value(build(&inputs(
        Some(r),
        Some(stamp(false, false)),
        Start::Warm,
    )))
    .unwrap();
    assert_eq!(v["menu"]["shown"], "tree");
    assert_eq!(v["menu"]["items"], 42);
    assert_eq!(v["menu"]["away"], false);
    assert!(v.get("ax").is_none());
    let mut r = record();
    r.ax = Some(crate::macvm::axmirror::AxRecord {
        window: Some(7),
        elements: 12,
        watched: true,
        away: false,
    });
    let v = serde_json::to_value(build(&inputs(
        Some(r),
        Some(stamp(false, false)),
        Start::Warm,
    )))
    .unwrap();
    assert_eq!(v["ax"]["window"], 7);
    assert_eq!(v["ax"]["elements"], 12);
    assert_eq!(v["ax"]["watched"], true);
    // No menu field before the first tree, and none for a stopped profile.
    let v = serde_json::to_value(build(&inputs(
        Some(record()),
        Some(stamp(false, false)),
        Start::Warm,
    )))
    .unwrap();
    assert!(v.get("menu").is_none());
    let v =
        serde_json::to_value(build(&inputs(None, Some(stamp(true, false)), Start::Cold))).unwrap();
    assert!(v.get("menu").is_none());
}

#[test]
fn open_questions_and_the_guest_system_show_while_running() {
    use crate::macvm::ask_status::{count_system, AskStatus, OpenQuestion};
    let mut i = inputs(Some(record()), Some(stamp(false, false)), Start::Warm);
    let mut asks = AskStatus {
        paused: true,
        ..Default::default()
    };
    asks.open.push(OpenQuestion {
        app: "com.example.notes".into(),
        host: "a.example".into(),
        port: 443,
        waiting: 2,
        asked_at: 1,
        shown: false,
    });
    for _ in 0..3 {
        count_system(
            &mut asks.system,
            "weather-edge.apple.com",
            443,
            "com.apple.weather",
        );
    }
    i.asks = asks;
    let s = build(&i);
    let text = render(&s);
    assert!(
        text.ends_with(
            "questions         1 open, asking paused: answer the notice, or add rules with paddock app rules; 3 connections of the guest system denied without asking
  com.example.notes -> a.example:443, 2 connections wait
  denied weather-edge.apple.com:443 x3 (guest system, last process com.apple.weather)
"
        ),
        "{text}"
    );
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["questions"]["paused"], true);
    assert_eq!(json["questions"]["system"][0]["count"], 3);
    // A stopped profile shows no questions.
    let stopped = build(&inputs(None, Some(stamp(true, true)), Start::Warm));
    assert!(!render(&stopped).contains("questions"));
}
