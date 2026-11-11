//! The grants commands against a stopped profile and against a fake
//! running session: a control socket served by `vm::bridge` with a guest
//! that plans every `grant` line with `grants_live::Live`, as the shim
//! does, and refuses on demand.

use super::*;
use crate::macvm::grants_live::Live;
use crate::macvm::shares::Share;
use crate::vm::bridge;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const APP: &str = "testapp";

struct Fake {
    live: Mutex<Live>,
    refuse: AtomicBool,
    lines: Mutex<Vec<String>>,
    sabotage_save: Mutex<Option<PathBuf>>,
    fresh_count: Mutex<Option<Option<u32>>>,
}

impl bridge::Guest for Fake {
    fn connect(&self, _port: u32) -> Result<std::os::fd::OwnedFd> {
        bail!("no ports")
    }
    fn stop(&self) {}
    fn grant(&self, args: &str) -> Result<String> {
        self.lines.lock().unwrap().push(args.to_string());
        if self.refuse.swap(false, Ordering::SeqCst) {
            bail!("the VM did not take the folder change (test)");
        }
        let op = Op::parse(args).map_err(|e| anyhow::anyhow!(e))?;
        let mut live = self.live.lock().unwrap();
        if matches!(op, Op::ListFresh) {
            if let Some(count) = self.fresh_count.lock().unwrap().take() {
                live.handles = None;
                if let Some(count) = count {
                    live.on_handles([("ws-1".into(), count)].into(), crate::state::now());
                }
            }
        }
        let p = live
            .plan(&op, crate::state::now())
            .map_err(|e| anyhow::anyhow!(e))?;
        let mut answer = p.answer;
        if matches!(op, Op::Network { .. }) {
            answer.closed = Some(2);
        }
        *live = p.live;
        if !matches!(op, Op::List | Op::ListFresh) {
            if let Some(path) = self.sabotage_save.lock().unwrap().take() {
                std::fs::create_dir(path).unwrap();
            }
        }
        Ok(serde_json::to_string(&answer)?)
    }
}

struct World {
    _t: tempfile::TempDir,
    env: HostEnv,
    web: PathBuf,
    docs: PathBuf,
    profile: Profile,
}

fn world() -> World {
    let t = tempfile::tempdir_in("/tmp").unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    let env = crate::privfs::tests::env_in(&root);
    let web = env.home.join("src/web");
    let docs = env.home.join("docs");
    for d in [&web, &docs] {
        std::fs::create_dir_all(d).unwrap();
    }
    let profile = Layout::new(&env).profile(APP, DEFAULT_HOME);
    std::fs::create_dir_all(&profile.dir).unwrap();
    World {
        web: std::fs::canonicalize(web).unwrap(),
        docs: std::fs::canonicalize(docs).unwrap(),
        _t: t,
        env,
        profile,
    }
}

/// A running session of the profile: its record names this process and
/// a control socket served by the fake.
fn run(w: &World) -> Arc<Fake> {
    let layout = Layout::new(&w.env);
    let control = w._t.path().join("control.sock");
    let shares = vec![
        Share {
            name: "ws-1".into(),
            host: w.web.clone(),
            read_only: false,
        },
        Share {
            name: "drop".into(),
            host: w.profile.drop_dir(),
            read_only: false,
        },
    ];
    let fake = Arc::new(Fake {
        live: Mutex::new(Live::new(APP, shares, Clipboard::default(), "ask")),
        refuse: AtomicBool::new(false),
        lines: Mutex::new(Vec::new()),
        sabotage_save: Mutex::new(None),
        fresh_count: Mutex::new(None),
    });
    bridge::serve_control(bridge::bind_control(&control).unwrap(), fake.clone());
    let s = Session {
        pid: std::process::id(),
        app: APP.into(),
        home: DEFAULT_HOME.into(),
        profile: w.profile.dir.clone(),
        control,
        network_mode: "ask".into(),
        ..Default::default()
    };
    let record = layout.record(APP, DEFAULT_HOME);
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(&record, serde_json::to_vec(&s).unwrap()).unwrap();
    fake
}

fn saved(w: &World) -> grants::Grants {
    grants::load(&w.profile).unwrap()
}

#[test]
fn an_unknown_name_is_refused() {
    let w = world();
    let e = share_list(&w.env, "no-such-app", false)
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "no-such-app is not an app of paddock's macOS guest. paddock app list shows the apps"
    );
    assert!(share_list(&w.env, "../x", false).is_err());
}

