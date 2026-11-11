use super::*;
use crate::horse::{store::Origin, Binding, Hash};

fn fixture() -> Result<(tempfile::TempDir, HostEnv, Store, String, Hash)> {
    let temp = tempfile::tempdir()?;
    let env = HostEnv::from_parts(
        temp.path().join("home"),
        "test".into(),
        0,
        0,
        Default::default(),
    );
    let root = crate::horse::root(&env);
    for dir in ["", "objects", "sessions", "horses", "tmp"] {
        crate::privfs::dir(&root.join(dir))?;
    }
    let store = Store::open_plain_for_migration(&root)?;
    let horse = store.new_default_horse()?;
    let session = store.new_session(
        &horse.id,
        Binding {
            harness: "test".into(),
            native_id: None,
            version: "1".into(),
            container: "test".into(),
        },
    )?;
    let origin = Origin {
        harness: "test",
        container: "test",
        version: "1",
    };
    let first = store.put_native(&origin, None, "seq:0", b"migration-canary\0\xff")?;
    let second = store.put_native(&origin, Some(&first), "seq:1", b"second node")?;
    store.update_session(&session.id, |s| s.head = Some(second.clone()))?;
    store.queue_append(&session.id, "user", "migration inbox canary")?;
    Ok((temp, env, store, session.id, second))
}

#[test]
fn migration_verifies_every_node_and_keeps_plaintext_until_explicit_drop() -> Result<()> {
    use crate::horse::registry::{self as horse_registry, FileIndex, Manifest, Transport};
    let (_temp, env, source, session, head) = fixture()?;
    let mut dna = crate::horse::dna::default_for_run();
    dna.traits.insert(
        "tone".into(),
        crate::horse::dna::Trait::new(
            "registry-migration-canary",
            crate::horse::dna::Provenance::Default,
        ),
    );
    let artifact = dna.encode().into_bytes();
    let manifest = Manifest {
        name: "test/dna".into(),
        version: "1.0.0".into(),
        hash: horse_registry::artifact_hash(&artifact),
        parents: vec![],
        created_ms: 1,
        author: None,
        signature: None,
    };
    let local = FileIndex::new("local", env.paddock_state().join("registry/local"));
    local.put(&manifest, &artifact)?;
    let plain_snapshot = snapshot::scan(source.root())?.digest;
    let expected = source.chain(Some(&head))?;
    let count = verify(&env)?;
    assert!(count >= expected.len());
    let backup = backup_path(&crate::horse::root(&env))?;
    assert_eq!(snapshot::scan(&backup)?.digest, plain_snapshot);
    assert!(registry::backup(&env).is_dir());
    let encrypted = Store::open_host(&env)?;
    assert_eq!(encrypted.chain(Some(&head))?, expected);
    assert_eq!(encrypted.inbox(&session)?[0].text, "migration inbox canary");
    let indexes = horse_registry::configured_for_store(&env.paddock_state(), &encrypted)?;
    assert_eq!(indexes[0].object(&manifest.hash)?.unwrap(), artifact);
    let object = std::fs::read(indexes[0].root.join("objects").join(manifest.hash.as_str()))?;
    assert!(!object
        .windows(b"registry-migration-canary".len())
        .any(|w| w == b"registry-migration-canary"));
    assert_eq!(verify(&env)?, count);
    assert!(backup.is_dir());
    let file = backup.join("sessions").join(format!("{session}.json"));
    let original = std::fs::read(&file)?;
    std::fs::write(&file, b"changed")?;
    assert!(drop_plain(&env).is_err());
    assert!(backup.is_dir());
    std::fs::write(&file, original)?;
    encrypted.update_session(&session, |s| s.binding.version = "2".into())?;
    assert_eq!(verify(&env)?, count);
    let archived = format!(
        ".paddock-migration/refs/{}",
        Hash::of(format!("sessions/{session}.json").as_bytes())
    );
    let archived_bytes = encrypted.read_ref(&archived)?.unwrap();
    encrypted.remove_ref(&archived)?;
    assert!(drop_plain(&env).is_err());
    assert!(backup.is_dir());
    encrypted.write_ref(&archived, &archived_bytes)?;
    let object_path = encrypted
        .root()
        .join("objects")
        .join(&head.as_str()[..2])
        .join(&head.as_str()[2..]);
    let ciphertext = std::fs::read(&object_path)?;
    std::fs::remove_file(&object_path)?;
    assert!(drop_plain(&env).is_err());
    assert!(backup.is_dir());
    std::fs::write(&object_path, ciphertext)?;
    assert_eq!(drop_plain(&env)?, count);
    assert!(!backup.exists());
    assert!(!registry::backup(&env).exists());
    assert_eq!(Store::open_host(&env)?.chain(Some(&head))?, expected);
    Ok(())
}

