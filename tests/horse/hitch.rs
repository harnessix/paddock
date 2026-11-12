//! Hitch mode in a real sandbox: a dsh parent in horse mode spawns two
//! hitched fake Claude Code children. Their results reach the parent's
//! session once each, and a child sees nothing of its sibling.

use super::*;
use paddock::horse::hitch::{self, EdgeKind};
use paddock::horse::{run, Store};

/// A test folder whose dsh parent is the fake `parent` and whose claude
/// children are the fake claude. `horse` is the `[horse] enabled` value.
fn hitch_setup(parent: &str, horse: bool) -> common::TestDir {
    let tmp = sandbox_setup(parent);
    let root = tmp.path();
    install_harness(
        root,
        "claude",
        "claude",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"-p\", \"--dangerously-skip-permissions\"]\nprompt = \"stdin\"\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        format!("[host]\nspawn = \"allow\"\n[spawn]\nharnesses = [\"claude\"]\nmax_running = 2\ntimeout = 120\n[horse]\nenabled = {horse}\n"),
    )
    .unwrap();
    task_project(root, "write the note\n");
    tmp
}

/// `paddock run dsh` in the test folder without horse mode, stopped after
/// `secs`.
fn run_plain_dsh(root: &Path, secs: u64) -> std::process::Output {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "dsh", "--backend", "bwrap", "--network", "none"])
        .args(["--env", "clean", "-y"])
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

fn read(root: &Path, file: &str) -> String {
    std::fs::read_to_string(root.join("project").join(file))
        .unwrap_or_else(|e| panic!("{file}: {e}"))
        .trim()
        .to_string()
}

/// The result.json of the child `id` in the parent's inbox.
fn result_of(root: &Path, id: &str) -> serde_json::Value {
    let p = root
        .join("project/.paddock/inbox")
        .join(format!("spawn-{id}"))
        .join("result.json");
    serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap()
}

fn git_show(root: &Path, what: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root.join("project"))
        .args(["show", what])
        .output()
        .unwrap();
    assert!(out.status.success(), "git show {what}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// The child sessions of the horses in the store: the `from` of each
/// SpawnedFrom edge. A hitched child has its edge before it starts.
fn hitched_children(store: &Store) -> Vec<String> {
    let mut out = Vec::new();
    for h in store.horses().unwrap_or_default() {
        for e in hitch::edges(store, &h.id).unwrap_or_default() {
            if e.kind == EdgeKind::SpawnedFrom && !out.contains(&e.from) {
                out.push(e.from);
            }
        }
    }
    out
}

/// What a failed check of a hitched pair shows: the parent run, the store
/// sessions and edges, and the result and output of each child.
fn report(root: &Path, out: &std::process::Output, fed: &[String]) -> String {
    let mut r = format!(
        "parent run: {out:?}
fed: {fed:?}
"
    );
    let trace = std::fs::read_to_string(root.join("project/trace.txt")).unwrap_or_default();
    r.push_str(&format!("trace:\n{trace}"));
    {
        let store = host_store(root);
        for s in store.sessions().unwrap_or_default() {
            r.push_str(&format!(
                "session {} horse {} harness {} native {:?} inbox {}\n",
                s.id,
                s.horse,
                s.binding.harness,
                s.binding.native_id,
                store.inbox(&s.id).map(|i| i.len()).unwrap_or(0)
            ));
        }
        for h in store.horses().unwrap_or_default() {
            for e in hitch::edges(&store, &h.id).unwrap_or_default() {
                r.push_str(&format!("edge {:?} {} {}\n", e.kind, e.from, e.to));
            }
        }
    }
    for f in ["child-a.txt", "child-b.txt"] {
        let id = std::fs::read_to_string(root.join("project").join(f)).unwrap_or_default();
        let dir = root
            .join("project/.paddock/inbox")
            .join(format!("spawn-{}", id.trim()));
        for name in ["result.json", "output.txt"] {
            let text = std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| e.to_string());
            r.push_str(&format!("{f} {name}: {text}\n"));
        }
    }
    r
}

