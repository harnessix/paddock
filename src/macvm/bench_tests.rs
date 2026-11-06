use super::*;
use crate::macvm::events::Event;

fn ev(secs: u64, ms: u16, kind: &str, text: &str) -> Event {
    Event {
        t: crate::waiting::format_rfc3339(secs),
        ms,
        kind: kind.into(),
        text: text.into(),
        n: Default::default(),
    }
}

/// The committed baseline meets every target of targets.json: a target change
/// and a baseline change are one commit. The failure names the measure
/// and both numbers.
#[test]
fn baseline_meets_targets() {
    let targets = targets();
    let run = Run::parse(BASELINE_JSON).unwrap();
    let misses = check(&targets, &run.measures);
    assert!(
        misses.is_empty(),
        "the baseline misses: {}",
        misses
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    );
    assert_eq!(run.measures.len(), 13);
    if run.measured {
        assert!(!run.model.is_empty() && !run.macos.is_empty() && !run.paddock.is_empty());
    } else {
        assert_eq!(run.model, "not measured yet");
    }
}

/// targets.json names every measure of every case and nothing else.
#[test]
fn targets_cover_every_case() {
    let targets = targets();
    let mut expected: Vec<&str> = Case::ALL
        .iter()
        .flat_map(|c| c.measures().iter().copied())
        .collect();
    expected.sort_unstable();
    let got: Vec<&str> = targets.keys().map(|k| k.as_str()).collect();
    assert_eq!(got, expected);
    for (name, t) in &targets {
        assert!(
            t.p50.is_some() || t.p95.is_some() || t.mean.is_some(),
            "{name} has no bound"
        );
        assert!(t.samples >= 1, "{name} needs at least one sample");
        let case = Case::ALL
            .iter()
            .find(|c| c.measures().contains(&name.as_str()))
            .unwrap();
        if case.measures().len() == 1 {
            assert_eq!(t.unit, case.unit(), "{name}");
        }
    }
    for c in Case::ALL {
        assert_eq!(Case::parse(c.name()), Some(c));
    }
    assert_eq!(Case::parse("fast"), None);
    assert!(Case::names().starts_with("warm, cold"));
}

/// The pure guard of the agent: every bench op is refused without the
/// flag, every one passes with it, and a plain request never minds.
#[test]
fn ops_refused_without_flag() {
    let ops = [
        Request::BenchAnimate { window: 1, secs: 3 },
        Request::BenchText { window: 1 },
        Request::BenchHang,
        Request::BenchSpawnChild {
            argv: vec!["/usr/bin/true".into()],
        },
        Request::BenchPasteboard {
            action: "get".into(),
            text: String::new(),
        },
        Request::BenchEnv,
        Request::BenchDockMenu,
    ];
    for op in &ops {
        assert!(is_bench_op(op), "{}", op.kind());
        let r = op_gate(7, op, false).expect("refused");
        assert_eq!(r.req, 7);
        match r.outcome {
            super::super::proto::Outcome::Error { code, text } => {
                assert_eq!(code, ErrorCode::Unsupported);
                assert!(text.contains("--bench"), "{text}");
            }
            other => panic!("{other:?}"),
        }
        assert!(op_gate(7, op, true).is_none(), "{}", op.kind());
        // The wire names stay `bench.<name>`.
        let json = serde_json::to_string(op).unwrap();
        assert!(json.contains("\"op\":\"bench."), "{json}");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, op);
    }
    for plain in [
        Request::Ping { t: 1 },
        Request::AxWatch { on: true },
        Request::Shutdown {
            password: String::new(),
        },
    ] {
        assert!(!is_bench_op(&plain));
        assert!(op_gate(1, &plain, false).is_none());
    }
    // `secs` of bench.animate has a default on the wire.
    let r: Request = serde_json::from_str(r#"{"op":"bench.animate","window":3}"#).unwrap();
    assert_eq!(r, Request::BenchAnimate { window: 3, secs: 4 });
    // The token carries the flag, and leaves it out when off.
    let t = serde_json::to_string(&Request::Token {
        token: "t".into(),
        v: super::super::proto::VERSION,
        bench: false,
    })
    .unwrap();
    assert!(!t.contains("bench"), "{t}");
}

