//! Encryption checks use only a temporary host and fake transcript.

use paddock::horse::dsh::{encode_file, Header};
use std::path::Path;
use std::process::Command;

fn command(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["horse"].into_iter().chain(args.iter().copied()))
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

fn assert_no_canary(root: &Path, canary: &[u8]) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            assert_no_canary(&entry.path(), canary);
        } else {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(!bytes.windows(canary.len()).any(|w| w == canary));
        }
    }
}

#[test]
fn ingest_keeps_session_content_out_of_the_store_files() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let header = Header {
        id: "session-11111111-2222-4333-8444-555555555555".into(),
        created_at: 1_793_000_000_000,
        cwd: Some("/work/demo".into()),
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: 0,
        agent_preset: None,
    };
    let canary = "private-canary-9576382";
    let row = format!(
        r#"{{"type":"user/message","seq":0,"time":1793000000200,"data":{{"content":[{{"type":"text","text":"{canary}"}}],"source":{{"kind":"user"}},"role":"user","id":"m1"}},"surfaceOp":"append"}}"#
    );
    let input = t.join("session.v4.jsonl.zstd");
    std::fs::write(
        &input,
        encode_file(&header.encode(), &[row.as_bytes()]).unwrap(),
    )
    .unwrap();
    let out = command(
        t,
        &[
            "ingest-dsh",
            input.to_str().unwrap(),
            "--dsh-version",
            "0.2.1-rc.5",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let session = text.split(':').next().unwrap();
    let log = command(t, &["log", session]);
    assert!(
        log.status.success(),
        "{}",
        String::from_utf8_lossy(&log.stderr)
    );
    assert!(String::from_utf8_lossy(&log.stdout).contains(canary));
    assert_no_canary(&t.join("data/paddock/horse"), canary.as_bytes());
}
