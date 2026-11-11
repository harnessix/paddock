//! Every test that runs the built paddock gives it temporary folders. A
//! build that runs with the user's own folders can rewrite the user's
//! launchers, shell commands and state with the path of the build, and
//! they break when the build is gone.
//!
//! A test file passes when it isolates home, config, data, state, cache and runtime, or
//! clears the environment and sets HOME. This is a source-level guard, not
//! a proof about every command in a file.

#[test]
fn tests_that_run_paddock_never_use_the_users_folders() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let mut bad = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        if !text.contains("CARGO_BIN_EXE_paddock") {
            continue;
        }
        if !isolated(&text) {
            bad.push(p.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(
        bad.is_empty(),
        "these tests run paddock with the user's own folders: {bad:?}"
    );
}

fn isolated(text: &str) -> bool {
    let home_var = "HOME";
    let home = text.contains(&format!("\"{home_var}\""));
    let xdg = [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ]
    .iter()
    .all(|key| text.contains(key));
    let cleared = text.contains(".env_clear()");
    home && (xdg || cleared)
}

#[test]
fn guard_requires_cache_and_runtime_as_well_as_data_and_state() {
    let vars = [
        "\"HOME\"",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ];
    assert!(isolated(&vars.join(" ")));
    for missing in 0..vars.len() {
        let text = vars
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != missing)
            .map(|(_, s)| *s)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!isolated(&text), "accepted without {}", vars[missing]);
    }
    assert!(isolated(".env_clear() \"HOME\""));
    assert!(!isolated(".env_clear()"));
}
