//! Tests of the namespaces of `[kv.namespaces]` (namespace.rs).

use super::shard;
use super::store::Store;
use super::*;
use crate::broker::{Answer, Approver, Pending, Verdict};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Counts the approval cards: a namespace must never show one.
#[derive(Default)]
struct Cards(Mutex<Vec<Pending>>);

impl Approver for Cards {
    fn ask(&self, _: &HostEnv, p: &Pending, _: Duration) -> Option<Answer> {
        self.0.lock().unwrap().push(p.clone());
        Some(Answer::Session)
    }
    fn ask_verdict(&self, env: &HostEnv, p: &Pending, wait: Duration) -> Option<Verdict> {
        self.ask(env, p, wait).map(|answer| Verdict {
            answer,
            note: None,
            who: "user",
        })
    }
}

struct World {
    _dir: tempfile::TempDir,
    env: HostEnv,
    root: PathBuf,
    /// Two member projects and one that is not a member.
    paddock: PathBuf,
    loft: PathBuf,
    other: PathBuf,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let [paddock, loft, other] = ["src/paddock", "src/loft", "src/other"].map(|p| home.join(p));
    for p in [&paddock, &loft, &other] {
        std::fs::create_dir_all(p).unwrap();
    }
    let env = HostEnv::from_parts(home, "u".into(), 1000, 1000, BTreeMap::new());
    let root = kv_root(&env);
    World {
        _dir: dir,
        env,
        root,
        paddock,
        loft,
        other,
    }
}

const CFG: &str = r#"
[kv.namespaces.host-build]
projects = ["~/src/paddock", "~/src/loft"]
harnesses = ["claude", "codex"]

[kv.namespaces.open]
projects = ["~/src/paddock"]
"#;

fn cfg() -> KvCfg {
    KvCfg::parse(CFG).unwrap()
}

/// A sandbox of `project` with `harness`, started by the sandboxes in
/// `chain` (the root first).
fn sandbox(
    w: &World,
    id: &str,
    harness: &str,
    project: &Path,
    chain: &[&str],
    cfg: KvCfg,
) -> (Service, Arc<Cards>) {
    let cards = Arc::new(Cards::default());
    let svc = Service::new(
        w.env.clone(),
        Identity {
            sandbox: id.into(),
            harness: harness.into(),
            project: project.into(),
            pasture: None,
            parent: chain.last().map(|s| s.to_string()),
            chain: chain.iter().map(|s| s.to_string()).collect(),
            tree: chain.first().map(|s| s.to_string()),
        },
        cfg,
        cards.clone(),
        Duration::from_secs(1),
    )
    .with_root(w.root.clone());
    Store::open(&shard::registry(&w.root))
        .unwrap()
        .register(
            id,
            harness,
            project,
            chain.last().copied(),
            None,
            crate::state::now(),
        )
        .unwrap();
    (svc, cards)
}

fn req(op: &str, ns: &str, key: &str) -> Request {
    Request {
        v: PROTOCOL,
        op: op.into(),
        scope: Some(Scope::Ns),
        namespace: Some(ns.into()),
        key: (!key.is_empty()).then(|| key.to_string()),
        ..Default::default()
    }
}

fn put(ns: &str, key: &str, value: &str) -> Request {
    Request {
        value: Some(value.into()),
        ..req("put", ns, key)
    }
}

fn claim(ns: &str, name: &str) -> Request {
    Request {
        ttl: Some(600),
        ..req("claim", ns, name)
    }
}

#[test]
fn the_configuration_names_projects_and_harnesses() {
    let c = cfg();
    let hb = &c.namespaces["host-build"];
    assert_eq!(hb.projects, ["~/src/paddock", "~/src/loft"]);
    assert_eq!(
        hb.harnesses.as_deref(),
        Some(&["claude".into(), "codex".into()][..])
    );
    assert_eq!(c.namespaces["open"].harnesses, None);
    // Without the table, there is no namespace, and the other scopes stay.
    let d = KvCfg::parse("").unwrap();
    assert!(d.namespaces.is_empty());
    assert_eq!(d, KvCfg::default());
    for (bad, says) in [
        (
            "[kv.namespaces.Host]\nprojects = [\"/p\"]\n",
            "not a namespace name",
        ),
        (
            "[kv.namespaces.\"a/b\"]\nprojects = [\"/p\"]\n",
            "not a namespace name",
        ),
        ("[kv.namespaces.x]\nprojects = []\n", "projects is empty"),
        (
            "[kv.namespaces.x]\nprojects = [\"src/p\"]\n",
            "not an absolute path",
        ),
        (
            "[kv.namespaces.x]\nprojects = [\"/p\"]\nharnesses = []\n",
            "harnesses is empty",
        ),
        (
            "[kv.namespaces.x]\nprojects = [\"/p\"]\nharnesses = [\"a b\"]\n",
            "not a harness name",
        ),
        ("[kv.namespaces.x]\nprojects = [\"/p\"]\nwrite = true\n", ""),
        ("[kv.namespaces.x]\nharnesses = [\"claude\"]\n", ""),
    ] {
        let e = format!("{:#}", KvCfg::parse(bad).unwrap_err());
        assert!(e.contains(says), "{bad}: {e}");
    }
}

