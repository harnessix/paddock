//! A real CLI migration of a handcrafted, temporary legacy plaintext store.

use paddock::horse::{
    store::{Hash, Meta},
    Binding, Horse, LineageStep, Session,
};
use std::path::Path;
use std::process::{Command, Output};

fn command(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .arg("horse")
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root.join("home"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .unwrap()
}

fn object(root: &Path, meta: &Meta, raw: &[u8]) -> Hash {
    let mut bytes = serde_json::to_vec(meta).unwrap();
    bytes.push(b'\n');
    bytes.extend_from_slice(raw);
    let hash = Hash::of(&bytes);
    let dir = root.join("objects").join(&hash.as_str()[..2]);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(&hash.as_str()[2..]), bytes).unwrap();
    hash
}

fn assert_no_canary(path: &Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            assert_no_canary(&entry.path());
        } else {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(!bytes
                .windows(b"migration-cli-canary".len())
                .any(|w| w == b"migration-cli-canary"));
        }
    }
}

#[test]
fn migrate_cli_preserves_the_plain_copy_until_the_person_drops_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let store = root.join("data/paddock/horse");
    for dir in ["objects", "horses", "sessions", "tmp"] {
        std::fs::create_dir_all(store.join(dir)).unwrap();
    }
    let dna = paddock::horse::dna::default_for_run().encode();
    let dna_hash = object(
        &store,
        &Meta {
            kind: "blob".into(),
            harness: None,
            container: None,
            version: None,
            parent: None,
            key: None,
            media: Some(paddock::horse::dna::MEDIA.into()),
        },
        dna.as_bytes(),
    );
    let raw = br#"{"type":"user/message","seq":0,"time":123,"data":{"content":[{"type":"text","text":"migration-cli-canary"}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}"#;
    let head = object(
        &store,
        &Meta {
            kind: "native".into(),
            harness: Some("dsh".into()),
            container: Some(paddock::horse::dsh::CONTAINER.into()),
            version: Some(paddock::horse::dsh::VERSION.into()),
            parent: None,
            key: Some("seq:0".into()),
            media: None,
        },
        raw,
    );
    let horse = Horse {
        id: "h-test".into(),
        dna: dna_hash.clone(),
        sessions: vec!["s-test".into()],
        created_ms: 1,
    };
    let session = Session {
        id: "s-test".into(),
        horse: horse.id.clone(),
        head: Some(head),
        binding: Binding {
            harness: "dsh".into(),
            native_id: None,
            version: paddock::horse::dsh::VERSION.into(),
            container: paddock::horse::dsh::CONTAINER.into(),
        },
        codex_reader_version: None,
        native_header: None,
        fork_of: None,
        lineage: vec![LineageStep {
            horse: horse.id.clone(),
            dna: dna_hash,
            since_ms: 1,
            until_ms: None,
        }],
        created_ms: 1,
    };
    std::fs::write(
        store.join("horses/h-test.json"),
        serde_json::to_vec(&horse).unwrap(),
    )
    .unwrap();
    std::fs::write(
        store.join("sessions/s-test.json"),
        serde_json::to_vec(&session).unwrap(),
    )
    .unwrap();

    let output = command(root, &["migrate", "--verify"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let backup = root.join("data/paddock/horse.plain");
    assert!(backup.is_dir());
    assert_no_canary(&store);
    let log = command(root, &["log", "s-test"]);
    assert!(
        log.status.success(),
        "{}",
        String::from_utf8_lossy(&log.stderr)
    );
    assert!(String::from_utf8_lossy(&log.stdout).contains("migration-cli-canary"));
    let output = command(root, &["migrate", "--drop-plain"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!backup.exists());
    assert!(command(root, &["log", "s-test"]).status.success());
}
