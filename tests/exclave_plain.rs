//! A fake stdio peer exercises plain-folder selection and output transfer.
use std::process::Command;

#[test]
fn plain_folder_transfers_selected_files_and_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let config = temp.path().join("config/paddock");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(project.join("source"), "hello").unwrap();
    std::fs::write(project.join(".gitignore"), "ignored\n").unwrap();
    std::fs::write(project.join(".paddockignore"), "private\n").unwrap();
    std::fs::write(project.join("ignored"), "no").unwrap();
    std::fs::write(project.join("private"), "no").unwrap();
    std::fs::write(project.join(".env"), "no").unwrap();
    std::os::unix::fs::symlink("source", project.join("link")).unwrap();
    let peer = temp.path().join("peer.py");
    let log = temp.path().join("manifest.json");
    let content = b"hello";
    let hash = blake3::hash(content).to_hex().to_string();
    std::fs::write(&peer, format!(r#"import json, sys

def read(): return json.loads(sys.stdin.readline())["args"]
def send(x): print(json.dumps({{"status":"done","result":x}}), flush=True)
read(); send({{"type":"ready","version":1}})
m = read()["manifest"]
open({log:?}, "w").write(json.dumps(m))
send({{"type":"missing","hashes":[]}})
read(); send({{"type":"accepted"}})
r = read()
assert r["outputs"] == ["result"] and r["network"] == "none"
send({{"type":"output_manifest","manifest":{{"files":{{"result":{{"hash":"{hash}","size":5,"executable":False}}}}}}}})
send({{"type":"output_chunk","hash":"{hash}","offset":0,"data":"aGVsbG8=","final_chunk":True}})
send({{"type":"exit","code":0}})
"#, log=log.to_str().unwrap(), hash=hash)).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "[exclave.peers.fake]\ntransport = ['/usr/bin/python3', '{}']\n",
            peer.display()
        ),
    )
    .unwrap();
    let run = |root: bool, git_path: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_paddock"));
        command
            .current_dir(&project)
            .env("HOME", temp.path())
            .env(
                "PATH",
                if git_path {
                    std::env::var("PATH").unwrap()
                } else {
                    String::new()
                },
            )
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .env("XDG_RUNTIME_DIR", temp.path().join("run"))
            .args(["exclave", "run"]);
        if root {
            command.arg("--root").arg(&project);
        }
        command
            .args([
                "--on",
                "fake",
                "--network",
                "none",
                "--include",
                "source",
                "--output",
                "result",
                "--",
                "/bin/true",
            ])
            .output()
            .unwrap()
    };
    let out = run(false, false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&log).unwrap()).unwrap();
    let files = manifest["files"].as_object().unwrap();
    assert!(files.contains_key("source"));
    for name in ["ignored", "private", ".env"] {
        assert!(!files.contains_key(name), "{name}");
    }
    assert_eq!(manifest["links"]["link"], "source");
    assert_eq!(std::fs::read(project.join("result")).unwrap(), content);
    assert!(Command::new("git")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(temp.path().join(".gitignore"), "project/source\n").unwrap();
    let rooted = run(true, false);
    assert!(
        rooted.status.success(),
        "{}",
        String::from_utf8_lossy(&rooted.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&log).unwrap()).unwrap();
    assert!(manifest["files"].get("source").is_some());
    assert!(String::from_utf8_lossy(&rooted.stderr).contains("explicit --root"));
    std::fs::write(temp.path().join(".gitignore"), "").unwrap();
    assert!(Command::new("git")
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["add", "project/source"])
        .status()
        .unwrap()
        .success());
    let git_run = run(false, true);
    assert!(
        git_run.status.success(),
        "{}",
        String::from_utf8_lossy(&git_run.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&log).unwrap()).unwrap();
    assert!(manifest["files"].get("project/source").is_some());
    std::fs::remove_file(project.join("link")).unwrap();
    std::os::unix::fs::symlink("../outside", project.join("link")).unwrap();
    let out = run(true, false);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("link"));
}
