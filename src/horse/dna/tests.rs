use super::*;
use crate::horse::{dsh, run};

fn store() -> (tempfile::TempDir, Store) {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    (t, st)
}

fn types(st: &Store, horse: &str) -> Vec<String> {
    history(st, horse)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e["type"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn parse_and_encode_are_canonical() {
    let a = parse(
        "capabilities = [\"net\", \"git\", \"net\"]\n[traits]\nstyle = \"short\"\ntone = { value = \"calm\", from = \"default\" }\n[policies]\nz = \"1\"\na = \"2\"\n",
    )
    .unwrap();
    let b = parse(
        "[policies]\na = \"2\"\nz = \"1\"\n[traits.tone]\nvalue = \"calm\"\n[traits.style]\nvalue = \"short\"\n[origin]\ncreated_ms = 0\n\ncapabilities = []\n",
    );
    // A value after a table belongs to the table: `capabilities` there is
    // an unknown key of `origin`, so the text is refused.
    assert!(b.is_err());
    let b = parse(
        "capabilities = [\"git\", \"net\"]\n[policies]\na = \"2\"\nz = \"1\"\n[traits.tone]\nvalue = \"calm\"\n[traits.style]\nvalue = \"short\"\n",
    )
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(a.encode(), b.encode());
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.capabilities, ["git", "net"]);
    // The encoded text reads back to the same DNA.
    assert_eq!(parse(&a.encode()).unwrap(), a);
    let (_t, st) = store();
    assert_eq!(put(&st, &a).unwrap(), a.hash());
    assert_eq!(get(&st, &a.hash()).unwrap(), a);
    // Provenance survives the text form.
    let mut c = a.clone();
    c.traits.get_mut("tone").unwrap().from = Provenance::Inherit {
        from_horse: "h-1".into(),
        event: Hash::of(b"e"),
    };
    assert_eq!(parse(&c.encode()).unwrap(), c);
}

#[test]
fn inherit_copies_traits_with_provenance() {
    let mut p = parse("[traits]\nstyle = \"short\"\n[policies]\nreview = \"always\"\n").unwrap();
    p.capabilities = vec!["git".into()];
    let e = Hash::of(b"event");
    let c = inherit(&p, "h-parent", &e);
    assert_eq!(c.origin.parent.as_deref(), Some("h-parent"));
    assert_eq!(c.origin.parent_dna, Some(p.hash()));
    assert_eq!(c.traits["style"].value, "short");
    assert_eq!(
        c.traits["style"].from,
        Provenance::Inherit {
            from_horse: "h-parent".into(),
            event: e
        }
    );
    assert_eq!(c.policies, p.policies);
    assert_eq!(c.capabilities, p.capabilities);
}

#[test]
fn invalid_text_is_refused_and_the_horse_keeps_its_hash() {
    let (_t, st) = store();
    let h = st.new_default_horse().unwrap();
    for bad in ["not = [toml", "[traits]\nx = 1\n", "unknown = \"key\"\n"] {
        assert!(hot_load(&st, &h.id, bad).is_err(), "{bad}");
    }
    assert_eq!(st.horse(&h.id).unwrap().dna, h.dna);
    assert_eq!(types(&st, &h.id), ["dna.default"]);
    // The same DNA again is no change and no event.
    let same = get(&st, &h.dna).unwrap().encode();
    assert_eq!(hot_load(&st, &h.id, &same).unwrap(), None);
    assert_eq!(types(&st, &h.id), ["dna.default"]);
}

#[test]
fn a_hot_load_writes_one_event_and_one_entry_per_running_session() {
    let (_t, st) = store();
    let a = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let b = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, Some(&a)).unwrap();
    let ended = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, Some(&a)).unwrap();
    run::record_event(&st, &a, "run.begin", "sb-a", json!({})).unwrap();
    run::record_event(&st, &b, "run.begin", "sb-b", json!({})).unwrap();
    run::record_event(&st, &ended, "run.begin", "sb-c", json!({})).unwrap();
    run::record_event(&st, &ended, "run.end", "sb-c", json!({})).unwrap();
    // The first entry of every session is the DNA at the start.
    for s in [&a, &b, &ended] {
        let inbox = st.inbox(&s.session).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].role, "system");
        assert!(inbox[0].text.contains("no traits"), "{}", inbox[0].text);
    }
    let old = st.horse(&a.horse).unwrap().dna;
    let to = hot_load(&st, &a.horse, "[traits]\nstyle = \"careful\"\n")
        .unwrap()
        .unwrap();
    assert_eq!(st.horse(&a.horse).unwrap().dna, to);
    let d = get(&st, &to).unwrap();
    let input = &history(&st, &a.horse).unwrap()[1].1["input"];
    assert_eq!(
        d.traits["style"].from,
        Provenance::Edit {
            event: Hash::parse(input.as_str().unwrap()).unwrap()
        }
    );
    assert_eq!(types(&st, &a.horse), ["dna.default", "dna.edit"]);
    let (_, e) = &history(&st, &a.horse).unwrap()[1];
    assert_eq!(e["from_hash"], json!(old));
    assert_eq!(e["to_hash"], json!(to));
    for (s, n) in [(&a, 2), (&b, 2), (&ended, 1)] {
        let inbox = st.inbox(&s.session).unwrap();
        assert_eq!(inbox.len(), n, "{}", s.session);
        if n == 2 {
            assert_eq!(inbox[1].role, "system");
            assert!(inbox[1].text.contains("style = careful"));
            assert!(inbox[1].text.contains("hot-load"));
        }
    }
    // The old DNA stays in the store, and a restore brings its hash back.
    assert!(get(&st, &old).is_ok());
    restore(&st, &a.horse, &old).unwrap();
    assert_eq!(st.horse(&a.horse).unwrap().dna, old);
    assert_eq!(
        types(&st, &a.horse),
        ["dna.default", "dna.edit", "dna.revert"]
    );
    // An unchanged trait keeps its provenance through an edit.
    let to = hot_load(&st, &a.horse, "[traits]\nstyle = \"careful\"\n")
        .unwrap()
        .unwrap();
    let first = get(&st, &to).unwrap();
    let to2 = hot_load(
        &st,
        &a.horse,
        "[traits]\nstyle = { value = \"careful\", from = \"default\" }\ntone = \"calm\"\n",
    )
    .unwrap()
    .unwrap();
    let second = get(&st, &to2).unwrap();
    assert_eq!(second.traits["style"].from, first.traits["style"].from);
    assert_ne!(second.traits["tone"].from, first.traits["style"].from);
    assert_eq!(second.origin, first.origin);
}

#[test]
fn render_entry_is_stable_text() {
    assert_eq!(
        render_entry(&Dna::default(), "start"),
        "DNA of this horse (start)\ntraits: none yet, this horse has no traits\npolicies: none"
    );
    let mut d = parse("[traits]\nb = \"2\"\na = \"1\"\n[policies]\np = \"q\"\n").unwrap();
    d.capabilities = vec!["git".into()];
    assert_eq!(
        render_entry(&d, "hot-load"),
        "DNA of this horse (hot-load)\ntraits: a = 1, b = 2\npolicies: p = q\ncapabilities (recorded only, they grant nothing): git"
    );
}

#[test]
fn a_legacy_dna_reads_as_empty() {
    let (_t, st) = store();
    let h = st
        .put_blob("toml", b"# horse DNA\norigin = \"paddock run\"\n")
        .unwrap();
    assert_eq!(get(&st, &h).unwrap(), Dna::default());
    let other = st.put_blob("text/plain", b"x").unwrap();
    assert!(get(&st, &other).is_err());
}