/// The percentiles of a fixed list: nearest rank, and the mean and max.
#[test]
fn percentiles() {
    let values: Vec<f64> = (1..=100).map(f64::from).collect();
    let m = Measure::from_samples("input", "ms", &values);
    assert_eq!((m.p50, m.p95, m.max, m.samples), (50.0, 95.0, 100.0, 100));
    assert_eq!(m.mean, 50.5);
    let m = Measure::from_samples("warm", "s", &[9.0, 3.0, 5.0, 7.0, 1.0]);
    assert_eq!((m.p50, m.p95, m.max), (5.0, 9.0, 9.0));
    let m = Measure::from_samples("quit", "s", &[4.2]);
    assert_eq!((m.p50, m.p95, m.mean), (4.2, 4.2, 4.2));
    let m = Measure::from_samples("none", "s", &[]);
    assert_eq!((m.p50, m.samples), (0.0, 0));
    assert_eq!(percentile(&[], 50.0), 0.0);
    let m = Measure::from_samples("nan", "s", &[1.0, f64::NAN, 3.0]);
    assert_eq!(m.samples, 2);
}

/// A miss names the measure and both numbers; a missing measure and too
/// few samples are misses too; the environment lowers a target.
#[test]
fn misses_and_env_targets() {
    let mut targets = targets();
    let mut run = Run::parse(BASELINE_JSON).unwrap();
    run.add(Measure::from_samples(
        "warm",
        "s",
        &[12.5, 1.0, 1.0, 1.0, 1.0],
    ));
    run.measures.remove("disk");
    run.add(Measure::from_samples("input", "ms", &[1.0, 2.0]));
    let misses = check(&targets, &run.measures);
    let lines: Vec<String> = misses.iter().map(|m| m.to_string()).collect();
    assert_eq!(
        lines,
        [
            "disk: no measure",
            "input: 2 samples, the target needs 200",
            "warm p95 12.5 s is over the target 10 s",
        ]
    );
    let changed = apply_env_targets(
        &mut targets,
        [
            (
                "PADDOCK_MACVM_BENCH_TARGET_IDLE_CPU".to_string(),
                "0.5".to_string(),
            ),
            (
                "PADDOCK_MACVM_BENCH_TARGET_WARM".to_string(),
                "1".to_string(),
            ),
            (
                "PADDOCK_MACVM_BENCH_TARGET_NOPE".to_string(),
                "1".to_string(),
            ),
            (
                "PADDOCK_MACVM_BENCH_TARGET_COLD".to_string(),
                "x".to_string(),
            ),
            ("HOME".to_string(), "/x".to_string()),
        ]
        .into_iter(),
    );
    assert_eq!(changed, ["idle.cpu", "warm"]);
    assert_eq!(targets["warm"].p50, Some(1.0));
    assert_eq!(targets["warm"].p95, Some(1.0));
    assert_eq!(targets["idle.cpu"].mean, Some(0.5));
    assert_eq!(targets["cold"].p95, Some(45.0));
    let base = Run::parse(BASELINE_JSON).unwrap();
    let misses = check(&targets, &base.measures);
    assert!(misses
        .iter()
        .any(|m| m.measure == "warm" && m.stat == "p50"));
    assert!(misses
        .iter()
        .any(|m| m.measure == "warm" && m.stat == "p95"));
    let one = check_case(Case::Warm, &targets, &base.measures);
    assert!(one.iter().all(|m| m.measure == "warm") && one.len() == 2);
    let table = table(&run, &targets);
    assert!(table.contains("MISS disk: no measure"), "{table}");
    assert!(table.lines().count() == 14, "{table}");
    assert!(allowed(true, None).is_ok());
    assert!(allowed(false, Some("1")).is_ok());
    assert!(allowed(false, None)
        .unwrap_err()
        .contains("PADDOCK_MACVM_BENCH"));
    assert!(parse_targets("{\"warm\": {\"p95\": \"x\"}}").is_err());
}

