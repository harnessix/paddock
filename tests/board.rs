//! `paddock board` end to end: the built binary on a temporary home with
//! one fixture run and one fixture ticket. The state rules and the exact
//! output have their golden tests in src/board/tests.rs.

use std::path::Path;
use std::process::{Command, Stdio};

/// paddock with temporary folders only, never the user's own.
fn paddock(temp: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.env_clear()
        .env("HOME", temp.join("home"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("XDG_CONFIG_HOME", temp.join("config"))
        .env("XDG_CACHE_HOME", temp.join("cache"))
        .env("XDG_DATA_HOME", temp.join("data"))
        .env("XDG_STATE_HOME", temp.join("state"))
        .env("XDG_RUNTIME_DIR", temp.join("run"));
    c
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A run of this test process (so it counts as alive) on ticket KV2, and a
/// ticket AUTH1 without a run.
fn fixture(temp: &Path) {
    let project = temp.join("src/alpha");
    write(
        &project.join(".paddock/tickets/KV2-worker.md"),
        "# KV2: named kv namespaces\n",
    );
    write(
        &project.join(".paddock/tickets/AUTH1-login.md"),
        "# AUTH1: login refresh\nStatus: open.\n",
    );
    let id = "claude-alpha-71c3ac";
    let info = serde_json::json!({
        "id": id, "harness": "claude", "project": project, "backend": "bwrap",
        "network": "egress", "gui": "none", "pid": std::process::id(), "started": 1,
    });
    write(
        &temp.join(format!("run/paddock/{id}/info.json")),
        &info.to_string(),
    );
    write(
        &temp.join(format!("state/paddock/board/{id}.json")),
        r#"{"file": ".paddock/tickets/KV2-worker.md", "sha256": ""}"#,
    );
}

#[test]
fn json_maps_the_run_to_its_ticket() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let out = paddock(temp.path())
        .args(["board", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(v["approvals_waiting"], 0);
    let p = &v["projects"][0];
    assert_eq!(p["name"], "alpha");
    let ids: Vec<&str> = p["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["KV2", "AUTH1"]);
    let run = &p["tickets"][0]["runs"][0];
    assert_eq!(run["id"], "71c3ac");
    assert_eq!(run["state"], "working");
    assert_eq!(run["card"], false);
    assert_eq!(p["tickets"][1]["state"], "open");
    assert_eq!(p["runs_without_ticket"], serde_json::json!([]));
}

#[test]
fn text_is_cut_to_columns_and_has_no_color_in_a_pipe() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let out = paddock(temp.path())
        .arg("board")
        .env("COLUMNS", "40")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("● working  KV2"), "{text}");
    assert!(text.contains("◇ open     AUTH1"), "{text}");
    assert!(!text.contains('\x1b'), "{text}");
    for l in text.lines() {
        assert!(l.chars().count() <= 40, "{l:?}");
    }
}

#[test]
fn a_ticket_file_name_cannot_send_escapes_to_the_terminal() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    write(
        &temp
            .path()
            .join("src/alpha/.paddock/tickets/X\x1b]52;c;aGk=\x07\x1b[2J-a.md"),
        "# a ticket\nStatus: open.\n",
    );
    for args in [&["board"][..], &["board", "--json"]] {
        let out = paddock(temp.path()).args(args).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("a ticket"), "{text}");
        assert!(!text.contains(['\x1b', '\x07']), "{args:?}: {text:?}");
    }
}

#[test]
fn a_ticket_file_name_cannot_send_an_8_bit_csi_or_bidi_characters_to_the_terminal() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let name = "X\u{9b}2J\u{202E}\u{200B}-a.md";
    write(
        &temp.path().join("src/alpha/.paddock/tickets").join(name),
        "# a \u{2066}ticket\nStatus: op\u{200F}en.\n",
    );
    let unsafe_chars = ['\u{9b}', '\u{202E}', '\u{200B}', '\u{2066}', '\u{200F}'];
    for args in [&["board"][..], &["board", "--json"]] {
        let out = paddock(temp.path()).args(args).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("a ticket"), "{args:?}: {text:?}");
        assert!(!text.contains(unsafe_chars), "{args:?}: {text:?}");
    }
    // The one-line JSON of --watch too.
    let mut child = paddock(temp.path())
        .args(["board", "--json", "--watch"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut r = std::io::BufReader::new(child.stdout.take().unwrap());
    let line = line_within(&mut r, 30);
    let _ = child.kill();
    let _ = child.wait();
    let line = line.expect("--watch printed no line within 30 s");
    assert!(line.contains("a ticket"), "{line:?}");
    assert!(!line.contains(unsafe_chars), "{line:?}");
    // The JSON value of `file` is still the exact name, so the panel can
    // open the file.
    let out = paddock(temp.path())
        .args(["board", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let board: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files: Vec<&str> = board["projects"][0]["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["file"].as_str())
        .collect();
    assert!(
        files.contains(&format!(".paddock/tickets/{name}").as_str()),
        "{files:?}"
    );
}

#[test]
fn an_empty_home_says_so() {
    let temp = tempfile::tempdir().unwrap();
    let out = paddock(temp.path()).arg("board").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "No runs on the board.\n"
    );
}

/// The next line of a pipe, or `None` after `secs` seconds without one.
fn line_within(r: &mut std::io::BufReader<std::process::ChildStdout>, secs: u64) -> Option<String> {
    use std::io::BufRead;
    use std::os::fd::AsRawFd;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut line = String::new();
    while !line.ends_with('\n') {
        if !r.buffer().contains(&b'\n') {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let mut fd = libc::pollfd {
                fd: r.get_ref().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for the length 1.
            let n = unsafe { libc::poll(&mut fd, 1, left.as_millis().max(1) as libc::c_int) };
            if n <= 0 && left.is_zero() {
                return None;
            }
            if n <= 0 {
                continue;
            }
        }
        // The pipe has data: read what there is, up to a line end.
        let buf = r.fill_buf().ok()?;
        if buf.is_empty() {
            return None;
        }
        let take = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        line.push_str(&String::from_utf8_lossy(&buf[..take]));
        r.consume(take);
    }
    line.pop();
    Some(line)
}

#[test]
fn watch_json_prints_one_line_per_change_and_ends_with_its_reader() {
    use std::io::BufReader;
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let mut child = paddock(temp.path())
        .args(["board", "--watch", "--json"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap());
    let mut next = |what: &str| match line_within(&mut lines, 30) {
        Some(l) => l,
        None => {
            let _ = child.kill();
            panic!("no {what} line from the watch in 30 s");
        }
    };
    let first = next("first");
    let v: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(v["projects"][0]["tickets"][1]["state"], "open");
    // A new title is a change: a second line follows.
    write(
        &temp
            .path()
            .join("src/alpha/.paddock/tickets/AUTH1-login.md"),
        "# AUTH1: login refresh, new title\nStatus: open.\n",
    );
    let second = next("second");
    assert!(second.contains("new title"), "{second}");
    // The reader goes away: the watch ends by itself.
    drop(lines);
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(start.elapsed().as_secs() < 20, "the watch did not end");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// A running run (this test process) in the project `src/NAME`.
fn run_in(temp: &Path, name: &str) -> std::path::PathBuf {
    let project = temp.join("src").join(name);
    std::fs::create_dir_all(&project).unwrap();
    let hash = name
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b.into()));
    let id = format!("claude-{name}-{:06x}", hash & 0xff_ffff);
    let info = serde_json::json!({
        "id": id, "harness": "claude", "project": project, "backend": "bwrap",
        "network": "egress", "gui": "none", "pid": std::process::id(), "started": 1,
    });
    write(
        &temp.join(format!("run/paddock/{id}/info.json")),
        &info.to_string(),
    );
    project
}

/// A project whose `.paddock` links to the folder `target`.
fn linked(temp: &Path, name: &str, target: &Path) -> std::path::PathBuf {
    let project = run_in(temp, name);
    std::os::unix::fs::symlink(target, project.join(".paddock")).unwrap();
    project
}

/// `paddock board --json` with the data folder `data`, its JSON and its
/// stderr. The run succeeds.
fn board_json(temp: &Path, data: &Path) -> (serde_json::Value, String) {
    let out = paddock(temp)
        .env("XDG_DATA_HOME", data)
        .args(["board", "--json"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{err}");
    (serde_json::from_slice(&out.stdout).unwrap(), err)
}

fn project<'a>(v: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no project {name}: {v}"))
}

fn loft_field(v: &serde_json::Value, name: &str) -> Option<String> {
    project(v, name)
        .get("loft")
        .map(|l| l.as_str().expect("loft is a string").to_string())
}

/// The loft's notes folder of `project` in the data folder `data`, made:
/// `<data>/paddock/loft/<key>/tree/.paddock`, as `Loft::notes()` has it.
fn loft_of(data: &Path, project: &Path) -> std::path::PathBuf {
    let key = paddock::kv::shard::project_hash(&std::fs::canonicalize(project).unwrap());
    let d = data.join("paddock/loft").join(key).join("tree/.paddock");
    std::fs::create_dir_all(d.join("tickets")).unwrap();
    std::fs::canonicalize(d).unwrap()
}

#[test]
fn a_loft_with_a_link_gives_the_loft_notes_folder() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    // `alpha` links to its loft.
    let alpha = run_in(temp.path(), "alpha");
    let alpha_loft = loft_of(&data, &alpha);
    std::os::unix::fs::symlink(&alpha_loft, alpha.join(".paddock")).unwrap();
    // `beta` links to some other folder: the field is still its loft.
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let beta = linked(temp.path(), "beta", &elsewhere);
    let beta_loft = loft_of(&data, &beta);
    let (v, _) = board_json(temp.path(), &data);
    assert_eq!(
        loft_field(&v, "alpha"),
        Some(alpha_loft.display().to_string()),
        "{v}"
    );
    assert_eq!(
        loft_field(&v, "beta"),
        Some(beta_loft.display().to_string()),
        "{v}"
    );
    assert!(Path::new(&loft_field(&v, "alpha").unwrap()).is_absolute());
}

#[test]
fn a_loft_without_a_link_gives_the_loft_notes_folder() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    // The loft-only case: `alpha` has its own `.paddock` folder, `beta` has
    // none, and a ticket is only in the loft.
    let alpha = run_in(temp.path(), "alpha");
    write(&alpha.join(".paddock/tickets/AUTH1-login.md"), "# AUTH1\n");
    let alpha_loft = loft_of(&data, &alpha);
    write(
        &alpha_loft.join("tickets/KV2-worker.md"),
        "# KV2\nStatus: open.\n",
    );
    let beta = run_in(temp.path(), "beta");
    let beta_loft = loft_of(&data, &beta);
    let (v, _) = board_json(temp.path(), &data);
    assert_eq!(
        loft_field(&v, "alpha"),
        Some(alpha_loft.display().to_string()),
        "{v}"
    );
    assert_eq!(
        loft_field(&v, "beta"),
        Some(beta_loft.display().to_string()),
        "{v}"
    );
    let file = Path::new(&loft_field(&v, "alpha").unwrap()).join("tickets/KV2-worker.md");
    assert!(file.is_file(), "{}", file.display());
}

#[test]
fn a_project_without_a_loft_folder_has_no_loft_field() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    // `alpha` links to a folder that is not its loft, `beta` has its own
    // `.paddock` folder, `gamma` has nothing. No loft folder for any.
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    linked(temp.path(), "alpha", &elsewhere);
    let beta = run_in(temp.path(), "beta");
    write(&beta.join(".paddock/tickets/KV2-worker.md"), "# KV2\n");
    run_in(temp.path(), "gamma");
    // A file where the loft folder would be is no folder.
    let delta = run_in(temp.path(), "delta");
    let delta_loft = loft_of(&data, &delta);
    std::fs::remove_dir_all(&delta_loft).unwrap();
    write(&delta_loft, "x");
    let (v, err) = board_json(temp.path(), &data);
    for name in ["alpha", "beta", "gamma", "delta"] {
        assert!(project(&v, name).get("loft").is_none(), "{name}: {v}");
    }
    assert!(!err.contains("loft"), "{err}");
}

#[test]
fn a_loft_path_with_invisible_or_control_characters_gives_no_field_and_a_warning() {
    for (i, bad) in ["da\u{200b}ta", "da\u{1b}[2Jta"].into_iter().enumerate() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join(bad);
        let project = run_in(temp.path(), &format!("p{i}"));
        let loft = loft_of(&data, &project);
        std::os::unix::fs::symlink(&loft, project.join(".paddock")).unwrap();
        let out = paddock(temp.path())
            .env("XDG_DATA_HOME", &data)
            .args(["board", "--json"])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{bad:?}: {err}");
        let text = String::from_utf8(out.stdout).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        // Rejected, not cleaned: no field, and no cleaned path either.
        assert!(loft_field(&v, &format!("p{i}")).is_none(), "{bad:?}: {v}");
        assert!(!text.contains("/data/paddock/loft"), "{bad:?}: {text}");
        assert!(
            err.contains("loft folder") && err.contains("invisible"),
            "{bad:?}: {err}"
        );
        assert!(!err.contains(['\u{200b}', '\u{1b}']), "{bad:?}: {err:?}");
    }
}
