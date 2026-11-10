//! `paddock-host horse ctl` through the broker: the grant, the card with
//! the cost, the ledger row and the audit line.
use super::*;
use crate::horse::device::ctl::{grant, tests as ctl_tests};

fn rows(b: &Broker) -> Vec<String> {
    let c = rusqlite::Connection::open(crate::devices::ledger::path(&b.env)).unwrap();
    let mut s = c
        .prepare("SELECT status FROM requests WHERE device = 'horse' ORDER BY id")
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A horse with one fresh dsh session for the broker. Returns the session.
fn give_horse(b: &Broker, tmp: &Path) -> String {
    let store = crate::horse::Store::open(&tmp.join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let sid = ctl_tests::ingest(&store, &tmp.join("sessions"), "session-b", &h.id);
    b.horse
        .set(crate::horse::device::Access {
            store,
            horse: h.id,
            session: sid.clone(),
        })
        .ok()
        .unwrap();
    sid
}

fn give_grant(b: &Broker, tokens: u64) {
    let access = b.horse.get().unwrap();
    grant::set(
        &access.store,
        &grant::Grant {
            // The sandbox id of the test broker.
            sandbox: "t-1".into(),
            commands: vec!["fork".into()],
            horses: Vec::new(),
            budget: 5,
            tokens,
            used: 0,
            spent_tokens: 0,
            granted_ms: 1,
        },
    )
    .unwrap();
}

fn ctl(line: &str) -> Request {
    req("horse", serde_json::json!({"op": "ctl", "command": line}))
}

#[test]
fn an_ungranted_ctl_is_denied_with_one_ledger_row_and_no_card() {
    let (tmp, b, fixed) = setup(Some(Answer::Once), HostPolicy::default());
    let sid = give_horse(&b, tmp.path());
    let r = b.handle(&ctl(&format!("fork {sid}")));
    assert_eq!(r.status, Status::Denied);
    assert!(r.message.contains("no ctl grant"), "{}", r.message);
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 0);
    assert_eq!(rows(&b), ["ctl fork refused no grant"]);
    let access = b.horse.get().unwrap();
    assert_eq!(access.store.horse(&access.horse).unwrap().sessions, [sid]);
}

#[test]
fn an_approved_break_runs_with_the_card_the_ledger_and_the_audit_line() {
    let (tmp, b, fixed) = setup(Some(Answer::Once), HostPolicy::default());
    let sid = give_horse(&b, tmp.path());
    give_grant(&b, 1_000_000);
    let r = b.handle(&ctl(&format!("fork {sid} {}", ctl_tests::MID)));
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 1);
    let bytes: Vec<u8> = serde_json::from_value(r.result["bytes"].clone()).unwrap();
    let id = String::from_utf8(bytes).unwrap();
    let access = b.horse.get().unwrap();
    assert_eq!(access.store.session(id.trim()).unwrap().horse, access.horse);
    let g = grant::get(&access.store, "t-1").unwrap().unwrap();
    assert_eq!(
        rows(&b),
        [format!(
            "ctl fork {sid} approved lost {} tokens",
            g.spent_tokens
        )]
    );
    let audit = std::fs::read_to_string(audit_file(&b.env)).unwrap();
    let line: serde_json::Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(line["capability"], "horse");
    assert_eq!(line["decision"], "allowed once");
    assert_eq!(line["detail"], format!("ctl fork {sid}"));
}

#[test]
fn a_denied_break_writes_nothing() {
    let (tmp, b, fixed) = setup(Some(Answer::Deny), HostPolicy::default());
    let sid = give_horse(&b, tmp.path());
    give_grant(&b, 1_000_000);
    let r = b.handle(&ctl(&format!("fork {sid} {}", ctl_tests::MID)));
    assert_eq!(r.status, Status::Denied);
    assert!(r.message.contains("nothing changed"), "{}", r.message);
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 1);
    assert_eq!(rows(&b), [format!("ctl fork {sid} denied")]);
    let access = b.horse.get().unwrap();
    assert_eq!(access.store.horse(&access.horse).unwrap().sessions, [sid]);
    assert_eq!(grant::get(&access.store, "t-1").unwrap().unwrap().used, 0);
}
