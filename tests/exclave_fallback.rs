//! Host CLI fallback decisions use only local fake transports and temporary state.
use std::process::Command;

#[test]
fn host_reports_peer_error_and_only_falls_back_for_transport_failure() {
    let temp = paddock::view::guard::tempdir();
    let project = temp.path().join("project");
    let config = temp.path().join("config/paddock");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let peer = temp.path().join("peer.sh");
    std::fs::write(&peer, "read hello\nprintf '%s\\n' '{\"status\":\"done\",\"result\":{\"type\":\"error\",\"message\":\"No space left on device\"}}'\n").unwrap();
    let config_file = config.join("config.toml");
    std::fs::write(
        &config_file,
        format!(
            "[exclave.peers.fake]\ntransport = ['/bin/sh', '{}']\n",
            peer.display()
        ),
    )
    .unwrap();
    let marker = project.join("local-ran");
    let run = |gpu: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_paddock"));
        command
            .current_dir(&project)
            .env("HOME", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .env("XDG_RUNTIME_DIR", temp.path().join("run"))
            .args(["exclave", "run", "--on", "fake"]);
        if gpu {
            command.arg("--gpu");
        }
        command
            .args(["--", "/usr/bin/touch"])
            .arg(&marker)
            .output()
            .unwrap()
    };
    let output = run(false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("No space left on device"));
    assert!(!marker.exists());
    std::fs::write(
        &config_file,
        "[exclave.peers.fake]\ntransport = ['/no-such-transport']\n",
    )
    .unwrap();
    let output = run(true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not start exclave transport"));
    assert!(!marker.exists());
    let output = run(false);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not start exclave transport"));
    assert!(marker.exists());
}
