#![cfg(target_os = "linux")]
//! One walk through horse mode, from a new horse to a resumed one: a
//! horse per run, DNA inherit, a hitched child whose result reaches the
//! parent, custody of a running turn, the TUI channel, a graft and its
//! undo, and stop and resume. Only the fake harnesses of
//! `tests/fixtures/horse` run. No vendor endpoint, no real session file.
//!
//! The walk has two forms. `the_whole_walk_in_sandboxes` runs `paddock run`
//! with bwrap and skips without bwrap or FUSE. `the_whole_walk_on_the_host`
//! takes the same steps without a sandbox: the run lifecycle of the
//! library holds the fakes as child processes, and the host CLI does what
//! the user does.

mod common;

#[path = "horse_e2e/host.rs"]
mod host;
#[path = "horse_e2e/sandbox.rs"]
mod sandbox;
#[path = "horse_e2e/tui.rs"]
mod tui;

use paddock::horse::{hitch, Store};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// A fake harness of `tests/fixtures/horse`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/horse")
        .join(name)
}

/// The XDG folders of the test folder `t`, as the host CLI gets them.
fn xdg(t: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("HOME", t.join("home")),
        ("XDG_CONFIG_HOME", t.join("cfg")),
        ("XDG_DATA_HOME", t.join("data")),
        ("XDG_STATE_HOME", t.join("state")),
        ("XDG_CACHE_HOME", t.join("cache")),
        ("XDG_RUNTIME_DIR", t.join("run")),
    ]
}

