//! Synthetic compatibility and write-order checks: bytes in memory, apart
//! from the start decode test, which uses a temporary folder. They do not
//! model filesystem or power-loss durability.
use super::*;
use crate::eventstream::schema::{Event, GrantSource, Line, StreamState};
use crate::macvm::firewall::{self, Conn, Decision, Mode, Port, Rule, Scope, Verdict};

#[path = "../../tests/fixtures/legacy/grants_851faba.rs"]
mod legacy;
#[path = "../../tests/fixtures/legacy/stream_851faba.rs"]
mod legacy_stream;

fn policy() -> Grants {
    Grants {
        network: Some("none".into()),
        shares: vec![FolderGrant {
            path: "/home/demo/work".into(),
            read_only: true,
            unprotected: true,
            added: 4,
        }],
        clipboard: Clipboard {
            to_app: true,
            from_app: false,
        },
        changes: vec![Change::new(
            5,
            What::Network,
            "set",
            "none",
            "none",
            Applies::NextStart,
            Source::You,
        )],
        ..Grants::default()
    }
}

fn historical(t: u64, action: &str) -> Change {
    Change::new(
        t,
        What::Rule,
        action,
        "api.example:443",
        "allow always (historical; order unknown)",
        Applies::Unknown,
        Source::You,
    )
}

fn rule(by: &str) -> Rule {
    Rule {
        id: format!("allow-{by}"),
        app: "*".into(),
        team: None,
        host: "api.example".into(),
        port: Port::Num(443),
        verdict: Verdict::Allow,
        scope: Scope::Always,
        made: "2020-01-01T00:00:00Z".into(),
        expires: None,
        by: by.into(),
    }
}

fn new_read(policy: &[u8], sidecar: Option<&[u8]>, recovery: bool) -> Grants {
    let g: Grants = serde_json::from_slice(policy).unwrap();
    assemble(g, &history::decode(sidecar), Some(policy), recovery)
}

fn sidecar(changes: &[Change]) -> Vec<u8> {
    let mut g = policy();
    for change in changes {
        history::record(&mut g, change.clone());
    }
    history::plan(&g, &history::Read::Missing)
        .unwrap()
        .sidecar
        .unwrap()
}

fn checkpoint(change: Change) -> Vec<u8> {
    let mut g = policy();
    g.changes.push(change);
    serde_json::to_vec(&g).unwrap()
}

/// A process-order model. Failure occurs before the selected write; it says
/// nothing about the filesystem's behavior after a crash or power loss.
#[derive(Default)]
struct Store {
    policy: Vec<u8>,
    sidecar: Option<Vec<u8>>,
    recovery: Vec<Vec<u8>>,
}

impl Store {
    fn apply(
        &mut self,
        plan: &history::Plan,
        fail: Option<history::Step>,
    ) -> (bool, Vec<history::Step>) {
        let mut steps = Vec::new();
        let result = history::execute(plan, |step, bytes| {
            steps.push(step);
            if Some(step) == fail {
                anyhow::bail!("injected write failure");
            }
            match step {
                history::Step::Recovery => self.recovery.push(bytes.to_vec()),
                history::Step::Sidecar => self.sidecar = Some(bytes.to_vec()),
                history::Step::Policy => self.policy = bytes.to_vec(),
            }
            Ok(())
        });
        (result.is_ok(), steps)
    }
}

