//! Peek through a real local broker and a temporary fake horse store.

use paddock::broker::{self, Broker, Context, DenyAll, HostPolicy, Request, Status, PROTOCOL};
use paddock::env::HostEnv;
use paddock::horse::{
    device::{peek, Access},
    store::Origin,
    Binding, Store,
};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

fn cli(root: &Path, socket: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .arg("host")
        .arg("peek")
        .args(args)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PADDOCK_DEVICES", root.join("no-device"))
        .env("PADDOCK_HOST_SOCKET", socket)
        .output()
        .unwrap()
}

#[test]
fn peek_uses_the_broker_and_exits_cleanly_when_the_pipe_closes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let env = HostEnv::from_parts(root.join("home"), "test".into(), 0, 0, Default::default());
    let store = Store::open_host(&env).unwrap();
    let horse = store.new_default_horse().unwrap();
    let session = store
        .new_session(
            &horse.id,
            Binding {
                harness: "test".into(),
                native_id: None,
                version: "1".into(),
                container: "test".into(),
            },
        )
        .unwrap();
    let raw = format!("peek-broker-canary-1825{}", "x".repeat(512 * 1024));
    let origin = Origin {
        harness: "test",
        container: "test",
        version: "1",
    };
    let hash = store
        .for_horse(&horse.id)
        .unwrap()
        .put_native(&origin, None, "seq:0", raw.as_bytes())
        .unwrap();
    store
        .update_session(&session.id, |s| s.head = Some(hash))
        .unwrap();
    let access = Access {
        store: store.clone(),
        horse: horse.id,
        session: session.id,
    };
    peek::issue_own(&access, "peek-sb").unwrap();
    let context = Context {
        id: "peek-sb".into(),
        harness: "test".into(),
        project: root.into(),
        repo: None,
        chain: vec![],
        depth: 0,
        role: None,
    };
    let broker = Broker::new(env, context, HostPolicy::default(), Arc::new(DenyAll));
    assert!(broker.horse.set(access).is_ok());
    let legacy = Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: [("op".into(), serde_json::json!("log"))].into(),
        reason: String::new(),
    };
    assert_eq!(broker.handle(&legacy).status, Status::Denied);
    let socket = root.join("broker.sock");
    broker::serve(&socket, Arc::new(broker)).unwrap();
    let output = cli(root, &socket, &["grep", "peek-broker-canary-1825"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.starts_with("# Data from the target session"));
    assert!(output.contains("peek-broker-canary-1825"));

    let chained = Command::new("bash")
        .args([
            "-o",
            "pipefail",
            "-c",
            "\"$1\" host peek ls | \"$1\" host peek cat",
            "peek-test",
            env!("CARGO_BIN_EXE_paddock"),
        ])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PADDOCK_DEVICES", root.join("no-device"))
        .env("PADDOCK_HOST_SOCKET", &socket)
        .output()
        .unwrap();
    assert!(
        chained.status.success(),
        "{}",
        String::from_utf8_lossy(&chained.stderr)
    );
    assert!(chained
        .stdout
        .windows(b"peek-broker-canary-1825".len())
        .any(|w| w == b"peek-broker-canary-1825"));

    let pipeline = Command::new("bash")
        .args([
            "-o",
            "pipefail",
            "-c",
            "\"$1\" host peek cat 0 | head -1",
            "peek-test",
            env!("CARGO_BIN_EXE_paddock"),
        ])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PADDOCK_DEVICES", root.join("no-device"))
        .env("PADDOCK_HOST_SOCKET", &socket)
        .output()
        .unwrap();
    assert!(
        pipeline.status.success(),
        "{}",
        String::from_utf8_lossy(&pipeline.stderr)
    );
    assert!(String::from_utf8_lossy(&pipeline.stdout).starts_with("# Data from the target session"));
    let (used_bytes, reads) = peek::grant::stats(&store, "peek-sb", "own")
        .unwrap()
        .unwrap();
    assert!(reads >= 2);
    assert!(used_bytes > 0);
    peek::revoke(&store, "peek-sb").unwrap();
    assert!(!cli(root, &socket, &["cat", "0"]).status.success());
}