/// Run the hitch parent. The fake claude children hold their first turn
/// until the test appends an entry to their sessions, as a host would.
/// The test finds each child by its SpawnedFrom edge, which the parent's
/// paddock writes before the child starts, so it cannot miss a child.
fn run_hitched_pair(root: &Path) -> (std::process::Output, Store) {
    let parent = {
        let root = root.to_path_buf();
        std::thread::spawn(move || run_dsh_for(&root, 240))
    };
    let store_path = root.join("data/paddock/horse");
    let mut fed = Vec::<String>::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(200);
    while fed.len() < 2 && !parent.is_finished() && std::time::Instant::now() < deadline {
        let children = store_helpers::open(&store_path)
            .map(|st| hitched_children(&st))
            .unwrap_or_default();
        for c in children {
            if !fed.contains(&c) {
                paddock(root, &["append", &c, "--text", "from the test"]);
                fed.push(c);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = parent.join().unwrap();
    assert!(out.status.success(), "{}", report(root, &out, &fed));
    assert_eq!(fed.len(), 2, "{}", report(root, &out, &fed));
    // Each child ended with exit 0: none gave up waiting for its entry.
    for f in ["child-a.txt", "child-b.txt"] {
        let result = result_of(root, &read(root, f));
        assert_eq!(result["exit_code"], 0, "{}", report(root, &out, &fed));
    }
    (out, store_helpers::open(&store_path).unwrap())
}

#[test]
fn two_hitched_children_join_the_parent_horse_and_their_results_reach_it_once() {
    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = hitch_setup("dsh-hitch", true);
    let root = tmp.path();
    let (_, store) = run_hitched_pair(root);
    let parent = read(root, "sessions.txt");
    let horse = store.session(&parent).unwrap().horse;

    // One horse with three sessions; one SpawnedFrom and one Append per child.
    assert_eq!(store.horses().unwrap().len(), 1);
    let sessions = store.horse(&horse).unwrap().sessions;
    assert_eq!(sessions.len(), 3, "{sessions:?}");
    let edges = hitch::edges(&store, &horse).unwrap();
    let children: Vec<String> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::SpawnedFrom)
        .map(|e| e.from.clone())
        .collect();
    assert_eq!(children.len(), 2, "{edges:?}");
    for c in &children {
        assert!(sessions.contains(c));
        assert_eq!(store.session(c).unwrap().binding.harness, "claude");
        let to_parent = |k: EdgeKind| {
            edges
                .iter()
                .filter(|e| e.kind == k && e.from == *c && e.to == parent)
                .count()
        };
        assert_eq!(to_parent(EdgeKind::SpawnedFrom), 1, "{edges:?}");
        assert_eq!(to_parent(EdgeKind::Append), 1, "{edges:?}");
    }

    // Each result came to the parent once, and landed in its session.
    let nodes = store
        .session_nodes(&store.session(&parent).unwrap())
        .unwrap();
    let mut delivered: Vec<_> = store
        .deliveries(&parent)
        .unwrap()
        .into_iter()
        .filter(|d| d.entry.text.starts_with("hitched "))
        .collect();
    assert_eq!(delivered.len(), 2, "{delivered:?}");
    for c in &children {
        let head = format!("hitched {} (claude) finished", &c[..8]);
        let mine: Vec<_> = delivered
            .iter()
            .filter(|d| d.entry.text.starts_with(&head))
            .collect();
        assert_eq!(mine.len(), 1, "{delivered:?}");
        let rows = nodes
            .iter()
            .filter(|n| String::from_utf8_lossy(&n.raw).contains(&head))
            .count();
        assert_eq!(rows, 1, "{head}");
    }
    delivered.sort_by_key(|d| d.landed_seq);
    for d in &delivered {
        // The delivery went on top of the head of that moment: the result
        // is the newest entry of the session.
        let k = d.landed_seq.expect("a landed delivery") as usize;
        let raw = String::from_utf8_lossy(&nodes[k].raw).into_owned();
        assert!(raw.contains("\"user/message\""), "{raw}");
        assert!(raw.contains(&d.entry.text[..17]), "{raw}");
        assert_eq!(nodes[k].meta.parent, d.at_head, "{raw}");
    }
    let last = delivered.last().unwrap().landed_seq.unwrap() as usize;
    assert_eq!(last, nodes.len() - 1);
    assert!(store.inbox(&parent).unwrap().is_empty());

    // Safe points only: every prompt came right after an idle of the fake.
    let trace = read(root, "trace.txt");
    let lines: Vec<&str> = trace.lines().collect();
    assert!(lines.iter().filter(|l| **l == "prompt").count() >= 2);
    for (i, l) in lines.iter().enumerate() {
        if *l == "prompt" {
            assert_eq!(lines[i - 1], "idle", "{trace}");
        }
    }

    // The parent's run ended, with the children's runs before it.
    let events = run::events(&store, &horse).unwrap();
    let ends: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "run.end")
        .filter_map(|e| e["session"].as_str())
        .collect();
    assert_eq!(ends.len(), 3, "{events:?}");
    assert_eq!(ends.last(), Some(&parent.as_str()), "{events:?}");
}

#[test]
fn a_hitched_child_sees_only_its_own_session() {
    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = hitch_setup("dsh-hitch", true);
    let root = tmp.path();
    let (_, store) = run_hitched_pair(root);
    let parent = read(root, "sessions.txt");
    let horse = store.session(&parent).unwrap().horse;
    let ids = [read(root, "child-a.txt"), read(root, "child-b.txt")];
    let mut owns = Vec::new();
    for id in &ids {
        let result = result_of(root, id);
        assert_eq!(result["exit_code"], 0, "{result}");
        let branch = result["branch"].as_str().unwrap();
        let seen = git_show(root, &format!("{branch}:seen.txt"));
        let own = seen
            .lines()
            .find_map(|l| l.strip_prefix("own "))
            .unwrap_or_else(|| panic!("{seen}"))
            .to_string();
        assert_eq!(store.session(&own).unwrap().horse, horse);
        let listed: Vec<&str> = seen
            .lines()
            .filter_map(|l| l.strip_prefix("listed "))
            .collect();
        assert_eq!(listed, [own.as_str()], "{seen}");
        assert!(seen.contains(&format!("horse-sessions {own} \n")), "{seen}");
        // The parent and the sibling are instances, and their logs are closed.
        let denied: Vec<&str> = seen
            .lines()
            .filter_map(|l| l.strip_prefix("denied "))
            .collect();
        assert_eq!(denied.len(), 2, "{seen}");
        assert!(denied.contains(&parent.as_str()), "{seen}");
        assert!(!seen.lines().any(|l| l.starts_with("read ")), "{seen}");
        // Edges and instances show ids, kinds and states only.
        for l in seen.lines() {
            if let Some(e) = l.strip_prefix("edge ") {
                assert_eq!(e.split(' ').count(), 3, "{l}");
            }
            if let Some(i) = l.strip_prefix("instance ") {
                assert_eq!(i.split(' ').count(), 3, "{l}");
            }
        }
        owns.push((
            own,
            denied.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ));
    }
    assert_ne!(owns[0].0, owns[1].0);
    assert!(owns[0].1.contains(&owns[1].0), "{owns:?}");
    assert!(owns[1].1.contains(&owns[0].0), "{owns:?}");
    // Nothing of a child's own content reached its sibling's session.
    for (own, _) in &owns {
        let s = store.session(own).unwrap();
        for n in store.session_nodes(&s).unwrap() {
            let raw = String::from_utf8_lossy(&n.raw);
            for (other, _) in owns.iter().filter(|(o, _)| o != own) {
                assert!(!raw.contains(other.as_str()), "{raw}");
            }
        }
    }
}

#[test]
fn hitch_without_horse_mode_is_refused_and_plain_spawn_is_unchanged() {
    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = hitch_setup("dsh-hitch-off", false);
    let root = tmp.path();
    let out = run_plain_dsh(root, 180);
    assert!(out.status.success(), "{out:?}");
    assert_ne!(read(root, "hitch.code"), "0");
    assert!(read(root, "hitch.err").contains("--hitch needs horse mode"));
    assert!(read(root, "hitch.out").is_empty());
    let id = read(root, "child-id.txt");
    let result = result_of(root, &id);
    assert_eq!(result["state"], "exited", "{result}");
    assert_eq!(result["exit_code"], 0, "{result}");
    let output = std::fs::read_to_string(
        root.join("project/.paddock/inbox")
            .join(format!("spawn-{id}"))
            .join("output.txt"),
    )
    .unwrap();
    assert!(output.contains("plain child: write the note"), "{output}");
    assert!(!root.join("data/paddock/horse/lock").exists());
}