/// The real start decode path, with the policy file in a temporary folder.
#[test]
fn invalid_saved_network_denies_even_with_matching_allow_rule() {
    let allow = rule(firewall::BY_USER);
    let rules = std::slice::from_ref(&allow);
    let conn = Conn::new("Editor", "api.example", 443);
    assert_eq!(
        firewall::decide(Mode::Ask, None, rules, &conn),
        Decision::Allow(Some(allow.id.clone()))
    );
    for (bytes, word) in [
        (br#"{"v":1,"network":"none"}"#.as_slice(), "none"),
        (br#"{"v":1,"network":"nond"}"#.as_slice(), "nond"),
    ] {
        let t = tempfile::tempdir().unwrap();
        let p = Profile::new(t.path());
        std::fs::write(path(&p), bytes).unwrap();
        let (g, warning) = load_for_start(&p);
        // Only a checked word reaches Live, the record and gate::build.
        assert_eq!(g.network.as_deref(), Some("none"), "saved {word:?}");
        let mode = crate::macvm::gate::startup_mode(g.network.as_deref().unwrap());
        let decision = firewall::decide(mode, None, rules, &conn);
        assert!(
            mode == Mode::None && matches!(&decision, Decision::Deny(_)),
            "saved network {word:?} selected {mode:?} and {decision:?}"
        );
        if word == "none" {
            assert_eq!(warning, None);
            continue;
        }
        let warning = warning.expect("an unknown word warns");
        assert!(warning.contains("\"nond\""), "{warning}");
        assert!(warning.contains("network is none"), "{warning}");
        assert!(!g.clipboard.to_app && !g.clipboard.from_app);
        // No unrelated save persists it, and a network change cannot build
        // its inverse from it.
        assert!(update(&p, |_| Ok(())).is_err());
        assert_eq!(std::fs::read(path(&p)).unwrap(), bytes);
        // The firewall's own fallback is closed too.
        assert_eq!(crate::macvm::gate::startup_mode(word), Mode::None);
    }
}

#[test]
fn legacy_policy_and_writers_preserve_none_with_allow_rules() {
    let mut g = policy();
    history::record(&mut g, historical(6, "add"));
    let plan = history::plan(&g, &history::Read::Missing).unwrap();
    let old = legacy::load(&plan.policy).unwrap();
    assert_eq!(legacy::startup_network(&old), "none");
    assert!(old.shares[0].read_only && old.shares[0].unprotected);
    assert!(old.clipboard.to_app && !old.clipboard.from_app);
    assert_eq!(old.changes.len(), 1);
    for by in [firewall::BY_PROFILE, firewall::BY_USER] {
        let allow = rule(by);
        let conn = Conn::new("Editor", "api.example", 443);
        assert_eq!(
            firewall::decide(
                Mode::parse(legacy::startup_network(&old)).unwrap(),
                None,
                std::slice::from_ref(&allow),
                &conn,
            ),
            Decision::Deny(None)
        );
        assert!(matches!(
            firewall::decide(Mode::Ask, None, &[allow], &conn),
            Decision::Allow(_)
        ));
    }
    let rewritten = legacy::update(&plan.policy, |old| old.clipboard.to_app = false).unwrap();
    let read = new_read(&rewritten, plan.sidecar.as_deref(), false);
    assert_eq!(read.network.as_deref(), Some("none"));
    assert!(!read.clipboard.to_app);
    assert_eq!(read.history_changes, [historical(6, "add")]);
    assert!(read.changes.iter().all(|c| c.applies != Applies::Unknown));
    let (changes, history) = recent_compatible(&read);
    let _: Vec<legacy::Change> =
        serde_json::from_value(serde_json::to_value(changes).unwrap()).unwrap();
    assert_eq!(history, [historical(6, "add")]);
}

#[test]
fn unreadable_policy_fails_closed_and_history_cannot_change_permissions() {
    let fallback = unavailable_policy();
    assert_eq!(fallback.network.as_deref(), Some("none"));
    assert!(!fallback.clipboard.to_app && !fallback.clipboard.from_app);
    let raw = serde_json::to_vec(&policy()).unwrap();
    for (bytes, state) in [
        (None, HistoryState::Missing),
        (Some(b"{broken".as_slice()), HistoryState::Damaged),
    ] {
        let mut read = new_read(&raw, bytes, false);
        assert_eq!(read.history_state, state);
        read.clipboard.to_app = false;
        let plan = history::plan(&read, &history::decode(bytes)).unwrap();
        assert!(plan.sidecar.is_none() && plan.recovery.is_none());
        let old = legacy::load(&plan.policy).unwrap();
        assert_eq!(legacy::startup_network(&old), "none");
        assert!(!old.clipboard.to_app);
        if state == HistoryState::Damaged {
            history::record(&mut read, historical(9, "remove"));
            let plan = history::plan(&read, &history::decode(bytes)).unwrap();
            assert!(plan.sidecar.is_none());
            assert!(plan.skipped_history.is_some());
            assert_eq!(
                legacy::startup_network(&legacy::load(&plan.policy).unwrap()),
                "none"
            );
        }
    }
}

#[test]
fn checkpoint_recontamination_requires_quiescent_remigration() {
    let raw = checkpoint(historical(6, "add"));
    let read = new_read(&raw, None, false);
    let first = history::plan(&read, &history::Read::Missing).unwrap();
    assert_eq!(first.recovery.as_deref(), Some(raw.as_slice()));
    assert_eq!(
        legacy::startup_network(&legacy::load(&first.policy).unwrap()),
        "none"
    );

    // The incompatible checkpoint writer can append again after migration.
    let mut stale: Grants = serde_json::from_slice(&first.policy).unwrap();
    stale.changes.push(historical(7, "add"));
    let recontaminated = serde_json::to_vec(&stale).unwrap();
    let (fallback, warned) = legacy::load_for_start(&recontaminated);
    assert!(warned);
    assert_eq!(legacy::startup_network(&fallback), "ask");
    let current = new_read(&recontaminated, None, true);
    assert_eq!(current.network.as_deref(), Some("none"));
    assert_eq!(current.history_state, HistoryState::Unresolved);
    let fixed = history::plan(&current, &history::Read::Missing).unwrap();
    assert_eq!(fixed.recovery.as_deref(), Some(recontaminated.as_slice()));
    assert_eq!(
        legacy::startup_network(&legacy::load(&fixed.policy).unwrap()),
        "none"
    );
}

#[test]
fn full_sidecar_does_not_block_restrictive_policy_or_discard_evidence() {
    let repeated = historical(6, "add");
    let side = sidecar(&vec![repeated.clone(); CHANGES_KEPT]);
    let mut old: Grants = serde_json::from_slice(&checkpoint(repeated)).unwrap();
    old.network = Some("ask".into());
    let raw = serde_json::to_vec(&old).unwrap();
    let mut read = new_read(&raw, Some(&side), false);
    assert_eq!(read.history_state, HistoryState::Unresolved);
    read.network = Some("none".into());
    history::record(&mut read, historical(7, "remove"));
    let plan = history::plan(&read, &history::decode(Some(&side))).unwrap();
    assert!(plan.sidecar.is_none());
    assert!(plan.skipped_history.is_some());
    let mut store = Store {
        policy: raw.clone(),
        sidecar: Some(side.clone()),
        ..Store::default()
    };
    assert_eq!(
        store.apply(&plan, None),
        (true, vec![history::Step::Recovery, history::Step::Policy])
    );
    assert_eq!(store.recovery, [raw]);
    assert_eq!(store.sidecar.as_deref(), Some(side.as_slice()));
    assert_eq!(
        legacy::startup_network(&legacy::load(&store.policy).unwrap()),
        "none"
    );
    let mut read = new_read(&store.policy, store.sidecar.as_deref(), true);
    assert_eq!(read.history_state, HistoryState::Unresolved);
    assert_eq!(read.changes.len(), 1);
    history::record(&mut read, historical(8, "add"));
    let next = history::plan(&read, &history::decode(Some(&side))).unwrap();
    assert!(next.sidecar.is_none() && next.skipped_history.is_some());
}

#[test]
fn failed_sidecar_write_does_not_undo_policy() {
    let mut g = policy();
    history::record(&mut g, historical(6, "add"));
    let plan = history::plan(&g, &history::Read::Missing).unwrap();
    let mut store = Store::default();
    assert_eq!(
        store.apply(&plan, Some(history::Step::Sidecar)),
        (true, vec![history::Step::Policy, history::Step::Sidecar])
    );
    assert!(store.sidecar.is_none());
    assert_eq!(
        legacy::startup_network(&legacy::load(&store.policy).unwrap()),
        "none"
    );
    let warning = history::execute(&plan, |step, _| {
        if step == history::Step::Sidecar {
            anyhow::bail!("injected failure");
        }
        Ok(())
    })
    .unwrap();
    assert!(warning.unwrap().contains("injected failure"));
}

#[test]
fn identical_sources_and_interrupted_writes_are_not_deduplicated() {
    let event = historical(6, "add");
    let side = sidecar(std::slice::from_ref(&event));
    let raw = checkpoint(event.clone());
    let read = new_read(&raw, Some(&side), false);
    assert_eq!(read.history_state, HistoryState::Unresolved);
    assert_eq!(read.history_changes, [event.clone(), event]);
    let plan = history::plan(&read, &history::decode(Some(&side))).unwrap();
    assert_eq!(plan.recovery.as_deref(), Some(raw.as_slice()));
    assert!(plan.sidecar.is_none());
    let mut store = Store {
        policy: raw.clone(),
        sidecar: Some(side.clone()),
        ..Store::default()
    };
    assert!(!store.apply(&plan, Some(history::Step::Recovery)).0);
    assert_eq!(store.policy, raw);
    assert!(store.recovery.is_empty());
    assert_eq!(store.sidecar.as_deref(), Some(side.as_slice()));
    assert!(!store.apply(&plan, Some(history::Step::Policy)).0);
    assert_eq!(store.policy, raw);
    assert_eq!(store.recovery, std::slice::from_ref(&raw));
    assert_eq!(store.sidecar.as_deref(), Some(side.as_slice()));
    assert!(store.apply(&plan, None).0);
    // Retrying after an interrupted write preserves both snapshots without
    // deciding whether they describe one occurrence or several.
    assert_eq!(store.recovery, [raw.clone(), raw]);
    assert_eq!(store.sidecar.as_deref(), Some(side.as_slice()));
}

#[test]
fn damaged_sidecar_does_not_block_checkpoint_recovery() {
    let raw = checkpoint(historical(6, "add"));
    let damaged = b"{broken".to_vec();
    let mut read = new_read(&raw, Some(&damaged), false);
    read.network = Some("none".into());
    let plan = history::plan(&read, &history::decode(Some(&damaged))).unwrap();
    let mut store = Store {
        policy: raw.clone(),
        sidecar: Some(damaged.clone()),
        ..Store::default()
    };
    assert!(store.apply(&plan, None).0);
    assert_eq!(store.recovery, [raw]);
    assert_eq!(store.sidecar, Some(damaged));
    assert_eq!(
        legacy::startup_network(&legacy::load(&store.policy).unwrap()),
        "none"
    );
}

#[test]
fn failed_recovery_sync_stops_before_policy_write() {
    let raw = checkpoint(historical(6, "add"));
    let read = new_read(&raw, None, false);
    let plan = history::plan(&read, &history::Read::Missing).unwrap();
    let mut snapshot = None;
    let mut policy_written = false;
    let result = history::execute(&plan, |step, bytes| {
        if step == history::Step::Recovery {
            snapshot = Some(bytes.to_vec());
            anyhow::bail!("injected recovery sync failure");
        }
        policy_written = true;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(snapshot, Some(raw));
    assert!(!policy_written);
}

#[test]
fn migration_without_original_bytes_refuses_policy_rewrite() {
    let raw = checkpoint(historical(6, "add"));
    let parsed: Grants = serde_json::from_slice(&raw).unwrap();
    let read = assemble(parsed, &history::Read::Missing, None, false);
    assert!(history::plan(&read, &history::Read::Missing).is_err());
}

#[test]
fn earlier_legacy_marker_is_preserved_before_policy_rewrite() {
    let raw = checkpoint(historical(6, "add"));
    let mut marker: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    marker["changes"][1]["applies"] = serde_json::json!("live");
    let rewritten = legacy::update(&serde_json::to_vec(&marker).unwrap(), |_| {}).unwrap();
    let read = new_read(&rewritten, None, false);
    assert!(read.legacy_history_in_policy);
    assert_eq!(read.history_changes, [historical(6, "add")]);
    let plan = history::plan(&read, &history::Read::Missing).unwrap();
    assert_eq!(plan.recovery.as_deref(), Some(rewritten.as_slice()));
    assert_eq!(legacy::load(&plan.policy).unwrap().changes.len(), 1);
}

#[test]
fn history_does_not_crowd_policy_status_or_claim_launcher_refresh() {
    let mut g = policy();
    g.history_changes = vec![historical(20, "add"); CHANGES_SHOWN];
    let (changes, history) = recent_compatible(&g);
    assert_eq!(changes, g.changes);
    assert_eq!(history.len(), CHANGES_SHOWN);
    let event = crate::macvm::events_stream::historical_rule_grant(
        "app",
        &rule("user"),
        "add",
        GrantSource::You,
    );
    assert!(matches!(&event, Event::Stream(stream) if stream.state == StreamState::Lost));
    let encoded = serde_json::to_vec(&Line::at(1, event)).unwrap();
    let old: legacy_stream::Line = serde_json::from_slice(&encoded).unwrap();
    assert!(
        matches!(old.event, legacy_stream::Event::Stream(stream) if stream.state == legacy_stream::StreamState::Lost)
    );
    assert_eq!(
        event_fields(&historical(1, "add")),
        ("grant_history", "grant_history")
    );
}
