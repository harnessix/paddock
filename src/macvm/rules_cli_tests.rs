//! The JSON of `paddock app rules` and `paddock app connections` against
//! a stopped profile and a fake running session. The fake serves the
//! control socket with the real firewall of a session (`gate::Firewall`),
//! which writes the snapshot `session-rules.json` as the shim does.

use super::*;
use crate::macvm::gate::Firewall;
use crate::vm::bridge;
use std::sync::{Arc, Mutex};

const APP: &str = "sample-notes";

struct World {
    t: tempfile::TempDir,
    env: HostEnv,
}

fn world() -> World {
    let t = tempfile::tempdir_in("/tmp").unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    let env = crate::privfs::tests::env_in(&root);
    let profiles = env.home.join(".config/paddock/harness.d");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join(format!("{APP}.toml")),
        format!("name = \"{APP}\"\n[app]\n[app.macos]\nseed_hosts = [\"login.example.com\"]\n"),
    )
    .unwrap();
    std::fs::create_dir_all(Layout::new(&env).profile(APP, DEFAULT_HOME).dir).unwrap();
    World { t, env }
}

struct Fake {
    fw: Firewall,
    lines: Mutex<Vec<String>>,
}

impl bridge::Guest for Fake {
    fn connect(&self, _port: u32) -> Result<std::os::fd::OwnedFd> {
        bail!("no ports")
    }
    fn stop(&self) {}
    fn grant(&self, args: &str) -> Result<String> {
        self.lines.lock().unwrap().push(args.to_string());
        let op = Op::parse(args).map_err(|e| anyhow::anyhow!(e))?;
        let a = self
            .fw
            .rule_op(&op)
            .unwrap_or_else(|| Ok(crate::macvm::grants_live::Answer::live()))
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(serde_json::to_string(&a)?)
    }
}

/// A running session: a record with this process and a control socket.
fn run(w: &World) -> Arc<Fake> {
    let layout = Layout::new(&w.env);
    let profile = layout.profile(APP, DEFAULT_HOME);
    let log = profile.connections_log();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let rules = Rules::load(&w.env, APP).unwrap();
    let fake = Arc::new(Fake {
        fw: Firewall::new(firewall::Mode::Ask, None, rules, None, log),
        lines: Mutex::new(Vec::new()),
    });
    let control = w.t.path().join("control.sock");
    bridge::serve_control(bridge::bind_control(&control).unwrap(), fake.clone());
    let s = Session {
        pid: std::process::id(),
        app: APP.into(),
        home: DEFAULT_HOME.into(),
        profile: profile.dir.clone(),
        control,
        network_mode: "ask".into(),
        ..Default::default()
    };
    let record = layout.record(APP, DEFAULT_HOME);
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(&record, serde_json::to_vec(&s).unwrap()).unwrap();
    fake
}

fn spec(host: &str, verdict: Verdict, lifetime: Lifetime) -> Spec {
    Spec {
        host: host.into(),
        port: Port::Num(443),
        app: "*".into(),
        team: None,
        verdict,
        lifetime,
        site: false,
        yes_all_hosts: false,
    }
}

fn ctx(w: &World) -> Ctx {
    Ctx::open(&w.env, APP).unwrap()
}

fn json(o: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(o).unwrap()
}

