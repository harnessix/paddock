use super::super::{get, history, hot_load, parse, Trait};
use super::*;
use crate::horse::{dsh, run};
use serde_json::json;

fn dna(text: &str) -> Dna {
    parse(text).unwrap()
}

#[test]
fn the_three_compatibility_classes_list_what_differs() {
    let host = dna("capabilities = [\"git\"]\n[traits]\nstyle = \"short\"\ntone = \"calm\"\n[policies]\nreview = \"always\"\n");
    // Other traits only, and an equal value: compatible.
    let b = dna("[traits]\nstyle = \"short\"\nlang = \"en\"\n[policies]\nreview = \"always\"\n");
    assert_eq!(check(&host, &b), Compat::Compatible);
    // The same trait keys with other values: listed.
    let b = dna("[traits]\nstyle = \"long\"\ntone = \"loud\"\nlang = \"en\"\n");
    assert_eq!(
        check(&host, &b),
        Compat::RequiresAdaptation(vec!["style".into(), "tone".into()])
    );
    // Another policy value and a capability that the host lacks.
    let b = dna("capabilities = [\"git\", \"net\"]\n[traits]\nstyle = \"long\"\n[policies]\nreview = \"never\"\nnew = \"x\"\n");
    assert_eq!(
        check(&host, &b),
        Compat::Incompatible(vec!["policy review".into(), "capability net".into()])
    );
}

#[test]
fn graft_keeps_the_host_value_and_records_the_donor_value() {
    let host = dna("[traits]\nstyle = \"short\"\n[policies]\nreview = \"always\"\n");
    let donor = dna("[traits]\nstyle = \"long\"\nlang = \"en\"\n[policies]\nci = \"on\"\n");
    let e = Hash::of(b"donor event");
    let g = graft(&host, &donor, "h-donor", &e).unwrap();
    let from = Provenance::Graft {
        from_horse: "h-donor".into(),
        event: e.clone(),
    };
    assert_eq!(g.traits["lang"], Trait::new("en", from.clone()));
    assert_eq!(g.traits["style"].value, "short");
    assert_eq!(g.traits["style"].from, Provenance::Default);
    assert_eq!(
        g.traits["style"].alternatives,
        [Alternative {
            value: "long".into(),
            from: from.clone()
        }]
    );
    assert_eq!(g.policies["review"], "always");
    assert_eq!(g.policies["ci"], "on");
    assert_eq!(g.origin, host.origin);
    // The text form keeps the alternatives, and a second graft adds none.
    assert_eq!(parse(&g.encode()).unwrap(), g);
    assert_eq!(graft(&g, &donor, "h-donor", &e).unwrap(), g);
    // An incompatible donor is refused with the list.
    let bad = dna("capabilities = [\"net\"]\n");
    let err = graft(&host, &bad, "h-bad", &e).unwrap_err().to_string();
    assert!(err.contains("capability net"), "{err}");
}

#[test]
fn a_horse_graft_is_one_event_and_one_entry_per_running_session() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let host = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    run::record_event(&st, &host, "run.begin", "sb-a", json!({})).unwrap();
    let donor = st.new_default_horse().unwrap();
    hot_load(&st, &donor.id, "[traits]\nlang = \"en\"\n").unwrap();
    let to = graft_horses(&st, &host.horse, &donor.id, "combine")
        .unwrap()
        .unwrap();
    let h = st.horse(&host.horse).unwrap();
    assert_eq!(h.dna, to);
    assert_eq!(h.sessions, std::slice::from_ref(&host.session));
    let g = get(&st, &to).unwrap();
    let donor_event = history(&st, &donor.id).unwrap().pop().unwrap().0;
    assert_eq!(
        g.traits["lang"].from,
        Provenance::Graft {
            from_horse: donor.id.clone(),
            event: donor_event
        }
    );
    let events = history(&st, &host.horse).unwrap();
    let (_, e) = events.last().unwrap();
    assert_eq!(e["type"], "dna.graft");
    assert_eq!(e["donor_horse"], json!(donor.id));
    assert_eq!(e["compat"], "compatible");
    let inbox = st.inbox(&host.session).unwrap();
    assert_eq!(inbox.len(), 2);
    assert!(inbox[1].text.contains("lang = en"), "{}", inbox[1].text);
    assert!(inbox[1].text.contains("graft from"), "{}", inbox[1].text);
    // The same graft again changes nothing.
    assert_eq!(
        graft_horses(&st, &host.horse, &donor.id, "combine").unwrap(),
        None
    );
    assert!(graft_horses(&st, &host.horse, &host.horse, "combine").is_err());
}

#[test]
fn an_incompatible_graft_writes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let host = st.new_default_horse().unwrap();
    hot_load(&st, &host.id, "[policies]\nreview = \"always\"\n").unwrap();
    let donor = st.new_default_horse().unwrap();
    hot_load(&st, &donor.id, "[policies]\nreview = \"never\"\n").unwrap();
    let before = st.horse(&host.id).unwrap();
    let events = history(&st, &host.id).unwrap().len();
    let objects = count_objects(t.path());
    let err = graft_horses(&st, &host.id, &donor.id, "combine").unwrap_err();
    assert!(err.to_string().contains("policy review"), "{err}");
    assert_eq!(st.horse(&host.id).unwrap(), before);
    assert_eq!(history(&st, &host.id).unwrap().len(), events);
    assert_eq!(count_objects(t.path()), objects);
}

fn count_objects(root: &std::path::Path) -> usize {
    std::fs::read_dir(root.join("objects"))
        .unwrap()
        .map(|d| std::fs::read_dir(d.unwrap().path()).unwrap().count())
        .sum()
}
