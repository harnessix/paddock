//! `paddock run dsh --horse-id`: the run continues the horse and resumes
//! its newest dsh session.
use super::*;
use crate::horse::live::fake;

/// A dsh run in horse mode whose dsh writes one turn into its session
/// file. Returns the horse and the dsh id of its session.
fn first_run(dir: &std::path::Path) -> (HostEnv, String, String) {
    let (env, mut b) = super::tests::setup(dir, "dsh", dsh::VERSION);
    b.horse = true;
    let l = start(&env, &mut b).unwrap().unwrap();
    let set = live::take_device_setup().expect("an armed dsh custody");
    let _ = live::take_hook();
    let rows = fake::turn_rows(0, 1, "hello", "m1");
    fake::write_file(&b.spec.home_src.join(".dsh/sessions"), &set.main, &rows);
    drop(l);
    (env, set.horse, set.main)
}

#[test]
fn a_run_with_a_horse_id_resumes_the_newest_session_of_that_horse() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, horse, native) = first_run(t.path());
    let store = Store::open_host(&env).unwrap();
    let session = store.horse(&horse).unwrap().sessions[0].clone();

    let (_, mut b) = super::tests::setup(t.path(), "dsh", dsh::VERSION);
    b.horse = true;
    b.spec.intent.horse_id = Some(horse.clone());
    let l = start(&env, &mut b).unwrap().unwrap();
    let set = live::take_device_setup().expect("an armed dsh custody");
    let _ = live::take_hook();
    assert_eq!(set.horse, horse);
    assert_eq!(set.main, native);
    assert!(set.resume);
    drop(l);

    assert_eq!(store.horses().unwrap().len(), 1);
    assert_eq!(
        store.horse(&horse).unwrap().sessions,
        std::slice::from_ref(&session)
    );
    let begins: Vec<String> = super::tests::run_events(&store, &horse)
        .unwrap()
        .into_iter()
        .filter(|e| e["type"] == "run.begin")
        .map(|e| e["session"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(begins, [session.clone(), session]);
}

#[test]
fn a_horse_id_fails_before_the_run_for_an_unknown_horse_or_another_harness() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, horse, _) = first_run(t.path());
    let store = Store::open_host(&env).unwrap();

    let (_, mut b) = super::tests::setup(t.path(), "dsh", dsh::VERSION);
    b.horse = true;
    b.spec.intent.horse_id = Some("h-000000000000".into());
    assert!(start(&env, &mut b).is_err());
    assert!(live::take_hook().is_none());

    let (_, mut b) = super::tests::setup(t.path(), "claude", claude::VERSION);
    b.horse = true;
    b.spec.intent.horse_id = Some(horse.clone());
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("--horse-id works with dsh runs only"), "{e}");
    assert_eq!(store.horses().unwrap().len(), 1);
    assert_eq!(store.horse(&horse).unwrap().sessions.len(), 1);
}

#[test]
fn a_horse_without_a_dsh_session_gets_a_new_one_with_its_dna_entry() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "dsh", dsh::VERSION);
    let store = Store::open_host(&env).unwrap();
    let horse = store.new_default_horse().unwrap().id;
    b.horse = true;
    b.spec.intent.horse_id = Some(horse.clone());
    let l = start(&env, &mut b).unwrap().unwrap();
    let set = live::take_device_setup().expect("an armed dsh custody");
    let _ = live::take_hook();
    assert!(!set.resume);
    let sessions = store.horse(&horse).unwrap().sessions;
    assert_eq!(sessions.len(), 1);
    let s = store.session(&sessions[0]).unwrap();
    assert_eq!(s.binding.native_id.as_deref(), Some(set.main.as_str()));
    assert_eq!(store.inbox(&s.id).unwrap()[0].role, "system");
    drop(l);
}
