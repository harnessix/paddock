//! Cache backend limits in explain, with a fake harness and temporary XDG paths.
use std::process::Command;

#[test]
fn explain_reports_backends_that_cannot_remap_caches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let config = root.join("config/paddock/harness.d");
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        config.join("fake.toml"),
        "name = 'fake'\n[locate]\ncommands = ['sh']\n",
    )
    .unwrap();
    for backend in ["srt", "seatbelt"] {
        for json in [false, true] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_paddock"));
            cmd.args(["explain", "fake"])
                .arg(&project)
                .args(["--backend", backend, "--network", "none", "--env", "clean"])
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("XDG_STATE_HOME", root.join("state"))
                .env("XDG_RUNTIME_DIR", root.join("runtime"));
            if json {
                cmd.arg("--json");
            }
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(
                text.contains("sandbox home") && text.contains("[cache] share does not apply"),
                "{text}"
            );
            if json {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert!(value["cache_backend_note"].as_str().is_some());
            }
        }
    }
}
