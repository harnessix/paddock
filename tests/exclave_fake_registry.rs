//! Local-only sparse registry acceptance fixture. Run on a host with bwrap and systemd.
#![cfg(all(target_os = "linux", feature = "exclave-fake-registry-test"))]

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[test]
fn local_registry_is_scoped_and_second_build_is_warm() {
    if !Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        || !Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    {
        eprintln!("skipped: requires host bwrap and systemd user manager");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let package = root.join("archive/local-dep-1.0.0");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname='local-dep'\nversion='1.0.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(package.join("src/lib.rs"), "pub fn value() -> u32 { 42 }\n").unwrap();
    let archive = root.join("local-dep.crate");
    assert!(Command::new("tar")
        .args(["-czf"])
        .arg(&archive)
        .arg("-C")
        .arg(root.join("archive"))
        .arg("local-dep-1.0.0")
        .status()
        .unwrap()
        .success());
    let bytes = std::fs::read(archive).unwrap();
    let checksum = format!("{:x}", Sha256::digest(&bytes));
    let wheel = root.join("paddock_fixture-1.0-py3-none-any.whl");
    let create_wheel = r#"import sys, zipfile
with zipfile.ZipFile(sys.argv[1], 'w', zipfile.ZIP_DEFLATED) as z:
    z.writestr('paddock_fixture.py', 'VALUE = 42\n')
    z.writestr('paddock_fixture-1.0.dist-info/METADATA', 'Metadata-Version: 2.1\nName: paddock-fixture\nVersion: 1.0\n')
    z.writestr('paddock_fixture-1.0.dist-info/WHEEL', 'Wheel-Version: 1.0\nGenerator: fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n')
    z.writestr('paddock_fixture-1.0.dist-info/RECORD', '')
"#;
    assert!(Command::new("python3")
        .args(["-c", create_wheel])
        .arg(&wheel)
        .status()
        .unwrap()
        .success());
    let wheel_bytes = std::fs::read(wheel).unwrap();
    let listener = TcpListener::bind("127.0.0.1:38473").expect("fixture port 38473 must be free");
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = requests.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let server = std::thread::spawn(move || {
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut buf = Vec::new();
            let mut part = [0; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut part) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&part[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf);
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            seen.lock().unwrap().push(path.clone());
            let body = match path.as_str() {
                "/simple/paddock-fixture/" => Some(b"<html><a href=\"http://localhost:38473/files/paddock_fixture-1.0-py3-none-any.whl\">paddock_fixture-1.0-py3-none-any.whl</a></html>".to_vec()),
                "/files/paddock_fixture-1.0-py3-none-any.whl" => Some(wheel_bytes.clone()),
                "/config.json" => Some(b"{\"dl\":\"http://localhost:38473/crates\"}".to_vec()),
                "/lo/ca/local-dep" => Some(format!("{{\"name\":\"local-dep\",\"vers\":\"1.0.0\",\"deps\":[],\"cksum\":\"{checksum}\",\"features\":{{}},\"yanked\":false}}\n").into_bytes()),
                "/crates/local-dep/1.0.0/download" => Some(bytes.clone()),
                _ => None,
            };
            let (status, body) = body.map_or(("404 Not Found", Vec::new()), |b| ("200 OK", b));
            let content_type = if path.starts_with("/simple/") {
                "text/html"
            } else {
                "application/octet-stream"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    let project = root.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join(".cargo")).unwrap();
    std::fs::write(project.join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n[dependencies]\nlocal-dep='1.0.0'\n").unwrap();
    std::fs::write(
        project.join("src/lib.rs"),
        "#[test] fn dependency() { assert_eq!(local_dep::value(), 42); }\n",
    )
    .unwrap();
    std::fs::write(project.join(".cargo/config.toml"), "[source.crates-io]\nreplace-with='fake'\n[source.fake]\nregistry='sparse+http://localhost:38473/'\n[http]\nproxy='http://127.0.0.1:3131'\n").unwrap();
    assert!(Command::new("git")
        .current_dir(&project)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .current_dir(&project)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .args(["add", "."])
        .status()
        .unwrap()
        .success());
    let peer = root.join("peer");
    std::fs::create_dir_all(peer.join("config/paddock")).unwrap();
    std::fs::write(
        peer.join("config/paddock/config.toml"),
        "[exclave.serve]\nallow_shared_disk = true\nhttp_registries=['localhost:38473']\n[exclave.serve.env]\nPIP_INDEX_URL='http://localhost:38473/simple'\nUV_INDEX_URL='http://localhost:38473/simple'\nPIP_TRUSTED_HOST='localhost'\n",
    )
    .unwrap();
    let script = root.join("transport.sh");
    std::fs::write(&script, format!("#!/bin/sh\nexport HOME='{0}' XDG_CONFIG_HOME='{0}/config' XDG_DATA_HOME='{0}/data' XDG_STATE_HOME='{0}/state' XDG_CACHE_HOME='{0}/cache'\nexec '{1}' exclave serve\n", peer.display(), env!("CARGO_BIN_EXE_paddock"))).unwrap();
    let config = root.join("config/paddock");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "[exclave.peers.fake]\ntransport=['/bin/sh', '{}']\n",
            script.display()
        ),
    )
    .unwrap();
    let run = |command: &str| {
        Command::new(env!("CARGO_BIN_EXE_paddock"))
            .current_dir(&project)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .args([
                "exclave",
                "run",
                "--on",
                "fake",
                "--network",
                "registries",
                "--",
                "/bin/sh",
                "-c",
                command,
            ])
            .output()
            .unwrap()
    };
    let cold = run("CARGO_BUILD_JOBS=2 cargo test -v");
    assert!(
        cold.status.success(),
        "cold: {}",
        String::from_utf8_lossy(&cold.stderr)
    );
    let paths = requests.lock().unwrap().clone();
    assert!(paths.iter().any(|p| p == "/lo/ca/local-dep"), "{paths:?}");
    assert!(
        paths
            .iter()
            .any(|p| p == "/crates/local-dep/1.0.0/download"),
        "{paths:?}"
    );
    let denied = run(
        r#"test "$(curl --noproxy '' --max-time 3 -sS -o /dev/null -w '%{http_code}' http://localhost:38474/)" = 403"#,
    );
    assert!(
        denied.status.success(),
        "unrelated target was not denied: {}",
        String::from_utf8_lossy(&denied.stderr)
    );
    let warm = run("CARGO_BUILD_JOBS=2 cargo test -v --offline");
    assert!(
        warm.status.success(),
        "warm: {}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stderr).contains("Fresh local-dep"),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert_eq!(
        *requests.lock().unwrap(),
        paths,
        "warm build contacted registry"
    );
    let pip_cold = run("python3 -m venv /work/.venv && /work/.venv/bin/pip install paddock-fixture==1.0 && /work/.venv/bin/python -c 'import paddock_fixture; assert paddock_fixture.VALUE == 42'");
    assert!(
        pip_cold.status.success(),
        "pip cold: {}",
        String::from_utf8_lossy(&pip_cold.stderr)
    );
    let pip_requests = requests.lock().unwrap().len();
    let pip_warm = run("/work/.venv/bin/pip install --no-index paddock-fixture==1.0 && /work/.venv/bin/python -c 'import paddock_fixture'");
    assert!(
        pip_warm.status.success(),
        "pip warm: {}",
        String::from_utf8_lossy(&pip_warm.stderr)
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        pip_requests,
        "warm pip contacted the index"
    );
    let uv_cold = run("/work/.venv/bin/pip uninstall -y paddock-fixture && uv pip install --python /work/.venv/bin/python paddock-fixture==1.0 && /work/.venv/bin/python -c 'import paddock_fixture; assert paddock_fixture.VALUE == 42'");
    assert!(
        uv_cold.status.success(),
        "uv cold: {}",
        String::from_utf8_lossy(&uv_cold.stderr)
    );
    let uv_requests = requests.lock().unwrap().len();
    let uv_warm = run("uv pip install --offline --python /work/.venv/bin/python paddock-fixture==1.0 && /work/.venv/bin/python -c 'import paddock_fixture'");
    assert!(
        uv_warm.status.success(),
        "uv warm: {}",
        String::from_utf8_lossy(&uv_warm.stderr)
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        uv_requests,
        "warm uv contacted the index"
    );
    let paths = requests.lock().unwrap().clone();
    assert!(
        paths
            .iter()
            .filter(|p| p.as_str() == "/simple/paddock-fixture/")
            .count()
            >= 2,
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .filter(|p| p.as_str() == "/files/paddock_fixture-1.0-py3-none-any.whl")
            .count()
            >= 2,
        "{paths:?}"
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    server.join().unwrap();
}
