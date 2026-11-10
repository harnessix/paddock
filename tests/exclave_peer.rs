//! Peer isolation requires a host with bwrap and a systemd user manager.
#[cfg(target_os = "linux")]
#[test]
fn peer_runs_in_a_limited_sandbox() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use std::io::Write;
    use std::process::{Command, Stdio};
    let bwrap = Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let scope = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !bwrap || !scope {
        eprintln!("skipped: peer test requires bwrap and a systemd user manager");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = "a".repeat(64);
    let content = b"original";
    let blob = blake3::hash(content).to_hex().to_string();
    let mut manifest = paddock::exclave::snapshot::Manifest::default();
    manifest.files.insert(
        "sample".into(),
        paddock::exclave::snapshot::Entry {
            hash: blob.clone(),
            size: content.len() as u64,
            executable: false,
            modified_ns: 0,
        },
    );
    let snapshot = manifest.hash().unwrap();
    std::fs::create_dir_all(dir.path().join("config/paddock")).unwrap();
    std::fs::write(
        dir.path().join("config/paddock/config.toml"),
        "[exclave.serve]\nallow_shared_disk = true\nseconds = 3\nmemory_bytes = 67108864\n",
    )
    .unwrap();
    let secret = dir.path().join("outside");
    std::fs::write(&secret, "canary").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_paddock"));
    cmd.args(["exclave", "serve"])
        .env("HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut child = cmd.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"v":1, "cap":"exclave", "args":{"type":"hello", "version":1, "project":project}})
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"v":1,"cap":"exclave","args":{"type":"manifest","manifest":manifest}})
    )
    .unwrap();
    writeln!(input, "{}", serde_json::json!({"v":1,"cap":"exclave","args":{"type":"blob_chunk","hash":blob,"offset":0,"data":STANDARD.encode(content),"final_chunk":true}})).unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"v":1,"cap":"exclave","args":{"type":"reserve"}})
    )
    .unwrap();
    writeln!(input, "{}", serde_json::json!({"v":1, "cap":"exclave", "args":{"type":"run", "snapshot":snapshot, "command":["/bin/sh", "-c", "test ! -e \"$1\" && printf changed > sample && printf isolated", "test", secret], "outputs":[]}})).unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"v":1,"cap":"exclave","args":{"type":"reserve"}})
    )
    .unwrap();
    writeln!(input, "{}", serde_json::json!({"v":1, "cap":"exclave", "args":{"type":"run", "snapshot":snapshot, "network":"none", "command":["/bin/sh", "-c", "sleep 8"], "outputs":[]}})).unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"v":1,"cap":"exclave","args":{"type":"reserve"}})
    )
    .unwrap();
    writeln!(input, "{}", serde_json::json!({"v":1, "cap":"exclave", "args":{"type":"run", "snapshot":snapshot, "command":["/bin/sh", "-c", "test \"$(cat sample)\" = original || exit 92; command -v curl >/dev/null || exit 91; curl --noproxy '' --max-time 1 -fsS \"$1\" >/dev/null 2>&1 && exit 90 || printf blocked", "test", format!("http://127.0.0.1:{port}")], "outputs":[]}})).unwrap();
    writeln!(
        input,
        "{}",
        serde_json::json!({"v":1,"cap":"exclave","args":{"type":"reserve"}})
    )
    .unwrap();
    writeln!(input, "{}", serde_json::json!({"v":1,"cap":"exclave","args":{"type":"run","snapshot":snapshot,"network":"none","command":["/usr/bin/python3","-c","print('alloc', flush=True); a=bytearray(512*1024*1024)"],"outputs":[]}})).unwrap();
    drop(input);
    let started = std::time::Instant::now();
    let out = child.wait_with_output().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(7),
        "the peer runtime limit did not stop the command"
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            let envelope: serde_json::Value = serde_json::from_str(l).unwrap();
            envelope["result"].clone()
        })
        .collect();
    let diagnostic = lines
        .iter()
        .filter(|v| v["type"] == "output" && v["stream"] == "stderr")
        .filter_map(|v| v["data"].as_str())
        .filter_map(|d| STANDARD.decode(d).ok())
        .flatten()
        .collect::<Vec<_>>();
    assert!(
        lines.iter().any(|v| v["type"] == "exit" && v["code"] == 0),
        "peer stderr: {} ({lines:?})",
        String::from_utf8_lossy(&diagnostic)
    );
    let exits: Vec<i64> = lines
        .iter()
        .filter(|v| v["type"] == "exit")
        .filter_map(|v| v["code"].as_i64())
        .collect();
    assert_eq!(
        exits.len(),
        4,
        "peer stderr: {} ({lines:?})",
        String::from_utf8_lossy(&diagnostic)
    );
    assert_eq!(
        exits[2], 0,
        "registry mode reached the peer loopback or curl could not run"
    );
    assert_ne!(exits[3], 0, "memory-limited command unexpectedly succeeded");
    assert!(
        lines.iter().any(|v| v["type"] == "output"
            && v["stream"] == "stdout"
            && STANDARD.decode(v["data"].as_str().unwrap()).unwrap() == b"alloc\n"),
        "memory test did not start: {lines:?}"
    );
    assert_ne!(
        exits[1], 0,
        "runtime-limited command unexpectedly succeeded"
    );
    assert!(
        lines.iter().any(|v| v["type"] == "output"
            && v["stream"] == "stdout"
            && STANDARD.decode(v["data"].as_str().unwrap()).unwrap() == b"isolated"),
        "{lines:?}"
    );
}
