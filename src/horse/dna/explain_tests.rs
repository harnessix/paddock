use super::super::adapt::{adapt, Outcome};
use super::super::{graft::graft_horses, hot_load, new_child_horse_locked, put};
use super::*;
use crate::horse::{dsh, Binding};

#[test]
fn explain_names_each_provenance_and_revert_restores_a_hash() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let parent = st.new_default_horse().unwrap();
    hot_load(&st, &parent.id, "[traits]\nstyle = \"short\"\n").unwrap();
    let child = new_child_horse_locked(&st.lock().unwrap(), &parent.id).unwrap();
    let session = st
        .new_session(
            &child.id,
            Binding {
                harness: dsh::HARNESS.into(),
                native_id: None,
                version: dsh::VERSION.into(),
                container: dsh::CONTAINER.into(),
            },
        )
        .unwrap();
    let donor = st.new_default_horse().unwrap();
    hot_load(&st, &donor.id, "[traits]\nlang = \"en\"\n").unwrap();
    graft_horses(&st, &child.id, &donor.id, "combine")
        .unwrap()
        .unwrap();
    // An adapt step, as the end of a run makes it.
    let run_end = Hash::of(b"a run.end event");
    let o = Outcome {
        run_ok: true,
        harness: "dsh".into(),
        nodes: 4,
        tools: Vec::new(),
    };
    let current = get(&st, &st.horse(&child.id).unwrap().dna).unwrap();
    let adapted = put(&st, &adapt(&current, &o, &run_end).unwrap()).unwrap();
    let why = DnaEvent::Adapt {
        rules: vec!["runs".into()],
        run_event: run_end,
    };
    set_horse_dna(&st, &child.id, &adapted, why).unwrap();
    let edit = "[traits]\nstyle = \"short\"\nlang = \"en\"\ntone = \"calm\"\n";
    hot_load(&st, &child.id, edit).unwrap().unwrap();
    assert!(explain(&st, &child.id)
        .unwrap()
        .contains("tone = calm  ← edit ("));

    revert(&st, &child.id, &adapted).unwrap();
    assert_eq!(st.horse(&child.id).unwrap().dna, adapted);
    let kinds: Vec<String> = history(&st, &child.id)
        .unwrap()
        .iter()
        .map(|(_, e)| e["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        [
            "dna.inherit",
            "dna.graft",
            "dna.adapt",
            "dna.edit",
            "dna.revert"
        ]
    );
    let text = explain(&st, &child.id).unwrap();
    let line = |key: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(&format!("{key} = ")))
            .unwrap_or_else(|| panic!("no {key}: {text}"))
            .to_string()
    };
    assert!(line("style").contains(&format!("← inherit from {} (", parent.id)));
    assert!(line("lang").contains(&format!("← graft from {} (", donor.id)));
    assert!(line("runs").contains("← adapt runs ("));
    assert!(line("reliability").contains("← adapt reliability 1/1 ("));
    assert!(!text.contains("tone = calm"), "{text}");
    for p in ["← inherit", "← graft", "← adapt runs", "← adapt harnesses"] {
        assert_eq!(text.matches(p).count(), 1, "{p}: {text}");
    }
    let events = text.split("DNA events, root first:\n").nth(1).unwrap();
    for k in &kinds {
        assert!(events.contains(k.as_str()), "{k}: {text}");
    }
    assert!(text.contains(&format!("lineage of session {}:", session.id)));

    // Revert takes only an earlier DNA of the horse, and not the current one.
    let other = st.horse(&donor.id).unwrap().dna;
    assert!(revert(&st, &child.id, &other).is_err());
    assert!(revert(&st, &child.id, &adapted).is_err());
    assert_eq!(history(&st, &child.id).unwrap().len(), 5);
}

#[test]
fn explain_names_the_registry_reference_of_a_used_dna() {
    use crate::horse::registry::{self as reg, FileIndex, Manifest, Transport};
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap();
    let bytes = super::super::parse("[traits]\nlang = \"en\"\n")
        .unwrap()
        .encode()
        .into_bytes();
    let m = Manifest {
        name: "acme/review".into(),
        version: "1.2.0".into(),
        hash: reg::artifact_hash(&bytes),
        parents: Vec::new(),
        created_ms: 1,
        author: None,
        signature: None,
    };
    let idx = FileIndex::new("local", t.path().join("idx"));
    idx.put(&m, &bytes).unwrap();
    let indexes: Vec<Box<dyn Transport>> = vec![Box::new(idx)];
    let trust = reg::Trust {
        require_signature: false,
        keys: Vec::new(),
    };
    // @latest is recorded as the version it resolved to.
    let r = reg::resolve(
        &indexes,
        &"registry://acme/review@latest".parse().unwrap(),
        &trust,
    )
    .unwrap();
    super::super::registry::use_in(&st, &horse.id, &r).unwrap();
    let text = explain(&st, &horse.id).unwrap();
    let fetch = history(&st, &horse.id).unwrap()[1].0.clone();
    let want = format!(
        "lang = en  ← registry://acme/review@1.2.0 ({}, {}, ",
        m.hash.short(),
        fetch.short()
    );
    assert!(text.contains(&want), "{text}");
    assert_eq!(text.matches("← registry://").count(), 1, "{text}");
    assert!(!text.contains("@latest"), "{text}");
}