#[test]
fn a_stopped_profile_lists_adds_changes_and_removes_rules_of_the_file() {
    let w = world();
    // The seed lists before the file exists, with who made it.
    let l = json(&listing(&ctx(&w)));
    assert_eq!(l["v"], 1);
    assert_eq!(l["running"], false);
    assert_eq!(l["rules"][0]["host"], "login.example.com");
    assert_eq!(l["rules"][0]["source"], "seed");
    assert_eq!(l["rules"][0]["by"], "profile");
    assert_eq!(l["rules"][0]["lifetime"], "always");
    assert!(l.get("warning").is_none());

    // add: the result, the rule written and the new list.
    let r = add(
        &w.env,
        &mut ctx(&w),
        &spec("files.example.com", Verdict::Allow, Lifetime::Always),
    );
    let o = json(&outcome(&w.env, APP, &r));
    assert_eq!(o["ok"], true);
    assert_eq!(o["applies"], "next_start");
    let rule = &o["rule"];
    for (k, v) in [
        ("host", "files.example.com"),
        ("verdict", "allow"),
        ("answer", "allow"),
        ("scope", "always"),
        ("lifetime", "always"),
        ("source", "you"),
        ("by", "user"),
    ] {
        assert_eq!(rule[k], v, "{k}: {o}");
    }
    assert_eq!(rule["port"], 443);
    assert!(rule["id"].as_str().unwrap().starts_with("rule-"));
    assert!(rule["made"].as_str().unwrap().ends_with('Z'));
    assert_eq!(o["rules"].as_array().unwrap().len(), 1);
    assert_eq!(o["list"].as_array().unwrap().len(), 2, "{o}");
    let id = rule["id"].as_str().unwrap().to_string();

    // A site: the host and `*.host`, the partner with the suffix.
    let mut s = spec("example.org", Verdict::Allow, Lifetime::Always);
    s.site = true;
    s.port = Port::Any;
    let d = add(&w.env, &mut ctx(&w), &s).unwrap();
    assert_eq!(d.written.len(), 2);
    assert_eq!(d.written[1].host, "*.example.org");
    assert_eq!(d.written[1].id, format!("{}{SUBDOMAINS}", d.written[0].id));
    let o = json(&outcome(&w.env, APP, &Ok(d)));
    assert_eq!(o["rules"][0]["port"], "*");
    let site_id = o["rules"][0]["id"].as_str().unwrap().to_string();
    let mut bad = spec("10.0.0.1", Verdict::Allow, Lifetime::Always);
    bad.site = true;
    assert!(add(&w.env, &mut ctx(&w), &bad).is_err());

    // change: deny instead of allow, in place of the old id.
    let d = change(
        &w.env,
        &mut ctx(&w),
        &id,
        &ChangeSpec {
            verdict: Some(Verdict::Deny),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(d.removed, vec![id.clone()]);
    assert_eq!(d.written[0].verdict, Verdict::Deny);
    assert_eq!(d.written[0].host, "files.example.com");
    let file = Rules::load(&w.env, APP).unwrap().file;
    assert_eq!(file.len(), 3);
    assert!(file.iter().all(|r| r.id != id));

    // change of a site takes its partner along.
    let d = change(
        &w.env,
        &mut ctx(&w),
        &site_id,
        &ChangeSpec {
            host: Some("example.net".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(d.removed.len(), 2, "{:?}", d.removed);
    assert_eq!(d.written.len(), 1);
    assert_eq!(Rules::load(&w.env, APP).unwrap().file.len(), 2);

    // remove by host and by id; one unknown target changes nothing.
    let before = Rules::load(&w.env, APP).unwrap().file;
    let r = remove(
        &w.env,
        &mut ctx(&w),
        &["files.example.com".into(), "rule-nope".into()],
    );
    let o = json(&outcome(&w.env, APP, &r));
    assert_eq!(o["ok"], false);
    assert!(
        o["refused"].as_str().unwrap().contains("no rule rule-nope"),
        "{o}"
    );
    assert_eq!(Rules::load(&w.env, APP).unwrap().file, before);
    let ids: Vec<String> = before.iter().map(|r| r.id.clone()).collect();
    let d = remove(
        &w.env,
        &mut ctx(&w),
        &["files.example.com".into(), ids[1].clone()],
    )
    .unwrap();
    assert_eq!(d.removed.len(), 2);
    assert!(Rules::load(&w.env, APP).unwrap().file.is_empty());

    // Refusals: a seed, every host, a session rule of a stopped app.
    let seed = ctx(&w).rules.seeds[0].id.clone();
    let e = remove(&w.env, &mut ctx(&w), &[seed])
        .unwrap_err()
        .to_string();
    assert!(e.contains("seed rule"), "{e}");
    let r = add(
        &w.env,
        &mut ctx(&w),
        &spec("*", Verdict::Allow, Lifetime::Always),
    );
    let o = json(&outcome(&w.env, APP, &r));
    assert!(
        o["refused"].as_str().unwrap().contains("--yes-all-hosts"),
        "{o}"
    );
    for l in [Lifetime::Session, Lifetime::Minutes(10)] {
        let e = add(
            &w.env,
            &mut ctx(&w),
            &spec("a.example.com", Verdict::Deny, l),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("needs a running session"), "{e}");
    }
    assert!(Rules::load(&w.env, APP).unwrap().file.is_empty());
}

#[test]
fn a_running_session_takes_session_and_timed_rules_live() {
    let w = world();
    let fake = run(&w);
    let c = ctx(&w);
    assert!(c.running.is_some());
    // --session: the session takes the rule, the file stays empty.
    let d = add(
        &w.env,
        &mut ctx(&w),
        &spec("tracker.example.net", Verdict::Deny, Lifetime::Session),
    )
    .unwrap();
    assert_eq!(d.applies, "live");
    let session_id = d.written[0].id.clone();
    assert!(session_id.starts_with("session-"));
    assert!(fake.lines.lock().unwrap()[0].starts_with("rule add {"));
    assert!(!Rules::path(&w.env, APP).exists());
    assert_eq!(fake.fw.rules().session.len(), 1);

    // --minutes: an end time, ten minutes on.
    let now = crate::state::now();
    let d = add(
        &w.env,
        &mut ctx(&w),
        &spec("api.example.com", Verdict::Allow, Lifetime::Minutes(10)),
    )
    .unwrap();
    let end = d.written[0].expires.unwrap();
    assert!((now + 600..=now + 602).contains(&end), "{end}");
    let o = json(&outcome(&w.env, APP, &Ok(d)));
    assert_eq!(o["rule"]["scope"], "session");
    assert_eq!(o["rule"]["lifetime"], "minutes");
    assert_eq!(o["rule"]["expires"], crate::waiting::format_rfc3339(end));
    assert!(e_minutes_refused(&w));

    // The list has the session rules from the snapshot, with the warning.
    let l = json(&listing(&ctx(&w)));
    assert_eq!(l["running"], true);
    assert!(l["warning"].as_str().unwrap().contains("snapshot"));
    let lifetimes: Vec<&str> = l["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["lifetime"].as_str().unwrap())
        .collect();
    assert_eq!(lifetimes, ["session", "minutes", "always"]);

    // An always rule while the app runs applies live.
    let d = add(
        &w.env,
        &mut ctx(&w),
        &spec("files.example.com", Verdict::Allow, Lifetime::Always),
    )
    .unwrap();
    assert_eq!(d.applies, "live");

    // change a session rule into an always rule: the session loses it.
    let d = change(
        &w.env,
        &mut ctx(&w),
        &session_id,
        &ChangeSpec {
            lifetime: Some(Lifetime::Always),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(d.removed, vec![session_id.clone()]);
    assert_eq!(d.written[0].scope, Scope::Always);
    assert_eq!(fake.fw.rules().session.len(), 1);
    assert_eq!(Rules::load(&w.env, APP).unwrap().file.len(), 2);

    // remove of the timed rule by its host goes to the session.
    let d = remove(&w.env, &mut ctx(&w), &["api.example.com".into()]).unwrap();
    assert_eq!(d.removed.len(), 1);
    assert!(fake.fw.rules().session.is_empty());
    assert!(fake
        .lines
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.starts_with("rule remove session-")));
}

fn e_minutes_refused(w: &World) -> bool {
    [0, MAX_MINUTES + 1].iter().all(|m| {
        add(
            &w.env,
            &mut ctx(w),
            &spec("x.example.com", Verdict::Allow, Lifetime::Minutes(*m)),
        )
        .unwrap_err()
        .to_string()
        .contains("not a time")
    })
}

fn entry(t: u64, host: &str, verdict: &str, rule: &str, count: u64) -> Entry {
    let mut e = Entry::new(t, "com.example.notes", None, host, 443, verdict, rule);
    e.count = count;
    e.last = t + 5;
    e
}

#[test]
fn connections_come_from_the_log_without_its_lock() {
    let w = world();
    let profile = Layout::new(&w.env).profile(APP, DEFAULT_HOME);
    let log = profile.connections_log();
    // No log: no rows, and no last session.
    let c = connections(&w.env, APP, 200).unwrap();
    assert!(c.connections.is_empty() && !c.last_session && !c.running);
    for e in [
        entry(100, "api.example.com", connlog::ALLOW, "rule-1", 2),
        entry(130, "t.example.net", connlog::DENY, "session-2", 1),
        entry(200, "api.example.com", connlog::ALLOW, "rule-1", 3),
        entry(300, "new.example.com", connlog::ASK, "ask", 1),
    ] {
        connlog::record_at(&log, e).unwrap();
    }
    // A writer holds the lock: the reader does not wait for it.
    let lock = std::fs::File::open(log.with_file_name("connections.lock")).unwrap();
    use std::os::fd::AsRawFd as _;
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let started = std::time::Instant::now();
    let c = connections(&w.env, APP, 200).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    drop(lock);
    assert!(c.last_session && !c.running);
    let v = json(&c);
    assert_eq!(v["v"], 1);
    let rows = v["connections"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{v}");
    // The newest last time first; lines of one target add up.
    assert_eq!(rows[0]["host"], "new.example.com");
    assert_eq!(rows[1]["host"], "api.example.com");
    assert_eq!(rows[1]["count"], 5);
    assert_eq!(rows[1]["rule"], "rule-1");
    assert_eq!(rows[1]["answer"], "allow");
    assert_eq!(rows[1]["port"], 443);
    assert_eq!(rows[1]["time"], crate::waiting::format_rfc3339(100));
    assert_eq!(rows[1]["last"], crate::waiting::format_rfc3339(205));
    assert_eq!(rows[2]["verdict"], "deny");
    assert_eq!(connections(&w.env, APP, 1).unwrap().connections.len(), 1);
    // The rules count their connections from the same log.
    let used = usage(&connlog::read_shared(&log));
    assert_eq!(used["rule-1"], (5, 205));
    // A bad name and an unknown app are refused.
    assert!(connections(&w.env, "../x", 10).is_err());
    assert!(connections(&w.env, "no-such-app", 10).is_err());
    // While the session runs, the rows are of it.
    let _fake = run(&w);
    let c = connections(&w.env, APP, 200).unwrap();
    assert!(c.running && !c.last_session);
}

#[test]
fn who_made_a_rule_and_how_long_it_applies() {
    let now = 1_700_000_000;
    let seed = firewall::seed_rule("login.example.com", now);
    assert_eq!((source(&seed), lifetime(&seed)), ("seed", "always"));
    let mut r = firewall::new_rule(
        "*",
        None,
        "a.example",
        Port::Any,
        Verdict::Deny,
        Scope::Session,
        firewall::BY_APPROVER,
        now,
    );
    assert_eq!((source(&r), lifetime(&r)), ("approver", "session"));
    r.expires = Some(now + 600);
    r.by = firewall::BY_USER.into();
    assert_eq!((source(&r), lifetime(&r)), ("you", "minutes"));
    r.by = firewall::BY_PROFILE.into();
    assert_eq!(source(&r), "profile");
    let j = json(&rule_json(&r, &BTreeMap::new()));
    assert_eq!(j["count"], 0);
    assert!(j.get("last_used").is_none());
    assert_eq!(j["expires"], crate::waiting::format_rfc3339(now + 600));
}
