use super::*;
use crate::env::HostEnv;
use crate::horse::{dsh, live::fake};
use serde_json::json;

fn request(command: &str, args: &[&str], grant_id: &str) -> Request {
    Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: [
            ("op".into(), json!("peek")),
            ("command".into(), json!(command)),
            ("args".into(), json!(args)),
            ("grant".into(), json!(grant_id)),
        ]
        .into(),
        reason: String::new(),
    }
}

#[test]
fn own_reads_are_scoped_counted_and_revocable() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let env = HostEnv::from_parts(
        temp.path().to_path_buf(),
        "test".into(),
        0,
        0,
        Default::default(),
    );
    let store = crate::horse::Store::open_host(&env)?;
    let horse = store.new_default_horse()?;
    let canary = "peek-canary-895614";
    let root = temp.path().join("native");
    fake::write_file(&root, "session-a", &fake::turn_rows(0, 1, canary, "m1"));
    let native = dsh::session_dir(&root, Some(fake::CWD), "session-a");
    let session = dsh::ingest(&store, &native, dsh::VERSION, Some(&horse.id))?.session;
    let access = Access {
        store: store.clone(),
        horse: horse.id,
        session: session.clone(),
    };
    issue_own(&access, "sb-1")?;
    let response = handle(Some(&access), "sb-1", &request("grep", &[canary], "own"));
    assert_eq!(response.status, Status::Done);
    let data: Vec<u8> = serde_json::from_value(response.result["bytes"].clone())?;
    assert!(String::from_utf8_lossy(&data).contains(canary));
    assert!(handle(Some(&access), "sb-1", &request("ls", &[], "own")).status == Status::Done);
    assert!(handle(Some(&access), "sb-1", &request("cat", &["3"], "own")).status == Status::Done);
    assert!(
        handle(Some(&access), "sb-1", &request("range", &["0", "2"], "own")).status == Status::Done
    );
    assert!(handle(Some(&access), "sb-1", &request("dna", &[], "own")).status == Status::Done);
    assert!(handle(Some(&access), "sb-1", &request("binding", &[], "own")).status == Status::Done);
    store.queue_append(&session, "user", "queued-private-canary")?;
    let inbox = handle(Some(&access), "sb-1", &request("inbox", &[], "own"));
    assert_eq!(inbox.status, Status::Done);
    let bytes: Vec<u8> = serde_json::from_value(inbox.result["bytes"].clone())?;
    assert!(String::from_utf8_lossy(&bytes).contains("queued-private-canary"));
    assert_ne!(
        handle(Some(&access), "other", &request("ls", &[], "own")).status,
        Status::Done
    );
    let grant = grant::get(&store, "sb-1", "own")?.unwrap();
    assert_eq!(grant.reads.len(), 7);
    assert!(grant.used_bytes > 0);
    let mut oversized = request("cat", &[], "own");
    oversized.args.insert("args".into(), json!(vec!["0"; 257]));
    assert_ne!(
        handle(Some(&access), "sb-1", &oversized).status,
        Status::Done
    );
    assert_eq!(grant::get(&store, "sb-1", "own")?.unwrap().reads.len(), 7);
    let mut stale = request("cat", &["3"], "own");
    stale.args.insert("fingerprint".into(), json!("different"));
    assert_ne!(handle(Some(&access), "sb-1", &stale).status, Status::Done);
    assert_eq!(
        grant::get(&store, "sb-1", "own")?
            .unwrap()
            .reads
            .last()
            .unwrap()
            .status,
        "failed"
    );
    assert!(revoke(&store, "sb-1")?);
    issue_own(&access, "sb-1")?;
    assert_ne!(
        handle(Some(&access), "sb-1", &request("grep", &[canary], "own")).status,
        Status::Done
    );
    assert!(grant::get(&store, "sb-1", "own")?.unwrap().revoked);
    Ok(())
}