#[test]
fn wrong_or_missing_master_refuses_before_touching_plain_store() -> Result<()> {
    let (_temp, env, store, _, _) = fixture()?;
    let keys = crate::horse::keys::Keyring::for_host(&env);
    keys.initialize()?;
    let master = env.paddock_state().join("keyring/horse/master");
    std::fs::write(&master, crate::horse::keys::Key::generate()?.as_bytes())?;
    let before = snapshot::scan(store.root())?.digest;
    assert!(verify(&env).is_err());
    assert_eq!(snapshot::scan(store.root())?.digest, before);
    assert!(!backup_path(store.root())?.exists());
    assert!(!stage_root(store.root())?.exists());
    std::fs::remove_file(&master)?;
    assert!(verify(&env).is_err());
    assert_eq!(snapshot::scan(store.root())?.digest, before);
    Ok(())
}

#[test]
fn symlinks_are_refused_without_following_or_replacing_them() -> Result<()> {
    use std::os::unix::fs::symlink;
    let (temp, env, store, _, _) = fixture()?;
    let outside = temp.path().join("outside");
    std::fs::write(&outside, b"outside canary")?;
    symlink(&outside, store.root().join("unsafe-link"))?;
    assert!(verify(&env).is_err());
    assert_eq!(std::fs::read(&outside)?, b"outside canary");
    assert!(store.root().join("unsafe-link").is_symlink());
    assert!(!stage_root(store.root())?.exists());
    Ok(())
}

#[test]
fn an_active_legacy_run_stops_migration_without_removing_its_record() -> Result<()> {
    let (_temp, env, store, _, _) = fixture()?;
    let record = crate::horse::live::Record {
        run: "fake-run".into(),
        pid: std::process::id(),
        horse: "h-fake".into(),
        main: "s-fake".into(),
        started_ms: 1,
        updated_ms: 1,
        sessions: vec![],
    };
    store.write_ref("live/fake-run.json", &serde_json::to_vec(&record)?)?;
    let before = snapshot::scan(store.root())?.digest;
    assert!(verify(&env).is_err());
    assert_eq!(snapshot::scan(store.root())?.digest, before);
    assert!(!stage_root(store.root())?.exists());
    Ok(())
}

#[test]
fn an_interrupted_atomic_exchange_can_finish_without_deleting_the_source() -> Result<()> {
    let (_temp, env, source, _, head) = fixture()?;
    let expected = source.chain(Some(&head))?;
    let root = crate::horse::root(&env);
    let stage = stage_root(&root)?;
    let before = snapshot::scan(&root)?;
    std::fs::DirBuilder::new().mode(0o700).create(&stage)?;
    let target = Store::open_migration_stage(&env, &stage)?;
    let nodes = copy::build(&root, &before, &target)?;
    let verified = Verified {
        version: 1,
        source: before.digest,
        registry: None,
        nodes,
    };
    target.write_ref(MARKER, &serde_json::to_vec(&verified)?)?;
    let dir = crate::safefs::Root::open(parent(&root)?)?.dir(Path::new(""), false)?;
    dir.exchange(OsStr::new("horse"), OsStr::new(STAGE))?;
    assert!(stage.is_dir());
    assert_eq!(verify(&env)?, nodes);
    assert!(backup_path(&root)?.is_dir());
    assert!(!stage.exists());
    assert_eq!(Store::open_host(&env)?.chain(Some(&head))?, expected);
    Ok(())
}

