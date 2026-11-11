use super::*;
use crate::horse::dna::{history, hot_load};
use crate::horse::{dsh, Binding};

fn binding() -> Binding {
    Binding {
        harness: dsh::HARNESS.into(),
        native_id: None,
        version: dsh::VERSION.into(),
        container: dsh::CONTAINER.into(),
    }
}

#[test]
fn move_session_closes_and_opens_lineage_steps() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let a = st.new_default_horse().unwrap();
    let b = st.new_default_horse().unwrap();
    let s = st.new_session(&a.id, binding()).unwrap();
    let before = crate::horse::now_ms();
    let m = st.move_session(&s.id, &b.id).unwrap();
    assert_eq!(m.horse, b.id);
    assert_eq!(m.lineage.len(), 2);
    let (old, new) = (&m.lineage[0], &m.lineage[1]);
    assert_eq!(old.horse, a.id);
    assert!(old.until_ms.unwrap() >= before);
    assert_eq!(new.horse, b.id);
    assert_eq!(new.dna, b.dna);
    assert_eq!(new.since_ms, old.until_ms.unwrap());
    assert_eq!(new.until_ms, None);
    assert!(st.horse(&a.id).unwrap().sessions.is_empty());
    assert_eq!(
        st.horse(&b.id).unwrap().sessions,
        std::slice::from_ref(&s.id)
    );
    assert!(st.move_session(&s.id, &b.id).is_err());
}

#[test]
fn a_move_grafts_the_source_dna_and_the_guard_records_it_as_safe() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let src = st.new_default_horse().unwrap();
    hot_load(&st, &src.id, "[traits]\nlang = \"en\"\n").unwrap();
    let target = st.new_default_horse().unwrap();
    hot_load(&st, &target.id, "[traits]\nstyle = \"short\"\n").unwrap();
    let s = st.new_session(&src.id, binding()).unwrap();
    // A session with rows, so the guard can see its prompt.
    let rows = crate::horse::live::fake::turn_rows(0, 1, "hello", "m1");
    let root = t.path().join("sessions");
    crate::horse::live::fake::write_file(&root, "session-move", &rows);
    let dir = crate::horse::live::custody::find_session_dir(&root, "session-move").unwrap();
    dsh::ingest(&st, &dir, dsh::VERSION, Some(&src.id)).unwrap();
    let rowed = st
        .sessions()
        .unwrap()
        .into_iter()
        .find(|x| x.binding.native_id.as_deref() == Some("session-move"))
        .unwrap();

    let m = move_to(&st, &rowed.id, &target.id).unwrap();
    assert_eq!(m.from, src.id);
    let v = m.verdict.expect("a verdict for a session with rows");
    assert_eq!(v.op, format!("a move to horse {}", target.id));
    let saved = st.guard_state(&rowed.id).unwrap().last.unwrap();
    assert_eq!(saved, v);
    // The fake rows are old, so only the cache age loses the prefix. A
    // move soon after the last request keeps it.
    let last = guard::tuple(&st, &st.session(&rowed.id).unwrap())
        .unwrap()
        .last_request_ms
        .unwrap();
    let op = Op::MoveHorse {
        to: target.id.clone(),
    };
    let soon = guard::check(&st, &rowed.id, &op, last + 60_000).unwrap();
    assert!(soon.keeps, "{}", soon.message());
    // The target gained the trait of the source.
    let now = st.horse(&target.id).unwrap();
    assert_eq!(Some(&now.dna), m.graft.as_ref());
    let d = crate::horse::dna::get(&st, &now.dna).unwrap();
    assert_eq!(d.traits["lang"].value, "en");
    assert_eq!(d.traits["style"].value, "short");
    // The new lineage step names the DNA after the graft.
    assert_eq!(m.session.lineage.last().unwrap().dna, now.dna);
    let last = history(&st, &target.id).unwrap().pop().unwrap().1;
    assert_eq!(last["type"], "dna.graft");
    assert_eq!(last["reason"], format!("move {}", rowed.id));
    // One system entry with the DNA of the new horse.
    let inbox = st.inbox(&rowed.id).unwrap();
    let e = inbox.last().unwrap();
    assert_eq!(e.role, "system");
    assert!(e.text.contains("lang = en") && e.text.contains("style = short"));
    for h in [&src.id, &target.id] {
        let moves = crate::horse::run::events(&st, h)
            .unwrap()
            .into_iter()
            .filter(|e| e["type"] == "session.move")
            .count();
        assert_eq!(moves, 1, "{h}");
    }
    // The other session stays in the source horse.
    assert_eq!(st.session(&s.id).unwrap().horse, src.id);
}

#[test]
fn an_incompatible_or_running_session_does_not_move() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let src = st.new_default_horse().unwrap();
    hot_load(&st, &src.id, "capabilities = [\"net\"]\n").unwrap();
    let target = st.new_default_horse().unwrap();
    let s = st.new_session(&src.id, binding()).unwrap();
    let err = move_to(&st, &s.id, &target.id).unwrap_err();
    assert!(err.to_string().contains("capability net"), "{err}");
    assert_eq!(st.session(&s.id).unwrap().horse, src.id);
    assert_eq!(history(&st, &target.id).unwrap().len(), 1);

    let rh = crate::horse::run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    crate::horse::run::record_event(&st, &rh, "run.begin", "sb", serde_json::json!({})).unwrap();
    let err = move_to(&st, &rh.session, &target.id).unwrap_err();
    assert!(err.to_string().contains("runs now"), "{err}");
}