#[test]
fn a_stopped_profile_keeps_the_change_for_the_next_start() {
    let w = world();
    share_add(&w.env, APP, &w.docs, false, false, true).unwrap();
    let g = saved(&w);
    assert_eq!(g.shares.len(), 1);
    assert!(g.shares[0].read_only, "read only by default");
    assert_eq!(g.changes[0].applies, Applies::NextStart);
    // Twice is refused and changes nothing.
    let e = share_add(&w.env, APP, &w.docs, true, false, true)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("is shared with testapp already, read only"),
        "{e}"
    );
    assert_eq!(saved(&w).changes.len(), 1);
    share_mode(&w.env, APP, &w.docs, false, true).unwrap();
    assert!(!saved(&w).shares[0].read_only);
    network(&w.env, APP, "none", true).unwrap();
    assert_eq!(saved(&w).network.as_deref(), Some("none"));
    assert!(network(&w.env, APP, "egress", true).is_err());
    clipboard(&w.env, APP, Some(false), None, true).unwrap();
    let c = saved(&w).clipboard;
    assert!(!c.to_app && c.from_app);
    share_remove(&w.env, APP, &w.docs, false, true).unwrap();
    assert!(saved(&w).shares.is_empty());
    let e = share_remove(&w.env, APP, &w.docs, false, true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("is not shared with testapp"), "{e}");
    // Every change is in the event log with its grant object.
    let events = crate::macvm::events::read(&w.profile.events_log());
    let grants: Vec<&str> = events
        .iter()
        .filter(|e| e.kind == "grant")
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(grants.len(), 5, "{grants:?}");
    assert!(
        grants[0].starts_with("grant folder add ~/docs ro next-start"),
        "{grants:?}"
    );
    assert_eq!(
        grants[3],
        "grant clipboard set clipboard to-app off, from-app on next-start"
    );
    // The list of a stopped profile.
    share_add(&w.env, APP, &w.web, true, false, true).unwrap();
    let rows = rows(&w.env.home, &saved(&w), None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].shown, "~/src/web");
    assert_eq!(rows[0].mode, "rw");
    assert_eq!(rows[0].status, Applies::NextStart);
    assert_eq!(share_list(&w.env, APP, true).unwrap(), 0);
}

#[test]
fn a_running_session_takes_each_change_live() {
    let w = world();
    let fake = run(&w);
    share_add(&w.env, APP, &w.docs, false, false, true).unwrap();
    let g = saved(&w);
    assert_eq!(g.shares[0].path, w.docs);
    assert_eq!(g.changes[0].applies, Applies::Live);
    assert_eq!(
        fake.lines.lock().unwrap().last().unwrap(),
        &format!("share add ro {}", w.docs.display())
    );
    // A folder of `app run` (ws-1) becomes a grant with its new mode.
    share_mode(&w.env, APP, &w.web, true, true).unwrap();
    let g = saved(&w);
    assert_eq!(g.shares.len(), 2);
    assert!(g.shares[1].read_only);
    // Open files: a remove is refused without --force, and nothing changes.
    fake.live
        .lock()
        .unwrap()
        .on_handles([("ws-1".to_string(), 2)].into(), crate::state::now());
    let e = share_remove(&w.env, APP, &w.web, false, true)
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("testapp has 2 files open in ~/src/web. Removing the folder closes them"),
        "{e}"
    );
    assert_eq!(saved(&w).shares.len(), 2);
    assert!(fake.live.lock().unwrap().has_workspace());
    share_remove(&w.env, APP, &w.web, true, true).unwrap();
    assert_eq!(saved(&w).shares.len(), 1);
    // A change that the VM refuses is not written.
    let before = std::fs::read(grants::path(&w.profile)).unwrap();
    fake.refuse.store(true, Ordering::SeqCst);
    let e = share_mode(&w.env, APP, &w.docs, false, true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("did not take the folder change"), "{e}");
    assert_eq!(std::fs::read(grants::path(&w.profile)).unwrap(), before);
    // Network and clipboard.
    network(&w.env, APP, "allowlist", true).unwrap();
    assert_eq!(fake.live.lock().unwrap().network, "allowlist");
    clipboard(&w.env, APP, None, Some(false), true).unwrap();
    assert!(!fake.live.lock().unwrap().clipboard.from_app);
    let g = saved(&w);
    assert!(g.changes.iter().all(|c| c.applies == Applies::Live));
    assert_eq!(g.changes.len(), 5);
    // The list comes from the session.
    assert_eq!(share_list(&w.env, APP, true).unwrap(), 0);
    let l = ask(&w._t.path().join("control.sock"), &Op::List).unwrap();
    assert_eq!(l.shares.len(), 1);
    assert_eq!(l.shares[0].name, "ws-2");
}

#[test]
fn a_session_that_does_not_answer_changes_nothing() {
    let w = world();
    let fake = run(&w);
    drop(fake);
    // The socket goes away: the record stays, the connect fails.
    std::fs::remove_file(w._t.path().join("control.sock")).unwrap();
    let e = share_add(&w.env, APP, &w.docs, false, false, true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("does not answer"), "{e}");
    assert!(!grants::path(&w.profile).exists() || saved(&w).shares.is_empty());
    assert!(network(&w.env, APP, "none", true).is_err());
    assert_eq!(saved(&w).network, None);
}

