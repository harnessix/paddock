use super::super::{get, history};
use super::*;
use crate::horse::{dsh, run, Store};
use serde_json::json;

fn outcome(ok: bool, harness: &str) -> Outcome {
    Outcome {
        run_ok: ok,
        harness: harness.into(),
        nodes: 3,
        tools: Vec::new(),
    }
}

#[test]
fn two_runs_set_runs_harnesses_and_reliability() {
    let e1 = Hash::of(b"run 1");
    let e2 = Hash::of(b"run 2");
    let d = adapt(&Dna::default(), &outcome(true, "dsh"), &e1).unwrap();
    assert_eq!(d.traits["runs"].value, "1");
    assert_eq!(d.traits["reliability"].value, "1.00");
    let d = adapt(&d, &outcome(false, "claude"), &e2).unwrap();
    assert_eq!(d.traits["runs"].value, "2");
    assert_eq!(d.traits["reliability"].value, "0.50");
    assert_eq!(d.traits["harnesses"].value, "claude, dsh");
    assert_eq!(
        d.traits["runs"].from,
        Provenance::Adapt {
            event: e2.clone(),
            rule: "runs".into()
        }
    );
    assert_eq!(
        d.traits["reliability"].from,
        Provenance::Adapt {
            event: e2.clone(),
            rule: "reliability 1/2".into()
        }
    );
    // The set of harnesses stays sorted and without repeats.
    let d = adapt(&d, &outcome(true, "dsh"), &e1).unwrap();
    assert_eq!(d.traits["harnesses"].value, "claude, dsh");
    assert_eq!(d.traits["reliability"].value, "0.67");
    // The counts stay exact through the rule name, past 100 runs.
    let mut d = d;
    for _ in 0..297 {
        d = adapt(&d, &outcome(true, "dsh"), &e1).unwrap();
    }
    assert_eq!(d.traits["runs"].value, "300");
    assert_eq!(
        d.traits["reliability"].from,
        Provenance::Adapt {
            event: e1.clone(),
            rule: "reliability 299/300".into()
        }
    );
}

#[test]
fn a_run_without_nodes_changes_nothing() {
    let d = adapt(&Dna::default(), &outcome(true, "dsh"), &Hash::of(b"r")).unwrap();
    let empty = Outcome {
        nodes: 0,
        ..outcome(true, "dsh")
    };
    assert_eq!(adapt(&d, &empty, &Hash::of(b"r2")), None);
}

#[test]
fn the_end_of_a_run_adapts_the_dna_with_one_event() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let rh = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let root = t.path().join("sessions");
    let rows = crate::horse::live::fake::turn_rows(0, 1, "hello", "m1");
    crate::horse::live::fake::write_file(&root, "session-test", &rows);
    st.lock()
        .unwrap()
        .bind_native(&rh.session, "session-test")
        .unwrap();
    run::record_event(&st, &rh, "run.begin", "sandbox", json!({})).unwrap();
    run::end_dsh(&st, &rh, "sandbox", &root, true).unwrap();
    let events = history(&st, &rh.horse).unwrap();
    let (_, e) = events.last().unwrap();
    assert_eq!(e["type"], "dna.adapt");
    assert_eq!(e["rules"], json!(["harnesses", "reliability", "runs"]));
    let end = run::events(&st, &rh.horse)
        .unwrap()
        .into_iter()
        .position(|e| e["type"] == "run.end")
        .unwrap();
    assert!(end > 0);
    let d = get(&st, &st.horse(&rh.horse).unwrap().dna).unwrap();
    assert_eq!(d.traits["runs"].value, "1");
    assert_eq!(d.traits["harnesses"].value, "dsh");
    assert_eq!(d.traits["reliability"].value, "1.00");
    // The run event is the event of the provenance.
    let Provenance::Adapt { event, .. } = &d.traits["runs"].from else {
        panic!("{:?}", d.traits["runs"].from);
    };
    let node: serde_json::Value = serde_json::from_slice(&st.get(event).unwrap().raw).unwrap();
    assert_eq!(node["type"], "run.end");
}