/// The event log math: the start cases, the quit, the update, with
/// millisecond times, and the errors when a line is missing.
#[test]
fn event_log_cases() {
    let events = vec![
        ev(100, 0, "start", "warm"),
        ev(101, 250, "hello", "x"),
        ev(104, 500, "window", "opened"),
        ev(105, 0, "window", "closed"),
        ev(200, 0, "agent", "the agent updates itself"),
        ev(202, 100, "hello", "x"),
        ev(300, 900, "quit", "cmd-q"),
        ev(301, 0, "save", "x"),
        ev(306, 400, "end", "the session ended"),
    ];
    assert_eq!(from_events(Case::Warm, &events).unwrap(), 4.5);
    assert_eq!(from_events(Case::Cold, &events).unwrap(), 4.5);
    assert_eq!(from_events(Case::Update, &events).unwrap(), 2.1);
    assert_eq!(from_events(Case::Quit, &events).unwrap(), 5.5);
    // The last start counts, and a start with no window after it is an error.
    let mut again = events.clone();
    again.push(ev(400, 0, "start", "cold"));
    let e = from_events(Case::Warm, &again).unwrap_err();
    assert!(e.contains("no end line of warm"), "{e}");
    again.push(ev(430, 0, "window", "opened"));
    assert_eq!(from_events(Case::Cold, &again).unwrap(), 30.0);
    let e = from_events(Case::Quit, &[]).unwrap_err();
    assert!(e.contains("no start line of quit"), "{e}");
    assert!(from_events(Case::Input, &events).is_err());
    // The move samples come from the shim's `bench` lines.
    let mut mv = ev(1, 0, "bench", "windows applied");
    mv.n.insert("ms".into(), serde_json::json!(2.5));
    let other = ev(1, 0, "bench", "windows event injected");
    assert_eq!(move_samples(&[mv.clone(), other, mv]), [2.5, 2.5]);
    // An event line written now carries its milliseconds and parses back.
    let t = tempfile::tempdir().unwrap();
    let log = super::super::events::Events::at(&t.path().join("e.jsonl"));
    log.record("start", "x", &[]);
    log.record_ms(5, 250, "window", "opened", &[]);
    let back = super::super::events::read(log.path());
    assert_eq!(back.len(), 2);
    assert_eq!((back[1].secs(), back[1].ms), (5, 250));
    assert!(std::fs::read_to_string(log.path())
        .unwrap()
        .contains("\"ms\":250"));
}

/// The pairing of the input case, the frame gaps and the `ps` and `du`
/// parsers.
#[test]
fn input_pairs_and_parsers() {
    let sent = [('a', 1000), ('b', 1100), ('c', 1200), ('d', 1300)];
    let seen = vec![
        (990, "".to_string()),
        (1012, "a".to_string()),
        (1130, "ab".to_string()),
        (1131, "ab".to_string()),
        (1340, "abcd".to_string()),
    ];
    // `c` has no value of its own (the field jumped to `abcd`), so it is dropped.
    assert_eq!(pair_input(&sent, &seen), [12.0, 30.0, 40.0]);
    assert_eq!(pair_input(&[], &seen), Vec::<f64>::new());
    assert_eq!(frame_gaps(&[0.0, 16.5, 33.0, 83.0]), [16.5, 16.5, 50.0]);
    let ps = "  812 Thu Nov  5 10:00:00 2026 /usr/libexec/paddock\n 9001 Thu Nov  5 10:00:01 2026 /System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/com.apple.Virtualization.VirtualMachine\n 8000 Thu Nov  5 09:00:00 2026 com.apple.Virtualization.VirtualMachine\n";
    assert_eq!(
        newest_process(ps, "com.apple.Virtualization.VirtualMachine"),
        Some(9001)
    );
    assert_eq!(newest_process(ps, "nothing"), None);
    assert_eq!(parse_ps_sample(" 2.5 1048576\n"), Some((2.5, 1.0)));
    assert_eq!(parse_ps_sample("x"), None);
    assert_eq!(parse_du_gib("2097152\t/p\n"), Some(2.0));
    assert_eq!(parse_du_gib(""), None);
    use super::super::bench_run::{first_window, parse_axt, parse_frames, parse_keys};
    assert_eq!(
        parse_keys("KEY a 1000\nnoise\nKEY b 1100\n"),
        [('a', 1000), ('b', 1100)]
    );
    assert_eq!(parse_axt("1012 4 a b"), Some((1012, "a b".into())));
    assert_eq!(parse_axt("4 a"), None);
    assert_eq!(parse_frames("FRAME 1.5\nFRAME 18\nx\n"), [1.5, 18.0]);
    assert_eq!(
        first_window("ok [{\"id\":4,\"title\":\"Untitled\",\"number\":77},{\"id\":5}]"),
        Some((4, 77))
    );
    assert_eq!(first_window("ok []"), None);
}