#[test]
fn answers_parse_or_name_the_line() {
    assert_eq!(
        parse_answer("ok {\"applies\":\"live\"}").unwrap().applies,
        Applies::Live
    );
    assert_eq!(
        parse_answer("error the VM: busy").unwrap_err().to_string(),
        "the VM: busy"
    );
    assert!(parse_answer("ok {")
        .unwrap_err()
        .to_string()
        .contains("unknown answer"));
    assert!(parse_answer("what").is_err());
}

#[test]
fn status_has_handles_clipboard_devices_and_changes() {
    let w = world();
    share_add(&w.env, APP, &w.docs, false, false, true).unwrap();
    clipboard(&w.env, APP, Some(false), Some(true), true).unwrap();
    let rec = Session {
        shares: vec![crate::macvm::host::RecordShare {
            path: w.docs.clone(),
            read_only: true,
            name: "ws-2".into(),
            open_handles: Some(3),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut v = serde_json::json!({"name": APP});
    extend_status(&mut v, &w.profile, Some(&rec));
    assert_eq!(v["name"], APP);
    assert_eq!(v["open_handles_per_share"][w.docs.display().to_string()], 3);
    assert_eq!(v["clipboard"]["to_app"], false);
    assert_eq!(v["clipboard"]["from_app"], true);
    assert_eq!(v["clipboard"]["applies"], "live");
    assert_eq!(v["devices"].as_array().unwrap().len(), 3);
    assert_eq!(v["devices"][0]["name"], "camera");
    assert_eq!(v["devices"][0]["applies"], "next-start");
    assert_eq!(v["devices"][0]["granted"], false);
    let changes = v["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["what"], "clipboard", "newest first");
    assert_eq!(changes[1]["source"], "you");
    // Stopped: no handles, the clipboard applies at the next start.
    let mut v = serde_json::json!({});
    extend_status(&mut v, &w.profile, None);
    assert_eq!(v["open_handles_per_share"], serde_json::json!({}));
    assert_eq!(v["clipboard"]["applies"], "next-start");
}

#[test]
fn record_rows_mark_the_reopen_window() {
    let now = crate::state::now();
    let s = Session {
        shares: vec![
            crate::macvm::host::RecordShare {
                path: "/a".into(),
                name: "ws-1".into(),
                reopen_until: Some(now + 20),
                ..Default::default()
            },
            crate::macvm::host::RecordShare {
                path: "/b".into(),
                name: "ws-2".into(),
                reopen_until: Some(now.saturating_sub(1)),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let r = record_rows(&s);
    assert_eq!(r[0].applies, Some(Applies::ReopenFiles));
    assert_eq!(r[1].applies, Some(Applies::Live));
    assert_eq!(r[1].guest, Path::new("/Volumes/My Shared Files/ws-2"));
    assert_eq!(
        named_folder(Path::new("x/../y"), Path::new("/nope")),
        PathBuf::from("/nope/y")
    );
}

#[test]
fn failed_save_reverts_live_network_change() {
    let w = world();
    let fake = run(&w);
    *fake.sabotage_save.lock().unwrap() = Some(grants::path(&w.profile));
    let error = network(&w.env, APP, "none", true).unwrap_err();
    assert!(format!("{error:#}").contains("live change was reverted"));
    assert_eq!(fake.live.lock().unwrap().network, "ask");
}

#[test]
fn remove_with_unknown_open_files_requires_force() {
    let w = world();
    run(&w);
    let error = share_remove(&w.env, APP, &w.web, false, true).unwrap_err();
    assert!(format!("{error:#}").contains("use --force"));
    share_remove(&w.env, APP, &w.web, true, true).unwrap();
}

#[test]
fn mode_checks_secrets_before_saving_app_run_folder() {
    let w = world();
    let fake = run(&w);
    std::fs::write(w.web.join(".env"), "SECRET=x").unwrap();
    assert!(share_mode(&w.env, APP, &w.web, true, true).is_err());
    assert!(!fake.live.lock().unwrap().shares[0].read_only);
    assert!(saved(&w).shares.is_empty());
}

#[test]
fn remove_requests_a_fresh_open_count() {
    let w = world();
    let fake = run(&w);
    fake.live
        .lock()
        .unwrap()
        .on_handles([("ws-1".into(), 0)].into(), crate::state::now());
    share_remove(&w.env, APP, &w.web, false, true).unwrap();
    assert!(fake.lines.lock().unwrap().iter().any(|s| s == "list fresh"));
}

#[test]
fn remove_uses_the_new_count_instead_of_a_stale_zero() {
    let w = world();
    let fake = run(&w);
    fake.live
        .lock()
        .unwrap()
        .on_handles([("ws-1".into(), 0)].into(), crate::state::now());
    *fake.fresh_count.lock().unwrap() = Some(Some(2));
    let error = share_remove(&w.env, APP, &w.web, false, true).unwrap_err();
    assert!(format!("{error:#}").contains("2 files open"));
    assert!(fake.live.lock().unwrap().has_workspace());
    *fake.fresh_count.lock().unwrap() = Some(None);
    let error = share_remove(&w.env, APP, &w.web, false, true).unwrap_err();
    assert!(format!("{error:#}").contains("use --force"));
}
