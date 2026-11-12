//! `paddock horse` (experimental) on a dsh session file that the test
//! writes itself: ingest, list, log, fork at a node, project the fork.
//! Temporary folders only; no dsh, no network.

#[cfg(target_os = "linux")]
#[path = "horse/store_helpers.rs"]
mod store_helpers;

use paddock::horse::dsh::{encode_file, read_file, session_dir, Header};
use std::path::Path;
use std::process::Command;

fn cmd(tmp: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.arg("horse")
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.join("home"))
        .env("XDG_DATA_HOME", tmp.join("data"))
        .env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CONFIG_HOME", tmp.join("config"))
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env("XDG_RUNTIME_DIR", tmp.join("run"));
    c
}

fn paddock(tmp: &Path, args: &[&str]) -> String {
    let out = cmd(tmp, args).output().unwrap();
    assert!(
        out.status.success(),
        "paddock horse {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn host_store(tmp: &Path) -> paddock::horse::Store {
    let vars = [("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state")]
        .into_iter()
        .map(|(key, path)| (key.to_string(), tmp.join(path).display().to_string()))
        .collect();
    let env = paddock::env::HostEnv::from_parts(tmp.join("home"), "test".into(), 0, 0, vars);
    paddock::horse::Store::open_host(&env).unwrap()
}

#[test]
fn horse_key_commands_refuse_wrong_keys_and_restore_backup() {
    let tmp = paddock::view::guard::tempdir();
    let t = tmp.path();
    paddock(t, &["list"]);
    let backup = t.join("horse.key");
    let backup_arg = backup.to_str().unwrap();
    paddock(t, &["key", "export", backup_arg]);
    let check = t.join("data/paddock/horse/key-check");
    let before = std::fs::read(&check).unwrap();
    let master = t.join("state/paddock/keyring/horse/master");
    std::fs::write(&master, [0u8; 32]).unwrap();
    let out = cmd(t, &["list"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrong key"));
    assert_eq!(std::fs::read(&check).unwrap(), before);
    assert!(!cmd(t, &["key", "import", backup_arg])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_file(&master).unwrap();
    assert!(!cmd(t, &["list"]).output().unwrap().status.success());
    assert_eq!(std::fs::read(&check).unwrap(), before);
    paddock(t, &["key", "import", backup_arg]);
    paddock(t, &["list"]);
}

#[test]
fn ingest_fork_and_project_a_dsh_session() {
    let tmp = paddock::view::guard::tempdir();
    let t = tmp.path();
    let root = t.join("dsh/sessions");
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
    let rows: Vec<Vec<u8>> = [
        r#"{"type":"turn/start","seq":0,"time":1793000000100,"data":{"turn":1}}"#,
        r#"{"type":"user/message","seq":1,"time":1793000000200,"data":{"content":[{"type":"text","text":"hi"}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}"#,
        r#"{"type":"turn/end","seq":2,"time":1793000000300,"data":{"turn":1,"reason":{"kind":"completed"}}}"#,
        r#"{"type":"turn/start","seq":3,"time":1793000000400,"data":{"turn":2}}"#,
    ]
    .iter()
    .map(|r| r.as_bytes().to_vec())
    .collect();
    let dir = session_dir(&root, header.cwd.as_deref(), &header.id);
    std::fs::create_dir_all(&dir).unwrap();
    let refs: Vec<&[u8]> = rows.iter().map(Vec::as_slice).collect();
    std::fs::write(
        dir.join("session.v4.jsonl.zstd"),
        encode_file(&header.encode(), &refs).unwrap(),
    )
    .unwrap();

    let dir_s = dir.to_str().unwrap();
    let err = cmd(t, &["ingest-dsh", dir_s, "--dsh-version", "9.9.9"])
        .output()
        .unwrap();
    assert!(!err.status.success());

    let out = paddock(t, &["ingest-dsh", dir_s, "--dsh-version", "0.2.1-rc.5"]);
    let sid = out.split(':').next().unwrap().to_string();
    assert!(out.contains("4 new"), "{out}");
    let store = t.join("data/paddock/horse");
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&store).unwrap().permissions())
            & 0o777,
        0o700
    );

    let list: serde_json::Value = serde_json::from_str(&paddock(t, &["list", "--json"])).unwrap();
    assert_eq!(list[0]["binding"]["native_id"], header.id.as_str());
    let log = paddock(t, &["log", &sid]);
    assert!(log.contains("user: hi"), "{log}");
    assert!(log.contains("seq:3"), "{log}");
    assert!(paddock(t, &["show", &sid]).contains("dsh 0.2.1-rc.5"));

    let fork = paddock(t, &["fork", &sid, "--at", "seq:2"])
        .trim()
        .to_string();
    let p = paddock(t, &["project-dsh", &fork, "--root", root.to_str().unwrap()]);
    let path = p.split_whitespace().nth(1).unwrap();
    let log = read_file(Path::new(path)).unwrap();
    assert_eq!(
        log.header.parent_session.as_deref(),
        Some(header.id.as_str())
    );
    assert!(log.header.is_seeded);
    assert_eq!(&log.rows[..3], &rows[..3]);
    assert_eq!(
        log.rows[3],
        br#"{"type":"session/end-seed","seq":3,"time":1793000000300,"data":{"inherited":true}}"#
    );

    // Without a live horse-mode run, an append waits in the inbox.
    let out = cmd(
        t,
        &["append", &sid, "--role", "note", "--text", "from the host"],
    )
    .output()
    .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("waits in its inbox"));
    let entry = String::from_utf8(out.stdout).unwrap().trim().to_string();
    let inbox = t
        .join("data/paddock/horse/inbox")
        .join(&sid)
        .join(format!("{entry}.json"));
    assert!(inbox.is_file());
    assert!(!cmd(t, &["append", &sid, "--role", "boss", "--text", "x"])
        .status()
        .unwrap()
        .success());
    assert!(paddock(t, &["status"]).contains("no live horse-mode run"));
    // No request yet: nothing is cached, so nothing is lost.
    let g = paddock(t, &["guard", &sid, "--model", "deepseek-v4-pro"]);
    assert!(g.contains("keeps the cached prefix"), "{g}");

    // DNA: show, set from a file, history, and a restore by hash.
    let show: serde_json::Value =
        serde_json::from_str(&paddock(t, &["show", &sid, "--json"])).unwrap();
    let horse = show["horse"]["id"].as_str().unwrap().to_string();
    let first = show["horse"]["dna"].as_str().unwrap().to_string();
    assert!(paddock(t, &["dna", "show", &horse]).starts_with("# horse DNA\n"));
    let file = t.join("dna.toml");
    std::fs::write(&file, "[traits]\nstyle = \"careful\"\n").unwrap();
    assert!(paddock(t, &["dna", "set", &horse, file.to_str().unwrap()]).contains("is now"));
    assert!(paddock(t, &["dna", "show", &horse]).contains("careful"));
    std::fs::write(&file, "not = [toml").unwrap();
    assert!(!cmd(t, &["dna", "set", &horse, file.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    let history = paddock(t, &["dna", "history", &horse]);
    let kinds: Vec<&str> = history
        .lines()
        .map(|l| l.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(kinds, ["dna.default", "dna.edit"], "{history}");
    assert!(!paddock(t, &["dna", "show", &horse, "--at", &first]).contains("careful"));
    paddock(t, &["dna", "set", &horse, &first]);
    assert!(!paddock(t, &["dna", "show", &horse]).contains("careful"));
    assert_eq!(paddock(t, &["dna", "history", &horse]).lines().count(), 3);
}

#[test]
fn graft_move_explain_and_revert_on_the_command_line() {
    use paddock::horse::{dna, Binding};

    let tmp = common::tempdir();
    let t = tmp.path();
    let store = host_store(t);
    let host = store.new_default_horse().unwrap();
    let donor = store.new_default_horse().unwrap();
    dna::hot_load(&store, &host.id, "[traits]\nstyle = \"short\"\n").unwrap();
    dna::hot_load(
        &store,
        &donor.id,
        "[traits]\nstyle = \"long\"\nlang = \"en\"\n",
    )
    .unwrap();
    let before = store.horse(&host.id).unwrap().dna;
    let out = paddock(t, &["graft", &host.id, &donor.id]);
    assert!(out.contains("grafted into"), "{out}");
    let explained = paddock(t, &["explain", &host.id]);
    assert!(explained.contains("style = short  ← edit ("), "{explained}");
    let alt = format!("alternative long  ← graft from {}", donor.id);
    assert!(explained.contains(&alt), "{explained}");
    assert!(explained.contains(&format!("lang = en  ← graft from {}", donor.id)));
    assert!(explained.contains("dna.graft"), "{explained}");
    // A move of an idle session: the guard answers first.
    let binding = Binding {
        harness: "dsh".into(),
        native_id: None,
        version: paddock::horse::dsh::VERSION.into(),
        container: paddock::horse::dsh::CONTAINER.into(),
    };
    let s = store.new_session(&donor.id, binding).unwrap();
    let out = paddock(t, &["move", &s.id, &host.id]);
    assert!(
        out.contains(&format!("moved from {} to {}", donor.id, host.id)),
        "{out}"
    );
    assert_eq!(store.session(&s.id).unwrap().horse, host.id);
    let show = paddock(t, &["show", &s.id]);
    assert_eq!(show.matches("lineage").count(), 2, "{show}");
    // An incompatible graft is refused.
    let other = store.new_default_horse().unwrap();
    dna::hot_load(&store, &other.id, "capabilities = [\"net\"]\n").unwrap();
    let out = cmd(t, &["graft", &host.id, &other.id]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("capability net"));
    // Revert to the DNA before the graft.
    paddock(t, &["dna", "revert", &host.id, before.as_str()]);
    assert_eq!(store.horse(&host.id).unwrap().dna, before);
    let history = paddock(t, &["dna", "history", &host.id]);
    let kinds: Vec<&str> = history
        .lines()
        .map(|l| l.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["dna.default", "dna.edit", "dna.graft", "dna.revert"],
        "{history}"
    );
}

#[test]
fn registry_keys_trust_publish_resolve_and_use_on_the_command_line() {
    use paddock::horse::dna;
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = common::tempdir();
    let t = tmp.path();
    let state = t.join("state/paddock/registry");
    // No key yet: a signed publish is refused with the next step.
    let store = host_store(t);
    let a = store.new_default_horse().unwrap();
    dna::hot_load(&store, &a.id, "[traits]\nstyle = \"registry-cli\"\n").unwrap();
    let out = cmd(t, &["registry", "publish", &a.id, "registry://t/x@1.0.0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("run paddock horse registry key new first"),
        "{out:?}"
    );
    // A key: `new` prints the public key, `show` the same, and a second
    // `new` does not replace it.
    let public = paddock(t, &["registry", "key", "new"]);
    let public = public.trim();
    assert_eq!(public.len(), 64, "{public}");
    assert_eq!(paddock(t, &["registry", "key", "show"]).trim(), public);
    assert!(!cmd(t, &["registry", "key", "new"])
        .output()
        .unwrap()
        .status
        .success());
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&state.join("local.key")), 0o600);
    assert_eq!(mode(&state), 0o700);
    // Trust the key for org t; the list shows it.
    paddock(
        t,
        &["registry", "trust", "add", "t", public, "--name", "ci"],
    );
    let list = paddock(t, &["registry", "trust", "list"]);
    assert!(list.contains(&format!("t {public} ci")), "{list}");
    assert_eq!(mode(&state.join("trust.toml")), 0o600);
    // Publish signed, resolve: Trusted, with the hash of the DNA of a.
    let out = paddock(t, &["registry", "publish", &a.id, "registry://t/x@1.0.0"]);
    let hash = store.horse(&a.id).unwrap().dna;
    assert!(out.starts_with(&format!("{hash}\n")), "{out}");
    let resolved = paddock(t, &["registry", "resolve", "registry://t/x@latest"]);
    assert!(
        resolved.contains("reference registry://t/x@1.0.0\n"),
        "{resolved}"
    );
    assert!(resolved.contains(&format!("hash {hash}\n")), "{resolved}");
    assert!(resolved.contains("verdict Trusted (ci)\n"), "{resolved}");
    // Unsigned: the policy refuses it, and resolve says so with exit 1.
    paddock(
        t,
        &[
            "registry",
            "publish",
            &a.id,
            "registry://t/y@1.0.0",
            "--unsigned",
        ],
    );
    let out = cmd(t, &["registry", "resolve", "registry://t/y@1.0.0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("verdict Unsigned"),
        "{out:?}"
    );
    let listed = paddock(t, &["registry", "list"]);
    assert!(listed.contains("t/x 1.0.0"), "{listed}");
    assert!(paddock(t, &["registry", "list", "t/x"]).contains(hash.as_str()));
    // `dna use` on a second horse: the trait comes from the registry.
    let b = store.new_default_horse().unwrap();
    let out = paddock(t, &["dna", "use", &b.id, "registry://t/x@1.0.0"]);
    assert!(out.contains("grafted into"), "{out}");
    let explained = paddock(t, &["explain", &b.id]);
    assert!(
        explained.contains("style = registry-cli  ← registry://t/x@1.0.0 ("),
        "{explained}"
    );
}

#[test]
fn registry_sync_of_a_plain_folder_and_of_a_git_checkout() {
    let tmp = common::tempdir();
    let t = tmp.path();
    let plain = t.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    paddock(
        t,
        &["registry", "index", "add", "plain", plain.to_str().unwrap()],
    );
    let out = paddock(t, &["registry", "sync", "plain"]);
    assert!(out.contains("nothing to sync"), "{out}");
    // The local index stays first after an add.
    let listed = paddock(t, &["registry", "list"]);
    let names: Vec<&str> = listed
        .lines()
        .filter_map(|l| l.strip_prefix("index "))
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert_eq!(names, ["local", "plain"], "{listed}");
    // A git checkout of a local repository: a pull, no network.
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    let origin = t.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(t, &["clone", "-q", origin.to_str().unwrap(), "checkout"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let checkout = t.join("checkout");
    paddock(
        t,
        &[
            "registry",
            "index",
            "add",
            "team",
            checkout.to_str().unwrap(),
        ],
    );
    let out = paddock(t, &["registry", "sync", "team"]);
    assert!(out.contains("index team is up to date"), "{out}");
    let log = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout), "two\none\n");
    assert!(!cmd(t, &["registry", "sync", "nope"])
        .output()
        .unwrap()
        .status
        .success());
}

mod common;
/// Bwrap and FUSE work here, so a sandbox test can run.
#[cfg(target_os = "linux")]
fn sandbox_works() -> bool {
    common::sandbox_works()
}
/// A test folder whose dsh harness is the fake dsh `fixture`.
#[cfg(target_os = "linux")]
fn sandbox_setup(fixture: &str) -> common::TestDir {
    let tmp = test_folder();
    install_harness(tmp.path(), "dsh", fixture, "[launch]\nargs = [\"web\"]\n");
    tmp
}
/// An empty test folder: project, home, runtime folder and harness folders.
#[cfg(target_os = "linux")]
fn test_folder() -> common::TestDir {
    use std::os::unix::fs::PermissionsExt;
    let tmp = common::tempdir();
    let root = tmp.path();
    for dir in [
        "project",
        "home",
        "run",
        "harness/bin",
        "cfg/paddock/harness.d",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::set_permissions(root.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
    tmp
}
/// A fake harness of `tests/fixtures/horse`.
fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/horse")
        .join(name)
}
/// Install the fixture `fixture` as the harness `name` of the test folder
/// `root`, with `profile` after the name and the locate table.
#[cfg(target_os = "linux")]
fn install_harness(root: &Path, name: &str, fixture: &str, profile: &str) {
    use std::os::unix::fs::PermissionsExt;
    // The install root of a bin executable is its grandparent; keep it
    // separate from the test home so it can be mounted in the sandbox.
    let fake = root.join("harness/bin").join(name);
    std::fs::copy(self::fixture(fixture), &fake).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        root.join(format!("cfg/paddock/harness.d/{name}.toml")),
        format!(
            "name = \"{name}\"\n[locate]\ncommands = [{}]\n{profile}",
            serde_json::to_string(fake.to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();
}
/// Make the project of the test folder `root` a git repository whose one
/// commit has `task.md` with `task`.
#[cfg(target_os = "linux")]
fn task_project(root: &Path, task: &str) {
    let project = root.join("project");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(project.join("task.md"), task).unwrap();
    git(&["add", "task.md"]);
    git(&["commit", "-q", "-m", "one"]);
}
/// `paddock run dsh --horse` in the test folder, stopped after a minute.
#[cfg(target_os = "linux")]
fn run_dsh(root: &Path) -> std::process::Output {
    run_dsh_for(root, 60)
}
/// `paddock run dsh --horse` in the test folder, stopped after `secs`.
#[cfg(target_os = "linux")]
fn run_dsh_for(root: &Path, secs: u64) -> std::process::Output {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args([
            "run",
            "dsh",
            "--horse",
            "--backend",
            "bwrap",
            "--network",
            "none",
            "--env",
            "clean",
            "-y",
        ])
        .arg(root.join("project"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .current_dir(root.join("project"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}
#[test]
#[cfg(target_os = "linux")]
fn horse_per_run_and_sandbox_device() {
    use paddock::horse::run;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = sandbox_setup("dsh");
    let root = tmp.path();
    let store_path = root.join("data/paddock/horse");
    let mut first = String::new();
    for count in 1..=2 {
        let out = run_dsh(root);
        assert!(out.status.success(), "{out:?}");
        let store = store_helpers::open(&store_path).unwrap();
        let horses = store.horses().unwrap();
        assert_eq!(horses.len(), count);
        let sid = std::fs::read_to_string(root.join("project/sessions.txt")).unwrap();
        let sid = sid.trim();
        let session = store.session(sid).unwrap();
        let horse = store.horse(&session.horse).unwrap();
        assert_eq!(horse.sessions, [sid]);
        if count == 1 {
            first = horse.id.clone();
        } else {
            assert_ne!(horse.id, first);
        }
        let events = run::events(&store, &horse.id).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["dna.default", "run.begin", "run.end", "dna.adapt"]
        );
        let device = std::fs::read(root.join("project/device-log.txt")).unwrap();
        assert!(!device.is_empty());
        let broker = std::fs::read(root.join("project/broker-log.txt")).unwrap();
        assert!(String::from_utf8_lossy(&broker).contains("horse test"));
        assert!(!String::from_utf8_lossy(&device).contains("horse test"));
    }
}
#[test]
#[cfg(target_os = "linux")]
fn a_dna_edit_in_the_sandbox_hot_loads_at_the_next_idle() {
    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = sandbox_setup("dsh-dna");
    let root = tmp.path();
    let out = run_dsh(root);
    assert!(out.status.success(), "{out:?}");
    // The fake dsh writes every request it gets after the edit.
    let prompts = std::fs::read_to_string(root.join("project/prompts.txt")).unwrap();
    let entry = prompts
        .lines()
        .find(|l| l.contains("careful-e2e"))
        .unwrap_or_else(|| panic!("no DNA entry: {prompts}"));
    assert!(entry.contains("session/prompt"), "{entry}");
    assert!(entry.contains("role=\\\"system\\\""), "{entry}");
    assert!(entry.contains("hot-load"), "{entry}");
    let store = host_store(root);
    let sid = std::fs::read_to_string(root.join("project/sessions.txt")).unwrap();
    let horse = store.session(sid.trim()).unwrap().horse;
    let history = paddock(root, &["dna", "history", &horse]);
    let kinds: Vec<&str> = history
        .lines()
        .map(|l| l.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(kinds, ["dna.default", "dna.edit", "dna.adapt"], "{history}");
}
#[test]
#[cfg(target_os = "linux")]
fn a_graft_from_the_host_reaches_the_idle_session() {
    use paddock::horse::dna;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = sandbox_setup("dsh-graft");
    let root = tmp.path().to_path_buf();
    let run = {
        let root = root.clone();
        std::thread::spawn(move || run_dsh(&root))
    };
    // The fake writes its session id when the session is idle.
    let ready = root.join("project/sessions.txt");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let sid = std::fs::read_to_string(&ready).expect("the fake session did not start");
    let store = host_store(&root);
    let horse = store.session(sid.trim()).unwrap().horse;
    let donor = store.new_default_horse().unwrap();
    dna::hot_load(&store, &donor.id, "[traits]\nlang = \"graft-e2e\"\n").unwrap();
    assert!(paddock(&root, &["graft", &horse, &donor.id]).contains("grafted into"));
    let out = run.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let prompts = std::fs::read_to_string(root.join("project/prompts.txt")).unwrap();
    let entry = prompts
        .lines()
        .find(|l| l.contains("graft-e2e"))
        .unwrap_or_else(|| panic!("no graft entry: {prompts}"));
    assert!(entry.contains("role=\\\"system\\\""), "{entry}");
    assert!(entry.contains("graft from"), "{entry}");
    let explained = paddock(&root, &["explain", &horse]);
    assert!(
        explained.contains(&format!("lang = graft-e2e  ← graft from {}", donor.id)),
        "{explained}"
    );
}
/// Answer every pending card of the test folder `root` with `once` until
/// `stop` turns true. Returns how many it answered.
#[cfg(target_os = "linux")]
fn auto_approve(
    root: std::path::PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<usize> {
    std::thread::spawn(move || {
        let paddock = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_paddock"))
                .args(args)
                .env("HOME", root.join("home"))
                .env("XDG_CONFIG_HOME", root.join("cfg"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("XDG_STATE_HOME", root.join("state"))
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("XDG_RUNTIME_DIR", root.join("run"))
                .output()
                .unwrap()
        };
        let mut answered = 0;
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let out = paddock(&["approvals", "--json"]);
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            for p in v["pending"].as_array().into_iter().flatten() {
                if p["capability"] == "horse" {
                    let id = p["id"].as_str().unwrap();
                    if paddock(&["approvals", "answer", id, "once"])
                        .status
                        .success()
                    {
                        answered += 1;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        answered
    })
}
#[test]
#[cfg(target_os = "linux")]
fn a_sandbox_fetch_with_use_is_approved_and_reaches_the_idle_session() {
    use paddock::horse::dna;
    use paddock::horse::registry::{artifact_hash, host, trust, Manifest, Transport};

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = sandbox_setup("dsh-registry");
    let root = tmp.path().to_path_buf();
    // The local index of the host, with t/x@1.0.0 signed by a trusted key.
    let state = root.join("state/paddock");
    let key = trust::new_keypair();
    host::trust_add(&state, "t", &key.public_hex(), None).unwrap();
    let bytes = dna::parse("[traits]\nlang = \"registry-e2e\"\n")
        .unwrap()
        .encode()
        .into_bytes();
    let mut m = Manifest {
        name: "t/x".into(),
        version: "1.0.0".into(),
        hash: artifact_hash(&bytes),
        parents: Vec::new(),
        created_ms: 1,
        author: None,
        signature: None,
    };
    m.sign(&key);
    paddock::horse::registry::configured_for_store(&state, &host_store(&root))
        .unwrap()
        .remove(0)
        .put(&m, &bytes)
        .unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approver = auto_approve(root.clone(), stop.clone());
    let out = run_dsh(&root);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(approver.join().unwrap(), 1);
    assert!(out.status.success(), "{out:?}");
    let fetched = std::fs::read_to_string(root.join("project/fetch.txt")).unwrap();
    assert!(fetched.starts_with(&format!("{}\n", m.hash)), "{fetched}");
    assert!(!fetched.contains(state.to_str().unwrap()), "{fetched}");
    assert!(!fetched.contains(&key.public_hex()), "{fetched}");
    let prompts = std::fs::read_to_string(root.join("project/prompts.txt")).unwrap();
    let entry = prompts
        .lines()
        .find(|l| l.contains("registry-e2e"))
        .unwrap_or_else(|| panic!("no registry entry: {prompts}"));
    assert!(entry.contains("role=\\\"system\\\""), "{entry}");
    let store = host_store(&root);
    let sid = std::fs::read_to_string(root.join("project/sessions.txt")).unwrap();
    let horse = store.session(sid.trim()).unwrap().horse;
    let explained = paddock(&root, &["explain", &horse]);
    assert!(
        explained.contains("lang = registry-e2e  ← registry://t/x@1.0.0 ("),
        "{explained}"
    );
}
/// Claude Code and Codex children of a run in horse mode, and the Codex swap.
#[path = "horse/children.rs"]
mod children;
/// An interactive Claude Code run in horse mode.
#[cfg(target_os = "linux")]
#[path = "horse/tui.rs"]
mod tui;

/// The agy codec on the command line.
#[path = "horse/agy.rs"]
mod agy;
/// agy children and the agy TUI in horse mode.
#[cfg(target_os = "linux")]
#[path = "horse/agy_custody.rs"]
mod agy_custody;
/// An interactive Claude Code run with no channel, through the relay.
#[cfg(target_os = "linux")]
#[path = "horse/claude_tui_pty.rs"]
mod claude_tui_pty;
#[cfg(target_os = "linux")]
#[path = "horse/codex_tui.rs"]
mod codex_tui;
/// `paddock horse grant` and the ctl file of a granted sandbox.
#[path = "horse/ctl.rs"]
mod ctl;
/// The Grok Build codec on the command line.
#[path = "horse/grok.rs"]
mod grok;
/// Grok Build children and the Grok Build TUI in horse mode.
#[cfg(target_os = "linux")]
#[path = "horse/grok_custody.rs"]
mod grok_custody;
/// Hitched children of a run in horse mode.
#[cfg(target_os = "linux")]
#[path = "horse/hitch.rs"]
mod hitch;