#[test]
fn trainer_grant_mount_is_ciphertext_and_quota_is_enforced() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let env = HostEnv::from_parts(
        temp.path().to_path_buf(),
        "test".into(),
        0,
        0,
        Default::default(),
    );
    let store = crate::horse::Store::open_host(&env)?;
    let target = store.new_default_horse()?;
    let trainer = store.new_default_horse()?;
    let canary = "trainer-canary-140789";
    let root = temp.path().join("native");
    fake::write_file(&root, "target", &fake::turn_rows(0, 1, canary, "m1"));
    let native = dsh::session_dir(&root, Some(fake::CWD), "target");
    let target_session = dsh::ingest(&store, &native, dsh::VERSION, Some(&target.id))?.session;
    let binding = crate::horse::Binding {
        harness: "dsh".into(),
        native_id: None,
        version: "1".into(),
        container: "test".into(),
    };
    let own = store.new_session(&trainer.id, binding)?;
    let access = Access {
        store: store.clone(),
        horse: trainer.id,
        session: own.id,
    };
    issue_own(&access, "trainer-sb")?;
    grant::issue_trainer(
        &store,
        "trainer-sb",
        "g-1",
        &target.id,
        &target_session,
        crate::horse::now_ms() + 60_000,
        1024 * 1024,
    )?;
    assert_ne!(
        handle(Some(&access), "trainer-sb", &request("inbox", &[], "g-1")).status,
        Status::Done
    );
    let grant = grant::get(&store, "trainer-sb", "g-1")?.unwrap();
    let mut tree = access.tree()?;
    add_mount(&access, "trainer-sb", &mut tree)?;
    let files: Vec<_> = tree
        .paths
        .iter()
        .filter(|(name, _)| name.starts_with("peek/g-1/"))
        .collect();
    assert!(!files.is_empty());
    for (_, entry) in files {
        if let super::super::Entry::File(bytes) = entry {
            assert!(!bytes.windows(canary.len()).any(|w| w == canary.as_bytes()));
            assert!(!bytes
                .windows(grant.mount_key.len())
                .any(|w| w == grant.mount_key));
        }
    }
    let response = handle(
        Some(&access),
        "trainer-sb",
        &request("grep", &[canary], "g-1"),
    );
    assert_eq!(response.status, Status::Done);
    let bytes: Vec<u8> = serde_json::from_value(response.result["bytes"].clone())?;
    assert!(String::from_utf8_lossy(&bytes).contains(canary));
    grant::issue_trainer(
        &store,
        "trainer-sb",
        "tiny",
        &target.id,
        &target_session,
        crate::horse::now_ms() + 60_000,
        1,
    )?;
    assert_ne!(
        handle(
            Some(&access),
            "trainer-sb",
            &request("grep", &[canary], "tiny")
        )
        .status,
        Status::Done
    );
    assert_eq!(
        grant::get(&store, "trainer-sb", "tiny")?
            .unwrap()
            .reads
            .len(),
        1
    );
    revoke(&store, "trainer-sb")?;
    assert_eq!(
        grant::get(&store, "trainer-sb", "g-1")?.unwrap().mount_key,
        [0; 32]
    );
    assert_ne!(
        handle(
            Some(&access),
            "trainer-sb",
            &request("grep", &[canary], "g-1")
        )
        .status,
        Status::Done
    );
    let mut after = access.tree()?;
    add_mount(&access, "trainer-sb", &mut after)?;
    assert!(!after.paths.keys().any(|name| name.starts_with("peek/g-1/")));
    Ok(())
}

#[test]
fn selecting_a_small_node_does_not_decrypt_its_large_ancestors() -> Result<()> {
    use crate::horse::store::Origin;
    let temp = tempfile::tempdir()?;
    let env = HostEnv::from_parts(
        temp.path().to_path_buf(),
        "test".into(),
        0,
        0,
        Default::default(),
    );
    let store = crate::horse::Store::open_host(&env)?;
    let horse = store.new_default_horse()?;
    let binding = crate::horse::Binding {
        harness: "test".into(),
        native_id: None,
        version: "1".into(),
        container: "test".into(),
    };
    let session = store.new_session(&horse.id, binding)?;
    let scoped = store.for_horse(&horse.id)?;
    let origin = Origin {
        harness: "test",
        container: "test",
        version: "1",
    };
    let large = scoped.put_native(&origin, None, "0", &vec![b'x'; 64 * 1024])?;
    let small = scoped.put_native(&origin, Some(&large), "1", b"small-canary")?;
    store.update_session(&session.id, |s| s.head = Some(small))?;
    let access = Access {
        store: store.clone(),
        horse: horse.id.clone(),
        session: session.id.clone(),
    };
    grant::issue_trainer(
        &store,
        "select-sb",
        "g",
        &horse.id,
        &session.id,
        crate::horse::now_ms() + 60_000,
        4096,
    )?;
    let response = handle(Some(&access), "select-sb", &request("cat", &["1"], "g"));
    assert_eq!(response.status, Status::Done, "{}", response.message);
    let bytes: Vec<u8> = serde_json::from_value(response.result["bytes"].clone())?;
    assert_eq!(bytes, b"small-canary");
    assert_ne!(
        handle(Some(&access), "select-sb", &request("cat", &["0"], "g")).status,
        Status::Done
    );
    let grant = grant::get(&store, "select-sb", "g")?.unwrap();
    assert_eq!(grant.reads.len(), 2);
    assert_eq!(grant.reads[1].status, "failed");
    assert!(grant.used_bytes < 4096);
    Ok(())
}

