use super::*;
use crate::env::HostEnv;
use crate::horse::{dsh, live::fake};

#[test]
fn mount_content_is_ciphertext_and_control_stays_writable() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let env = HostEnv::from_parts(
        temp.path().to_path_buf(),
        "test".into(),
        0,
        0,
        Default::default(),
    );
    let store = Store::open_host(&env)?;
    let horse = store.new_default_horse()?;
    let canary = "mount-private-canary-71492";
    let rows = fake::turn_rows(0, 1, canary, "msg-a");
    let native = temp.path().join("native");
    fake::write_file(&native, "session-a", &rows);
    let path = dsh::session_dir(&native, Some(fake::CWD), "session-a");
    let session = dsh::ingest(&store, &path, dsh::VERSION, Some(&horse.id))?.session;
    store.queue_append(&session, "user", canary)?;
    write_memory(&store, &horse.id, "note", canary.as_bytes())?;
    write_dna(
        &store,
        &horse.id,
        format!("[traits]\nnote = \"{canary}\"\n").as_bytes(),
    )?;
    let access = Access {
        store,
        horse: horse.id.clone(),
        session: session.clone(),
    };
    let master = std::fs::read(env.paddock_state().join("keyring/horse/master"))?;
    let data_key = crate::horse::keys::Keyring::for_host(&env).horse_key(&horse.id)?;
    let tree = access.tree()?;
    assert_eq!(
        tree.paths["self/id"],
        Entry::File(format!("{}\n", horse.id).into_bytes())
    );
    assert_eq!(tree.paths[ctl::PATH], Entry::File(Vec::new()));
    assert!(tree
        .paths
        .contains_key(&format!("self/sessions/{session}/log")));
    for (name, entry) in tree.paths {
        if let Entry::File(bytes) = entry {
            assert!(
                !bytes.windows(canary.len()).any(|w| w == canary.as_bytes()),
                "{name}"
            );
            assert!(!bytes.windows(master.len()).any(|w| w == master));
            assert!(!bytes
                .windows(data_key.as_bytes().len())
                .any(|w| w == data_key.as_bytes()));
        }
    }
    assert!(writable(ctl::PATH));
    assert!(!writable(DNA_PATH));
    assert!(!writable("self/memory/note"));
    Ok(())
}