#[test]
fn scope_syntax() {
    assert_eq!(
        parse_scope("ns:host-build").unwrap(),
        (Scope::Ns, Some("host-build".into()))
    );
    assert_eq!(parse_scope("user").unwrap(), (Scope::User, None));
    assert!(parse_scope("ns").unwrap_err().contains("ns:NAME"));
    assert!(parse_scope("ns:").is_err());
    assert!(parse_scope("ns:../x").is_err());
    assert!(parse_scope("other").unwrap_err().contains("ns:NAME"));
    // The wire format: the scope ns and the name beside it.
    let line = serde_json::to_string(&req("get", "host-build", "k")).unwrap();
    assert!(
        line.contains(r#""scope":"ns","namespace":"host-build""#),
        "{line}"
    );
}

#[test]
fn members_of_two_projects_read_and_write() {
    let w = world();
    let (a, cards_a) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    let (b, cards_b) = sandbox(&w, "b", "codex", &w.loft, &[], cfg());
    let r = a.handle(&put("host-build", "note", "from paddock"), None);
    assert_eq!(r.status, Status::Done, "{}", r.message);
    let v1 = r.result["version"].as_u64().unwrap();
    let got = b.handle(&req("get", "host-build", "note"), None);
    assert_eq!(got.status, Status::Done, "{}", got.message);
    assert_eq!(got.result["value"], "from paddock");
    assert_eq!(got.result["writer"], "a");
    assert_eq!(got.result["untrusted"], true);
    // Versions work as in the other scopes.
    let stale = b.handle(
        &Request {
            version: Some(v1 + 100),
            ..put("host-build", "note", "x")
        },
        None,
    );
    assert_eq!(stale.status, Status::Conflict);
    let r = b.handle(
        &Request {
            version: Some(v1),
            ..put("host-build", "note", "from loft")
        },
        None,
    );
    assert_eq!(r.status, Status::Done);
    let list = a.handle(&req("list", "host-build", ""), None);
    assert_eq!(list.result["entries"][0]["key"], "note");
    assert_eq!(list.result["entries"][0]["writer"], "b");
    // The data is in its own file, apart from the user scope.
    assert!(w.root.join("ns/host-build.db").is_file());
    assert!(!w.root.join("user.db").exists());
    // No approval card: the user configuration is the approval.
    assert!(cards_a.0.lock().unwrap().is_empty());
    assert!(cards_b.0.lock().unwrap().is_empty());
    // The size limit holds.
    let big = "x".repeat(KvCfg::default().max_value + 1);
    assert_eq!(
        a.handle(&put("host-build", "big", &big), None).status,
        Status::TooLarge
    );
}

#[test]
fn a_project_that_is_not_a_member_is_refused() {
    let w = world();
    let (a, _) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    assert_eq!(
        a.handle(&put("host-build", "k", "v"), None).status,
        Status::Done
    );
    let (x, cards) = sandbox(&w, "x", "claude", &w.other, &[], cfg());
    for r in [
        req("get", "host-build", "k"),
        put("host-build", "k", "w"),
        req("list", "host-build", ""),
        claim("host-build", "slot"),
        req("release", "host-build", "slot"),
        Request {
            timeout: Some(1),
            ..req("watch", "host-build", "k")
        },
    ] {
        let resp = x.handle(&r, None);
        assert_eq!(resp.status, Status::Denied, "{}", r.op);
        assert!(resp.message.contains("'host-build'"), "{}", resp.message);
        assert!(
            resp.message
                .contains("adds it to projects in [kv.namespaces.host-build]"),
            "{}",
            resp.message
        );
        assert!(resp.message.contains("~/src/other"), "{}", resp.message);
    }
    assert!(
        cards.0.lock().unwrap().is_empty(),
        "no card for a namespace"
    );
    // Nothing changed.
    let (a2, _) = sandbox(&w, "a2", "claude", &w.paddock, &[], cfg());
    assert_eq!(
        a2.handle(&req("get", "host-build", "k"), None).result["value"],
        "v"
    );
    // The audit log records the refusal with the namespace.
    let log = std::fs::read_to_string(audit_file(&w.env)).unwrap();
    assert!(log
        .lines()
        .any(|l| l.contains(r#""namespace":"host-build""#) && l.contains(r#""sandbox":"x""#)));
}

#[test]
fn a_harness_that_is_not_a_member_is_refused() {
    let w = world();
    let (g, cards) = sandbox(&w, "g", "gemini", &w.paddock, &[], cfg());
    let r = g.handle(&put("host-build", "k", "v"), None);
    assert_eq!(r.status, Status::Denied);
    assert!(
        r.message
            .contains("the harness 'gemini' is not a member of the kv namespace 'host-build'")
            && r.message.contains("adds it to harnesses"),
        "{}",
        r.message
    );
    assert!(cards.0.lock().unwrap().is_empty());
    // Without harnesses, every harness of a member project is a member.
    assert_eq!(g.handle(&put("open", "k", "v"), None).status, Status::Done);
}

#[test]
fn a_spawned_sandbox_gets_no_more_than_the_sandboxes_above_it() {
    let w = world();
    // A gemini root may not use host-build, so its claude child may not.
    let (_root, _) = sandbox(&w, "r", "gemini", &w.paddock, &[], cfg());
    let (child, _) = sandbox(&w, "c", "claude", &w.paddock, &["r"], cfg());
    let resp = child.handle(&put("host-build", "k", "v"), None);
    assert_eq!(resp.status, Status::Denied);
    assert!(
        resp.message
            .contains("sandbox r (gemini) is above this sandbox"),
        "{}",
        resp.message
    );
    // A claude root and its codex child are members.
    let (_root2, _) = sandbox(&w, "r2", "claude", &w.paddock, &[], cfg());
    let (child2, _) = sandbox(&w, "c2", "codex", &w.paddock, &["r2"], cfg());
    assert_eq!(
        child2.handle(&put("host-build", "k", "v"), None).status,
        Status::Done
    );
    // A sandbox above without a record fails closed.
    let (orphan, _) = sandbox(&w, "c3", "claude", &w.paddock, &["gone"], cfg());
    let resp = orphan.handle(&req("get", "host-build", "k"), None);
    assert_eq!(resp.status, Status::Denied);
    assert!(resp.message.contains("no record"), "{}", resp.message);
}

#[test]
fn an_unknown_namespace_names_the_table_to_add() {
    let w = world();
    let (a, cards) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    let r = a.handle(&put("missing", "k", "v"), None);
    assert_eq!(r.status, Status::Disabled);
    assert!(
        r.message.contains("there is no kv namespace 'missing'")
            && r.message
                .contains("[kv.namespaces.missing] projects = [\"~/src/paddock\"]"),
        "{}",
        r.message
    );
    assert!(cards.0.lock().unwrap().is_empty());
    assert!(!w.root.join("ns/missing.db").exists());
}

#[test]
fn a_project_paddock_toml_cannot_add_a_namespace() {
    let w = world();
    let file = w.paddock.join(crate::manifest::PROJECT_FILE);
    std::fs::write(
        &file,
        "[kv.namespaces.host-build]\nprojects = [\"~/src/other\"]\n",
    )
    .unwrap();
    let e = format!("{:#}", crate::manifest::Manifest::read(&file).unwrap_err());
    assert!(
        e.contains("[kv] is only read from the user configuration"),
        "{e}"
    );
    // The kv configuration comes from the user file only: the project file
    // adds nothing, so a sandbox of the other project stays outside.
    let loaded = KvCfg::load(&w.env).unwrap();
    assert!(loaded.namespaces.is_empty());
    let (x, _) = sandbox(&w, "x", "claude", &w.other, &[], cfg());
    assert_eq!(
        x.handle(&put("host-build", "k", "v"), None).status,
        Status::Denied
    );
}

#[test]
fn leases_in_a_namespace() {
    let w = world();
    let (a, _) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    let (b, _) = sandbox(&w, "b", "codex", &w.loft, &[], cfg());
    let r = a.handle(&claim("host-build", "compile-slot-1"), None);
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_eq!(r.result["owner"], "a");
    assert_eq!(r.result["lease"], "lease:compile-slot-1");
    // The other project sees it taken, and cannot free it.
    let held = b.handle(&claim("host-build", "compile-slot-1"), None);
    assert_eq!(held.status, Status::Conflict);
    assert_eq!(held.result["owner"], "a");
    let free = |s: &Service, force: bool| {
        s.handle(
            &Request {
                force: Some(force),
                ..req("release", "host-build", "compile-slot-1")
            },
            None,
        )
    };
    assert_eq!(free(&b, false).status, Status::Denied);
    assert_eq!(free(&b, true).status, Status::Denied);
    // A plain write cannot forge a lease.
    assert_eq!(
        b.handle(&put("host-build", "lease:compile-slot-1", "{}"), None)
            .status,
        Status::Invalid
    );
    // The owner renews and frees it; then the other one takes it.
    let renewed = a.handle(&claim("host-build", "compile-slot-1"), None);
    assert_eq!(renewed.result["renewed"], true);
    assert_eq!(free(&a, false).status, Status::Done);
    assert_eq!(free(&a, false).status, Status::NotFound);
    assert_eq!(
        b.handle(&claim("host-build", "compile-slot-1"), None)
            .status,
        Status::Done
    );
    // A frozen sandbox frees its leases in a namespace too.
    let freed = release_leases(&w.root, "b", &KvCfg::default(), crate::state::now()).unwrap();
    assert_eq!(
        freed,
        [("ns".to_string(), "lease:compile-slot-1".to_string())]
    );
}

#[test]
fn a_watch_in_a_namespace_wakes_on_a_write_from_another_project() {
    let w = world();
    let (a, _) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    let (b, _) = sandbox(&w, "b", "codex", &w.loft, &[], cfg());
    let v = a.handle(&put("host-build", "k", "1"), None).result["version"]
        .as_u64()
        .unwrap();
    let r = b.handle(
        &Request {
            version: Some(v),
            timeout: Some(1),
            ..req("watch", "host-build", "k")
        },
        None,
    );
    assert_eq!(r.status, Status::Waiting);
    a.handle(&put("host-build", "k", "2"), None);
    let r = b.handle(
        &Request {
            version: Some(v),
            timeout: Some(1),
            ..req("watch", "host-build", "k")
        },
        None,
    );
    assert_eq!(r.status, Status::Done);
    assert_eq!(r.result["value"], "2");
}

#[test]
fn requests_that_mix_scopes_are_invalid() {
    let w = world();
    let (a, _) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    let no_name = Request {
        namespace: None,
        ..req("get", "host-build", "k")
    };
    assert_eq!(a.handle(&no_name, None).status, Status::Invalid);
    let wrong = Request {
        scope: Some(Scope::User),
        ..req("get", "host-build", "k")
    };
    let r = a.handle(&wrong, None);
    assert_eq!(r.status, Status::Invalid);
    assert!(r.message.contains("scope ns only"), "{}", r.message);
    let bad = req("get", "../x", "k");
    assert_eq!(a.handle(&bad, None).status, Status::Invalid);
    let timer = Request {
        action: Some("start".into()),
        budget: Some(60),
        ..req("timer", "host-build", "t")
    };
    assert_eq!(a.handle(&timer, None).status, Status::Invalid);
}

#[test]
fn a_running_sandbox_follows_the_user_configuration() {
    let w = world();
    let file = w.env.paddock_config().join("config.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, CFG).unwrap();
    let (a, _) = sandbox(&w, "a", "claude", &w.paddock, &[], KvCfg::default());
    let a = a.with_live_namespaces();
    assert_eq!(
        a.handle(&put("host-build", "k", "v"), None).status,
        Status::Done
    );
    let info = a.handle(
        &Request {
            v: PROTOCOL,
            op: "info".into(),
            ..Default::default()
        },
        None,
    );
    let names: Vec<&str> = info.result["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["scope"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ns:host-build", "ns:open"]);
    // The user takes the project out: the next request is refused.
    std::fs::write(
        &file,
        "[kv.namespaces.host-build]\nprojects = [\"~/src/loft\"]\n",
    )
    .unwrap();
    assert_eq!(
        a.handle(&req("get", "host-build", "k"), None).status,
        Status::Denied
    );
}

#[test]
fn the_host_user_needs_no_membership() {
    let w = world();
    let (svc, _) = sandbox(&w, "user", "host", Path::new(""), &[], cfg());
    let svc = svc.as_host_user();
    assert_eq!(
        svc.handle(&claim("host-build", "compile-slot-2"), None)
            .status,
        Status::Done
    );
    assert_eq!(
        svc.handle(&put("missing", "k", "v"), None).status,
        Status::Disabled
    );
    let (a, _) = sandbox(&w, "a", "claude", &w.paddock, &[], cfg());
    let held = a.handle(&claim("host-build", "compile-slot-2"), None);
    assert_eq!(held.status, Status::Conflict);
    assert_eq!(held.result["owner"], "user");
}