/// An empty test folder: project, home, runtime folder, configuration.
fn test_folder() -> common::TestDir {
    use std::os::unix::fs::PermissionsExt;
    let tmp = common::tempdir();
    let t = tmp.path();
    for dir in [
        "project",
        "home",
        "run",
        "harness/bin",
        "cfg/paddock/harness.d",
    ] {
        std::fs::create_dir_all(t.join(dir)).unwrap();
    }
    std::fs::set_permissions(t.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
    tmp
}

/// `paddock ARGS` in the test folder `t`. Its input is empty.
fn paddock_out(t: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .envs(xdg(t))
        .output()
        .unwrap()
}

/// `paddock horse ARGS` in the test folder `t`: its output, or a failed
/// test with its error.
fn horse(t: &Path, args: &[&str]) -> String {
    let mut all = vec!["horse"];
    all.extend_from_slice(args);
    let out = paddock_out(t, &all);
    assert!(
        out.status.success(),
        "paddock horse {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Wait until `f` holds; else fail with `what` and the state of `report`.
fn wait_until(what: &str, report: &dyn Fn() -> String, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !f() {
        assert!(Instant::now() < deadline, "timed out: {what}\n{}", report());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A file of the project of `t`, or "" when it is not there.
fn project_file(t: &Path, name: &str) -> String {
    std::fs::read_to_string(t.join("project").join(name)).unwrap_or_default()
}

/// What a failed check shows: the sessions, inboxes, deliveries and edges
/// of the store, what the fake dsh saw, and the results of the children.
fn report(t: &Path) -> String {
    let mut r = String::new();
    if let Ok(store) = Store::open_host(&host::host_env(t)) {
        for s in store.sessions().unwrap_or_default() {
            r.push_str(&format!(
                "session {} horse {} harness {} native {:?} nodes {} inbox {}\n",
                s.id,
                s.horse,
                s.binding.harness,
                s.binding.native_id,
                store.session_nodes(&s).map(|n| n.len()).unwrap_or(0),
                store.inbox(&s.id).map(|i| i.len()).unwrap_or(0)
            ));
            for d in store.deliveries(&s.id).unwrap_or_default() {
                let head: String = d.entry.text.chars().take(40).collect();
                r.push_str(&format!(
                    "  delivery {:?} landed {:?}\n",
                    head, d.landed_seq
                ));
            }
        }
        for h in store.horses().unwrap_or_default() {
            for e in hitch::edges(&store, &h.id).unwrap_or_default() {
                r.push_str(&format!("edge {:?} {} {}\n", e.kind, e.from, e.to));
            }
        }
    }
    for f in [
        "trace.txt",
        "prompts.txt",
        "dsh-stderr.txt",
        "plain.txt",
        "hitched.txt",
    ] {
        r.push_str(&format!("{f}:\n{}\n", project_file(t, f)));
    }
    // The result and the output of each spawned child.
    for f in ["plain.txt", "hitched.txt"] {
        let id = project_file(t, f);
        if id.trim().is_empty() {
            continue;
        }
        let dir = t
            .join("project/.paddock/inbox")
            .join(format!("spawn-{}", id.trim()));
        for name in ["result.json", "output.txt"] {
            let text = std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| e.to_string());
            r.push_str(&format!("{f} {name}: {text}\n"));
        }
    }
    r
}

/// The deliveries to `session` whose text starts with `head`.
fn delivered(store: &Store, session: &str, head: &str) -> Vec<paddock::horse::inbox::Delivery> {
    store
        .deliveries(session)
        .unwrap()
        .into_iter()
        .filter(|d| d.entry.text.starts_with(head))
        .collect()
}

/// The raw rows of `session`, as text.
fn rows(store: &Store, session: &str) -> Vec<String> {
    let s = store.session(session).unwrap();
    store
        .session_nodes(&s)
        .unwrap()
        .into_iter()
        .map(|n| String::from_utf8_lossy(&n.raw).into_owned())
        .collect()
}

/// The lines of trace.txt of the fake dsh.
fn trace(t: &Path) -> Vec<String> {
    project_file(t, "trace.txt")
        .lines()
        .map(str::to_string)
        .collect()
}

/// The index of the first trace line with `part` in it.
fn trace_at(t: &Path, part: &str) -> usize {
    trace(t)
        .iter()
        .position(|l| l.contains(part))
        .unwrap_or_else(|| panic!("no trace line with {part}\n{}", report(t)))
}

/// The prompt with `part` came at a safe point: right after an idle.
fn at_safe_point(t: &Path, part: &str) {
    let k = trace_at(t, part);
    assert!(trace(t)[k].starts_with("prompt "), "{part}\n{}", report(t));
    assert!(k > 0, "{}", report(t));
    assert_eq!(trace(t)[k - 1], "idle", "{part}\n{}", report(t));
}

/// Write the dsh session file of `native` under `root` with the rows of
/// `session`, as dsh keeps it in its home.
fn write_session_file(store: &Store, session: &str, root: &Path, native: &str, cwd: &str) {
    use paddock::horse::dsh::{encode_file, session_dir, Header};
    let header = Header {
        id: native.into(),
        created_at: 1_793_000_000_000,
        cwd: Some(cwd.into()),
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: 0,
        agent_preset: None,
    };
    let s = store.session(session).unwrap();
    let raws: Vec<Vec<u8>> = store
        .session_nodes(&s)
        .unwrap()
        .into_iter()
        .map(|n| n.raw)
        .collect();
    let refs: Vec<&[u8]> = raws.iter().map(Vec::as_slice).collect();
    let dir = session_dir(root, Some(cwd), native);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("session.v4.jsonl.zstd"),
        encode_file(&header.encode(), &refs).unwrap(),
    )
    .unwrap();
}

/// The DNA graft of a donor into `horse` and its undo, on the command
/// line. Returns the donor.
fn graft_and_undo(t: &Path, store: &Store, horse_id: &str) -> String {
    use paddock::horse::dna;
    let donor = store.new_default_horse().unwrap();
    dna::hot_load(store, &donor.id, "[traits]\nlang = \"walk-graft\"\n").unwrap();
    let before = store.horse(horse_id).unwrap().dna;
    let out = horse(t, &["graft", horse_id, &donor.id]);
    assert!(out.contains("grafted into"), "{out}");
    let explained = horse(t, &["explain", horse_id]);
    let line = format!("lang = walk-graft  ← graft from {}", donor.id);
    assert!(explained.contains(&line), "{explained}");
    horse(t, &["dna", "revert", horse_id, before.as_str()]);
    assert_eq!(store.horse(horse_id).unwrap().dna, before);
    let history = horse(t, &["dna", "history", horse_id]);
    let kinds: Vec<&str> = history
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    assert_eq!(
        kinds[kinds.len() - 2..],
        ["dna.graft", "dna.revert"],
        "{history}"
    );
    assert!(!horse(t, &["explain", horse_id]).contains("walk-graft"));
    donor.id
}