#[test]
fn an_interrupted_registry_swap_resumes_without_discarding_either_copy() -> Result<()> {
    use crate::horse::registry::{self as horse_registry, FileIndex, Manifest, Transport};
    let (_temp, env, _source, _, head) = fixture()?;
    let root = crate::horse::root(&env);
    let local = env.paddock_state().join("registry/local");
    let artifact = crate::horse::dna::default_for_run().encode().into_bytes();
    let manifest = Manifest {
        name: "test/dna".into(),
        version: "1.0.0".into(),
        hash: horse_registry::artifact_hash(&artifact),
        parents: vec![],
        created_ms: 1,
        author: None,
        signature: None,
    };
    FileIndex::new("local", &local).put(&manifest, &artifact)?;
    let before = snapshot::scan(&root)?;
    let stage = stage_root(&root)?;
    std::fs::DirBuilder::new().mode(0o700).create(&stage)?;
    let target = Store::open_migration_stage(&env, &stage)?;
    let nodes = copy::build(&root, &before, &target)?;
    let registry = registry::prepare(&env, &target)?;
    let verified = Verified {
        version: 1,
        source: before.digest,
        registry,
        nodes,
    };
    target.write_ref(MARKER, &serde_json::to_vec(&verified)?)?;
    let registry_parent = env.paddock_state().join("registry");
    crate::safefs::Root::open(&registry_parent)?
        .dir(Path::new(""), false)?
        .exchange(OsStr::new("local"), OsStr::new(".local-migrate"))?;
    assert_eq!(verify(&env)?, nodes);
    assert!(backup_path(&root)?.is_dir());
    assert!(registry::backup(&env).is_dir());
    let store = Store::open_host(&env)?;
    assert_eq!(store.get(&head)?.raw, b"second node");
    let indexes = horse_registry::configured_for_store(&env.paddock_state(), &store)?;
    assert_eq!(indexes[0].object(&manifest.hash)?.unwrap(), artifact);
    Ok(())
}

#[test]
fn explicit_plaintext_removal_can_be_retried_after_an_interruption() -> Result<()> {
    let (_temp, env, _store, _, _) = fixture()?;
    let count = verify(&env)?;
    let store = Store::open_host(&env)?;
    let verified = marker(&store)?;
    store.write_ref(DROP_MARKER, &serde_json::to_vec(&verified)?)?;
    let backup = backup_path(&crate::horse::root(&env))?;
    std::fs::remove_dir_all(&backup)?;
    assert_eq!(drop_plain(&env)?, count);
    assert_eq!(drop_plain(&env)?, count);
    Ok(())
}

#[test]
fn drop_plain_refuses_a_replaced_backup_symlink() -> Result<()> {
    use std::os::unix::fs::symlink;
    let (temp, env, _store, _, _) = fixture()?;
    verify(&env)?;
    let backup = backup_path(&crate::horse::root(&env))?;
    let saved = temp.path().join("saved-plain");
    std::fs::rename(&backup, &saved)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("keep"), b"outside")?;
    symlink(&outside, &backup)?;
    assert!(drop_plain(&env).is_err());
    assert_eq!(std::fs::read(outside.join("keep"))?, b"outside");
    assert!(saved.is_dir());
    Ok(())
}

#[test]
fn an_existing_plaintext_backup_is_never_replaced() -> Result<()> {
    let (_temp, env, source, _, _) = fixture()?;
    let backup = backup_path(source.root())?;
    std::fs::create_dir(&backup)?;
    std::fs::write(backup.join("keep"), b"existing user data")?;
    let before = snapshot::scan(source.root())?.digest;
    assert!(verify(&env).is_err());
    assert_eq!(snapshot::scan(source.root())?.digest, before);
    assert_eq!(std::fs::read(backup.join("keep"))?, b"existing user data");
    assert!(!stage_root(source.root())?.exists());
    Ok(())
}
