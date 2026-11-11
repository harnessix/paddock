use super::*;
use crate::macvm::golden::{Manifest, Prev, State};

const GIB: u64 = 1 << 30;

fn view() -> View {
    let mut m = Manifest::new("50.2", "99B102", "0.2.1");
    m.version = "50.1".into();
    m.build = "99A101".into();
    m.at(State::Sealed);
    View {
        golden: GoldenView::Sealed {
            manifest: m,
            size: 25 * GIB,
            flags_ok: true,
        },
        host_version: Some("50.2".into()),
        apple_silicon: true,
        entitlement: Some(true),
        profiles: Vec::new(),
        sessions: Vec::new(),
        slots: vec![
            Some(Holder::Mine {
                app: "sample-notes".into(),
                pid: 42,
            }),
            None,
        ],
        slots_dir: None,
        free: 184 * GIB,
        prev: Prev::None,
    }
}

fn facts() -> Facts {
    Facts {
        chip: Some("Example Chip".into()),
        memory: Some(32 * GIB),
        dir: "/Users/u/Library/Application Support/paddock/macvm".into(),
        helper: Some("/opt/homebrew/bin/paddock-vm".into()),
        paddock_version: "0.9.0".into(),
    }
}

fn level(j: &Value, id: &str) -> String {
    let c = j["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no check {id}: {j}"));
    c["level"].as_str().unwrap().to_string()
}

/// The JSON of a Mac that passes every check, through a text round trip,
/// with each field that the launcher's checklist reads.
#[test]
fn a_ready_mac() {
    let j: Value = serde_json::from_str(&build(&view(), &facts()).to_string()).unwrap();
    assert_eq!(j["v"], V);
    assert_eq!(j["ok"], true);
    assert_eq!(j["chip"]["name"], "Example Chip");
    assert_eq!(j["chip"]["apple_silicon"], true);
    assert_eq!(j["macos"]["version"], "50.2");
    assert_eq!(j["macos"]["min_major"], 14);
    assert_eq!(j["disk"]["free"], 184 * GIB);
    assert_eq!(j["disk"]["need"], 8 * GIB);
    assert_eq!(j["memory"]["bytes"], 32 * GIB);
    assert_eq!(j["memory"]["per_app_max_mib"], 16 * 1024);
    assert_eq!(j["memory"]["default_mib"], 4096);
    assert_eq!(j["entitlement"]["present"], true);
    assert_eq!(j["slots"]["total"], 2);
    assert_eq!(j["slots"]["used"], 1);
    assert_eq!(j["slots"]["holders"][0]["kind"], "mine");
    assert_eq!(j["slots"]["holders"][0]["app"], "sample-notes");
    assert_eq!(j["slots"]["holders"][0]["pid"], 42);
    assert_eq!(j["slots"]["holders"][1], Value::Null);
    assert_eq!(j["golden"]["state"], "sealed");
    assert_eq!(j["paddock"]["version"], "0.9.0");
    let ids: Vec<&str> = j["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "apple_silicon",
            "macos",
            "free_space",
            "memory",
            "entitlement",
            "slots"
        ]
    );
    for c in j["checks"].as_array().unwrap() {
        assert_eq!(c["level"], "ok", "{c}");
        assert!(!c["text"].as_str().unwrap().is_empty());
    }
}

#[test]
fn each_failure_has_its_level() {
    let mut v = view();
    v.apple_silicon = false;
    v.host_version = Some("13.6".into());
    v.entitlement = None;
    v.golden = GoldenView::Missing;
    v.free = 12 * GIB;
    v.slots = vec![
        Some(Holder::User { name: "ann".into() }),
        Some(Holder::OtherProgram),
    ];
    let mut f = facts();
    f.memory = Some(8 * GIB);
    f.helper = None;
    let j = build(&v, &f);
    assert_eq!(j["ok"], false);
    for (id, want) in [
        ("apple_silicon", "fail"),
        ("macos", "fail"),
        ("free_space", "fail"),
        ("memory", "warn"),
        ("entitlement", "fail"),
        ("slots", "warn"),
    ] {
        assert_eq!(level(&j, id), want, "{id}");
    }
    // No golden: the need is the download and the install.
    assert_eq!(
        j["disk"]["need"],
        crate::macvm::golden::space_needed(State::Fetch, None)
    );
    assert_eq!(j["slots"]["holders"][0]["user"], "ann");
    assert_eq!(j["slots"]["holders"][1]["kind"], "other_program");
    assert_eq!(j["golden"]["state"], "missing");
}

#[test]
fn warnings_keep_ok() {
    let mut v = view();
    v.slots = vec![
        Some(Holder::Mine {
            app: "a".into(),
            pid: 1,
        }),
        Some(Holder::Mine {
            app: "b".into(),
            pid: 2,
        }),
    ];
    let mut f = facts();
    f.memory = None;
    let j = build(&v, &f);
    assert_eq!(level(&j, "slots"), "warn");
    assert_eq!(level(&j, "memory"), "warn");
    assert_eq!(j["ok"], true);
    assert_eq!(j["memory"]["per_app_max_mib"], Value::Null);
}

#[test]
fn unknown_macos_fails_and_an_unsealed_golden_needs_its_state() {
    let mut v = view();
    v.host_version = None;
    let mut m = Manifest::new("50.2", "99B102", "0.2.1");
    m.at(State::Install);
    v.golden = GoldenView::Unsealed(m);
    let j = build(&v, &facts());
    assert_eq!(level(&j, "macos"), "fail");
    assert_eq!(
        j["disk"]["need"],
        crate::macvm::golden::space_needed(State::Install, None)
    );
    assert_eq!(j["golden"]["state"], "install");
}
