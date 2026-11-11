use super::*;
use std::path::Path;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/launcher")
}

/// Every line of every fixture parses into the schema, has the version of
/// this paddock and the kind of its file, and serializes back into the
/// same JSON: the fixtures are what paddock prints.
#[test]
fn every_fixture_line_round_trips() {
    for kind in KINDS {
        let path = fixtures().join(format!("{kind}.jsonl"));
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut n = 0;
        for raw in text.lines() {
            n += 1;
            assert!(valid_line(raw), "{raw}");
            let line: Line =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{kind}: {e}: {raw}"));
            assert_eq!(line.v, V, "{raw}");
            assert_eq!(line.event.kind(), kind, "{raw}");
            let back: serde_json::Value = serde_json::from_str(&line.to_text()).unwrap();
            let want: serde_json::Value = serde_json::from_str(raw).unwrap();
            assert_eq!(back, want, "{kind}: the fixture is not what paddock prints");
        }
        assert!(n >= 2, "{kind}: a fixture needs every state, {n} lines");
    }
}

/// The fixture folder holds a file for each kind and nothing else.
#[test]
fn a_fixture_for_each_kind_and_no_other() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut want: Vec<String> = KINDS.iter().map(|k| format!("{k}.jsonl")).collect();
    want.sort();
    assert_eq!(names, want);
}

/// The kind word of each variant is the `kind` field that serde writes.
#[test]
fn kind_word_is_the_serde_tag() {
    let events = [
        Event::Session(Session {
            app: "a".into(),
            home: "default".into(),
            state: SessionState::Running,
            phase: None,
            text: String::new(),
        }),
        Event::Question(Question {
            state: QuestionState::Open,
            id: "r".into(),
            app: "a".into(),
            host: "h".into(),
            port: 1,
            process: None,
            answer: None,
            by: None,
            open: None,
        }),
        Event::Grant(Grant {
            app: "a".into(),
            what: GrantWhat::Rule,
            action: "add".into(),
            detail: "d".into(),
            source: GrantSource::You,
            applies: Applies::Live,
            id: None,
        }),
        Event::Setup(Setup::default()),
        Event::Golden(Golden {
            state: GoldenState::Removed,
            version: None,
            bytes: 0,
        }),
        Event::Disk(Disk {
            dir: "/m".into(),
            free: 1,
        }),
        Event::Stream(Stream {
            state: StreamState::Started,
        }),
    ];
    assert_eq!(events.len(), KINDS.len());
    for (e, k) in events.iter().zip(KINDS) {
        assert_eq!(e.kind(), k);
        let v: serde_json::Value = serde_json::to_value(Line::at(0, e.clone())).unwrap();
        assert_eq!(v["kind"], k);
        assert_eq!(v["v"], V);
        assert_eq!(v["t"], "1970-01-01T00:00:00Z");
    }
}

/// A reader of this version takes a line with a field it does not know,
/// and the follower passes a kind it does not know.
#[test]
fn unknown_fields_and_kinds() {
    let raw =
        r#"{"v":1,"t":"2026-11-06T12:00:00Z","kind":"disk","dir":"/m","free":5,"later":true}"#;
    let line: Line = serde_json::from_str(raw).unwrap();
    assert_eq!(
        line.event,
        Event::Disk(Disk {
            dir: "/m".into(),
            free: 5
        })
    );
    assert!(valid_line(r#"{"v":2,"kind":"window","id":3}"#));
    assert!(serde_json::from_str::<Line>(r#"{"v":2,"t":"x","kind":"window"}"#).is_err());
}

#[test]
fn invalid_lines_are_refused() {
    for raw in [
        "",
        "not json",
        "[1,2]",
        r#"{"kind":"disk"}"#,
        r#"{"v":"1","kind":"disk"}"#,
        r#"{"v":1}"#,
        r#"{"v":1,"kind":3}"#,
    ] {
        assert!(!valid_line(raw), "{raw}");
    }
    let long = format!(r#"{{"v":1,"kind":"x","p":"{}"}}"#, "a".repeat(MAX_LINE));
    assert!(!valid_line(&long));
}