#[test]
fn trainer_key_is_absent_from_sandbox_environment_files_and_process_memory() -> Result<()> {
    if !crate::testutil::bwrap_works() {
        eprintln!("skipped: key isolation needs bwrap");
        return Ok(());
    }
    use crate::backend::Backend as _;
    use crate::spec::{Mount, Network, SandboxSpec};
    let temp = tempfile::tempdir()?;
    let mut env = crate::privfs::tests::env_in(temp.path());
    env.uid = unsafe { libc::getuid() };
    env.gid = unsafe { libc::getgid() };
    env.path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let store = crate::horse::Store::open_host(&env)?;
    let target = store.new_default_horse()?;
    let session = store.new_session(
        &target.id,
        crate::horse::Binding {
            harness: "test".into(),
            native_id: None,
            version: "1".into(),
            container: "test".into(),
        },
    )?;
    let origin = crate::horse::store::Origin {
        harness: "test",
        container: "test",
        version: "1",
    };
    let node = store.for_horse(&target.id)?.put_native(
        &origin,
        None,
        "seq:0",
        b"key-isolation-plaintext-canary",
    )?;
    store.update_session(&session.id, |session| session.head = Some(node))?;
    let owned = store.new_default_horse()?;
    let own = store.new_session(&owned.id, session.binding.clone())?;
    let access = Access {
        store: store.clone(),
        horse: owned.id,
        session: own.id,
    };
    grant::issue_trainer(
        &store,
        "key-canary",
        "trainer",
        &target.id,
        &session.id,
        crate::horse::now_ms() + 60_000,
        1024,
    )?;
    let grant = grant::get(&store, "key-canary", "trainer")?.unwrap();
    let key = grant.mount_key;
    let key_hex = crate::manifest::hex(&key);
    let mut tree = access.tree()?;
    add_mount(&access, "key-canary", &mut tree)?;
    for entry in tree.paths.values() {
        if let super::super::Entry::File(bytes) = entry {
            assert!(!bytes.windows(key.len()).any(|window| window == key));
            assert!(!bytes
                .windows(key_hex.len())
                .any(|window| window == key_hex.as_bytes()));
        }
    }
    let target_dir = temp.path().join("target");
    let home = temp.path().join("private");
    std::fs::create_dir_all(&target_dir)?;
    std::fs::create_dir_all(&home)?;
    let mut spec = SandboxSpec::new(
        "key-canary".into(),
        "fake",
        &target_dir,
        home.clone(),
        &env.home,
        env.uid,
        env.gid,
        "trainer",
    );
    spec.network = Network::None;
    spec.add_mount(Mount::same(&target_dir, true, "target"));
    // Dump this fake process's readable private mappings to stdout.
    // The key itself is never passed to the sandbox or this program.
    let script = r#"import os,sys
mem = os.open('/proc/self/mem', os.O_RDONLY)
total = 0
for line in open('/proc/self/maps'):
    parts = line.split()
    if not parts[1].startswith('r') or not parts[1].endswith('p'):
        continue
    if parts[-1] in ('[vvar]', '[vvar_vclock]', '[vsyscall]'):
        continue
    start,end = [int(v,16) for v in parts[0].split('-')]
    while start < end:
        size = min(1048576, end-start)
        data = os.pread(mem, size, start)
        if len(data) != size:
            sys.exit(3)
        total += len(data)
        sys.stdout.buffer.write(data)
        start += size
if total == 0:
    sys.exit(2)
"#;
    spec.command = vec!["/usr/bin/python3".into(), "-c".into(), script.into()];
    crate::trainer::sandbox::restrict_mounts(&mut spec, &target_dir);
    let launch = crate::backend::bwrap::Bwrap.render(&env, &spec, false)?;
    crate::run::write_files(&launch)?;
    for (name, value) in &launch.env {
        assert!(!name
            .as_bytes()
            .windows(key.len())
            .any(|window| window == key));
        assert!(!value
            .as_bytes()
            .windows(key.len())
            .any(|window| window == key));
        assert!(!name.contains(&key_hex) && !value.contains(&key_hex));
    }
    let output = std::process::Command::new(&launch.program)
        .args(&launch.args)
        .env_clear()
        .envs(&launch.env)
        .output()?;
    assert!(
        output.status.success(),
        "fake trainer memory read failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stdout.windows(key.len()).any(|window| window == key),
        "grant key entered the fake process memory"
    );
    assert!(
        !output
            .stdout
            .windows(key_hex.len())
            .any(|window| window == key_hex.as_bytes()),
        "encoded grant key entered the fake process memory"
    );
    Ok(())
}
