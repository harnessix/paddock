//! The `macvm` section of the report: the planted secrets (item 18 of
//! the checklist), `--no-hosts`, and a missing log.

use super::*;
use crate::macvm::events::Events;
use crate::macvm::golden::{Manifest, State};
use crate::macvm::proto::Version;
use crate::macvm::proto::{Event, FileHeader, FileKind, PasteItem, Request};
use crate::macvm::session::{Display, Now, Profile};
use std::collections::BTreeMap;
use std::io::Write;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "hunter2-planted-7Q";
const CLIP: &str = "CLIPBOARD-MARKER-9f3e";

fn env(t: &Path) -> HostEnv {
    HostEnv::from_parts(
        t.join("home"),
        "u".into(),
        501,
        20,
        [
            (
                "XDG_DATA_HOME".to_string(),
                t.join("data").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                t.join("config").display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

fn sealed_golden(layout: &Layout) {
    std::fs::create_dir_all(&layout.golden.dir).unwrap();
    let mut m = Manifest::new("15.2", "24C101", "0.9.0");
    m.version = "15.2".into();
    m.build = "24C101".into();
    m.sealed = Some("2026-11-01T10:00:00Z".into());
    m.at(State::Sealed);
    layout.golden.write_manifest(&m).unwrap();
}

fn stamped_profile(layout: &Layout, app: &str) -> Profile {
    let p = layout.profile(app, "default");
    std::fs::create_dir_all(p.logs()).unwrap();
    let now = Now {
        golden: "15.2 (24C101)".into(),
        host_build: "24C101".into(),
        vz: "vz".into(),
        protocol: Version { major: 1, minor: 3 },
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    };
    now.stamp("abcd1234-3".into()).write(&p.stamp()).unwrap();
    p
}

/// The clipboard messages, as the shim and the agent log them: through
/// `kind()`, which strips the payload.
fn clipboard_kinds() -> Vec<String> {
    let item = PasteItem {
        types: BTreeMap::from([("public.utf8-plain-text".to_string(), CLIP.to_string())]),
    };
    vec![
        Request::ClipboardSet {
            count: 1,
            items: vec![item.clone()],
            transfer: None,
        }
        .kind(),
        Event::Clipboard {
            count: 2,
            items: vec![item],
            transfer: Some(CLIP.to_string()),
        }
        .kind(),
        FileHeader {
            token: TOKEN.into(),
            kind: FileKind::Clipboard,
            name: "clip-1.json".into(),
            size: 9,
        }
        .kind(),
    ]
}

/// Plant the token and the password raw in each log, and the clipboard
/// text through the log path of each side.
fn plant(p: &Profile) {
    let kinds = clipboard_kinds();
    let raw = [
        format!("token={TOKEN}"),
        format!("{{\"token\":\"{TOKEN}\",\"v\":1}}"),
        format!("Token from host: {TOKEN} accepted"),
        format!("password: {PASSWORD}"),
        format!("the guest password is {PASSWORD}"),
        format!("GUEST_PASSWD={PASSWORD}"),
    ];
    // The shim log and the console log: plain lines.
    for path in [p.shim_log(), p.console_log(), p.boundary_log()] {
        let mut f = std::fs::File::create(&path).unwrap();
        for l in &raw {
            writeln!(f, "2026-11-04T10:00:00Z INFO {l}").unwrap();
        }
        for k in &kinds {
            writeln!(f, "2026-11-04T10:00:01Z DEBUG macvm: <- {k}").unwrap();
        }
    }
    // The event log: the writer.
    let ev = Events::open(p);
    for l in &raw {
        ev.record("error", l, &[]);
    }
    for k in &kinds {
        ev.record("clipboard", k, &[]);
    }
    // The agent log: the guest's own writer.
    let log = crate::guest::log::FileLog::new(p.agent_log(), log::LevelFilter::Debug);
    for l in &raw {
        log.write_line(l).unwrap();
    }
    for k in &kinds {
        log.write_line(k).unwrap();
    }
}

fn rules(env: &HostEnv, app: &str) -> PathBuf {
    let path = rules_file(env, app);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# the rules of the app\n[hosts]\n\"chatgpt.com\" = \"allow\"\n\"*.openai.com\" = \"allow\"\n\"Telemetry.Example.com\" = \"deny\"\nallow = [\"api.github.com\", \"10.0.0.5\"]\nnote = \"stamp.json is a file, not a host\"\n",
    )
    .unwrap();
    path
}

#[test]
fn planted_secrets_absent() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    sealed_golden(&layout);
    let p = stamped_profile(&layout, "chatgpt-desktop");
    plant(&p);
    // Item 18: the clipboard text never reached any log.
    for which in super::super::logs::Which::ALL {
        let text = std::fs::read_to_string(which.path(&p)).unwrap();
        assert!(!text.contains(CLIP), "{which:?} holds the clipboard text");
        assert!(text.contains(TOKEN), "{which:?} lacks the planted token");
    }
    let out = macvm_section(&e, &Opts::default());
    for leak in [TOKEN, PASSWORD, CLIP] {
        assert!(!out.contains(leak), "{leak} leaked:\n{out}");
    }
    // The section has what a bug report needs.
    assert!(out.starts_with("macvm:\n"), "{out}");
    assert!(out.contains("golden macOS 15.2 (24C101)"), "{out}");
    assert!(
        out.contains("manifest ") && out.contains("manifest.toml:"),
        "{out}"
    );
    assert!(out.contains("profile chatgpt-desktop/default ("), "{out}");
    assert!(
        out.contains("stamp ") && out.contains("\"schema\": 1"),
        "{out}"
    );
    for log in [
        "shim.log",
        "events.jsonl",
        "console.log",
        "agent.log",
        "boundary.jsonl",
    ] {
        assert!(
            out.contains(&format!("{log} (last 200 lines):")),
            "{log}: {out}"
        );
    }
    assert!(
        out.contains("<token>") || out.contains("<redacted>"),
        "{out}"
    );
    assert!(out.contains("<line with a password removed>"), "{out}");
    assert!(out.contains("clipboard change 2 with 1 items"), "{out}");
    // The home path and the user are gone too.
    assert!(
        !out.contains(&t.path().join("home").display().to_string()),
        "{out}"
    );
    assert!(
        !out.lines()
            .any(|l| l.starts_with("missing: ") && l.ends_with(".log")),
        "{out}"
    );
}

#[test]
fn no_hosts_replaces_every_host() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    sealed_golden(&layout);
    let p = stamped_profile(&layout, "chatgpt-desktop");
    plant(&p);
    let rules_path = rules(&e, "chatgpt-desktop");
    std::fs::write(
        p.logs().join(status::CONNECTIONS_FILE),
        "{\"host\":\"chatgpt.com\",\"port\":443,\"verdict\":\"allowed\"}\n{\"host\":\"telemetry.example.com\",\"port\":443,\"verdict\":\"denied\"}\n{\"host\":\"cdn.oaistatic.com\",\"port\":443,\"verdict\":\"allowed\"}\n",
    )
    .unwrap();
    let hosts = hosts_in(&std::fs::read_to_string(&rules_path).unwrap());
    assert_eq!(
        hosts,
        [
            "chatgpt.com",
            "openai.com",
            "telemetry.example.com",
            "api.github.com",
            "10.0.0.5"
        ]
    );
    let with = macvm_section(&e, &Opts::default());
    assert!(
        with.contains("chatgpt.com") && with.contains("Telemetry.Example.com"),
        "{with}"
    );
    assert!(with.contains("connections: 2 allowed, 1 denied"), "{with}");
    let without = macvm_section(
        &e,
        &Opts {
            no_hosts: true,
            ..Opts::default()
        },
    );
    for h in hosts
        .iter()
        .map(String::as_str)
        .chain(["cdn.oaistatic.com", "Telemetry"])
    {
        assert!(
            !without
                .to_ascii_lowercase()
                .contains(&h.to_ascii_lowercase()),
            "{h} is in the output:\n{without}"
        );
    }
    assert!(
        without.starts_with("5 host names replaced with host-N (--no-hosts)\n"),
        "{without}"
    );
    assert!(without.contains("\"host-"), "{without}");
    assert!(
        without.contains("connections: 2 allowed, 1 denied"),
        "{without}"
    );
    // The redaction alone, on a text: the hosts, the token runs, the
    // password lines.
    let r = Redactor::new("/Users/u", "u", "mac", &BTreeMap::new());
    let out = redact(
        "a line with token id x-abcdefabcdefabcdefabcdefabcdefabcdef and chatgpt.com\nPassword hunter2 here\nplain\n",
        &r,
        &["chatgpt.com".to_string()],
        true,
    );
    assert_eq!(
        out,
        "1 host names replaced with host-N (--no-hosts)\na line with token id x-<token> and host-1\n<line with a password removed>\nplain\n"
    );
    assert_eq!(
        hide_hex_runs("x 0123456789abcdef0123456789abcdef1 y"),
        "x <token> y"
    );
    assert_eq!(hide_hex_runs("deadbeef"), "deadbeef");
}

#[test]
fn missing_log_noted() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    // No golden, a profile with a stamp and no logs at all.
    let p = stamped_profile(&layout, "vscode");
    std::fs::remove_dir(p.logs()).unwrap();
    let out = macvm_section(&e, &Opts::default());
    assert!(out.contains("no       golden: none yet"), "{out}");
    assert!(
        out.contains("missing: ") && out.contains("manifest.toml"),
        "{out}"
    );
    for log in [
        "shim.log",
        "events.jsonl",
        "console.log",
        "agent.log",
        "boundary.jsonl",
    ] {
        assert!(
            out.lines()
                .any(|l| l.starts_with("missing: ") && l.ends_with(log)),
            "{log}: {out}"
        );
    }
    assert!(
        out.lines()
            .any(|l| l.starts_with("missing: ") && l.ends_with("vscode.toml")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("missing: ") && l.ends_with("connections.jsonl")),
        "{out}"
    );
    assert!(out.contains("\"schema\": 1"), "{out}");
    // No profile at all: the section still renders.
    let t2 = tempfile::tempdir().unwrap();
    let out = macvm_section(&env(t2.path()), &Opts::default());
    assert!(out.starts_with("macvm:\n"), "{out}");
    assert!(!out.contains("profile "), "{out}");
}
