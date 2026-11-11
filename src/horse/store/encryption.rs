//! Encrypted record framing and format detection for the horse store.

use super::Store;
use anyhow::{bail, Context as _, Result};
use std::path::Path;

const FORMAT_FILE: &str = "encrypted-format";
const FORMAT: &[u8] = b"paddock horse encrypted store 1\n";
const SHARED: &str = "shared";

pub(super) fn is_encrypted(root: &Path) -> Result<bool> {
    match std::fs::read(root.join(FORMAT_FILE)) {
        Ok(bytes) if bytes == FORMAT => Ok(true),
        Ok(_) => bail!("horse store encryption format marker is damaged; store was not changed"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// A missing marker must not make an encrypted object look like a legacy
/// plaintext store to the unkeyed API.
pub(super) fn contains_encrypted_records(root: &Path) -> Result<bool> {
    use std::io::Read as _;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if entry.file_name() != "keys" && contains_encrypted_records(&entry.path())? {
                return Ok(true);
            }
        } else if ty.is_file() {
            let mut prefix = Vec::new();
            std::fs::File::open(entry.path())?
                .take(128)
                .read_to_end(&mut prefix)?;
            let n = prefix.len();
            if super::index::has_magic(&prefix) {
                return Ok(true);
            }
            if n > 1 {
                let owner_len = usize::from(prefix[0]);
                if owner_len > 0
                    && owner_len <= 64
                    && n > 1 + owner_len
                    && super::super::crypto::has_magic(&prefix[1 + owner_len..n])
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Refuse to reinterpret an existing store as empty if its format marker is
/// missing. A plaintext store must be migrated explicitly, never overwritten.
pub(super) fn require_empty(root: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some("keys" | "key-check" | "lock" | FORMAT_FILE)
        ) {
            continue;
        }
        if entry.file_type()?.is_dir() && std::fs::read_dir(entry.path())?.next().is_none() {
            continue;
        }
        bail!("horse store has content without a complete encryption setup; run paddock horse migrate --verify or restore its keys; store was not changed");
    }
    Ok(())
}

pub(super) fn initialize(store: &Store) -> Result<()> {
    let keys = store
        .keyring
        .as_ref()
        .context("horse keyring is unavailable")?;
    if is_encrypted(&store.root)? {
        keys.horse_key(SHARED)?;
        return Ok(());
    }
    require_empty(&store.root)?;
    keys.ensure_horse_key(SHARED)?;
    super::super::keys::write_new_synced(&store.root.join(FORMAT_FILE), FORMAT, true)?;
    Ok(())
}

fn context(kind: &str, identity: &str, owner: &str) -> Vec<u8> {
    format!("paddock horse {kind}\0{identity}\0{owner}").into_bytes()
}

pub(super) fn seal(store: &Store, kind: &str, identity: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let owner = store.owner.as_deref().unwrap_or(SHARED);
    let key = store
        .keyring
        .as_ref()
        .context("horse keyring is unavailable")?
        .horse_key(owner)?;
    let ciphertext = super::super::crypto::seal(&key, &context(kind, identity, owner), bytes)?;
    let mut result = Vec::with_capacity(1 + owner.len() + ciphertext.len());
    result.push(owner.len() as u8);
    result.extend_from_slice(owner.as_bytes());
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub(super) fn open(store: &Store, kind: &str, identity: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let Some(&len) = bytes.first() else {
        bail!("invalid encrypted horse record")
    };
    let len = usize::from(len);
    if len == 0 || len > 64 || bytes.len() <= len {
        bail!("invalid encrypted horse record owner");
    }
    let owner =
        std::str::from_utf8(&bytes[1..=len]).context("invalid encrypted horse record owner")?;
    let key = store
        .keyring
        .as_ref()
        .context("horse keyring is unavailable")?
        .horse_key(owner)?;
    super::super::crypto::open(&key, &context(kind, identity, owner), &bytes[len + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::HostEnv;
    use crate::horse::Binding;

    #[test]
    fn host_store_encrypts_objects_and_mutable_session_content() -> Result<()> {
        fn scan(path: &Path, canary: &[u8]) -> Result<()> {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    scan(&entry.path(), canary)?;
                } else {
                    let bytes = std::fs::read(entry.path())?;
                    assert!(!bytes.windows(canary.len()).any(|w| w == canary));
                }
            }
            Ok(())
        }
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
        let scoped = store.for_horse(&horse.id)?;
        let canary = b"session-content-canary-783462";
        let hash = scoped.put_blob("application/test", canary)?;
        assert_eq!(scoped.get(&hash)?.raw, canary);
        let object_bytes = std::fs::read(store.object_path(&hash))?;
        assert!(object_bytes
            .windows(horse.id.len())
            .any(|bytes| bytes == horse.id.as_bytes()));
        let session = store.new_session(
            &horse.id,
            Binding {
                harness: "test".into(),
                native_id: None,
                version: "1".into(),
                container: "test".into(),
            },
        )?;
        let entry = store.queue_append(&session.id, "user", std::str::from_utf8(canary)?)?;
        assert_eq!(store.inbox(&session.id)?[0].text.as_bytes(), canary);
        store.record_delivery(&entry, None, "message-1")?;
        assert_eq!(
            store.deliveries(&session.id)?[0].entry.text.as_bytes(),
            canary
        );
        let approval = serde_json::json!({"command": std::str::from_utf8(canary)?});
        crate::horse::live::codex::append_ledger(&store, &session.id, &approval)?;
        assert_eq!(
            crate::horse::live::codex::ledger(&store, &session.id)?,
            vec![approval]
        );
        scan(store.root(), canary)?;
        assert_eq!(Store::open_host(&env)?.get(&hash)?.raw, canary);
        assert!(Store::open(store.root()).is_err());
        Ok(())
    }

    #[test]
    fn missing_format_marker_refuses_to_reinterpret_existing_content() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = Store::open_host(&env)?;
        let hash = store.put_blob("test", b"keep me")?;
        let object = store.object_path(&hash);
        let before = std::fs::read(&object)?;
        std::fs::remove_file(store.root().join(FORMAT_FILE))?;
        assert!(Store::open_host(&env).is_err());
        assert_eq!(std::fs::read(&object)?, before);
        assert!(!store.root().join(FORMAT_FILE).exists());
        std::fs::remove_file(store.root().join("key-check"))?;
        assert!(Store::open(store.root()).is_err());
        assert_eq!(std::fs::read(object)?, before);
        Ok(())
    }

    #[test]
    fn missing_key_check_never_initializes_over_an_encrypted_store() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = Store::open_host(&env)?;
        let hash = store.put_blob("test", b"preserve")?;
        let object = store.object_path(&hash);
        let before = std::fs::read(&object)?;
        let master = env.paddock_state().join("keyring/horse/master");
        let original_key = std::fs::read(&master)?;
        std::fs::remove_file(store.root().join("key-check"))?;
        assert!(Store::open_host(&env).is_err());
        assert!(!store.root().join("key-check").exists());
        assert_eq!(std::fs::read(&master)?, original_key);
        assert_eq!(std::fs::read(object)?, before);
        Ok(())
    }

    #[test]
    fn clear_object_metadata_is_authenticated() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = Store::open_host(&env)?;
        let hash = store.put_blob("test", b"secret")?;
        let path = store.object_path(&hash);
        let mut bytes = std::fs::read(&path)?;
        let at = bytes
            .windows(4)
            .position(|window| window == b"blob")
            .context("missing clear kind")?;
        bytes[at] = b'x';
        std::fs::write(path, bytes)?;
        assert!(store.get(&hash).is_err());
        assert!(store.indexed_chain(Some(&hash), u64::MAX, &mut 0).is_err());
        Ok(())
    }

    #[test]
    fn local_registry_artifacts_are_encrypted_and_round_trip() -> Result<()> {
        use crate::horse::registry::{self, Manifest, Transport};
        let temp = tempfile::tempdir()?;
        let env = HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = Store::open_host(&env)?;
        let mut dna = crate::horse::dna::default_for_run();
        dna.traits.insert(
            "test".into(),
            crate::horse::dna::Trait::new(
                "registry-canary-2842",
                crate::horse::dna::Provenance::Default,
            ),
        );
        let bytes = dna.encode().into_bytes();
        let manifest = Manifest {
            name: "test/item".into(),
            version: "1.0.0".into(),
            hash: registry::artifact_hash(&bytes),
            parents: vec![],
            created_ms: 1,
            author: None,
            signature: None,
        };
        let indexes = registry::configured_for_store(&env.paddock_state(), &store)?;
        let index = &indexes[0];
        index.put(&manifest, &bytes)?;
        assert_eq!(index.object(&manifest.hash)?.unwrap(), bytes);
        assert_eq!(index.manifest("test", "item", "1.0.0")?.unwrap(), manifest);
        let disk = std::fs::read(index.root.join("objects").join(manifest.hash.as_str()))?;
        assert!(!disk
            .windows(b"registry-canary-2842".len())
            .any(|w| w == b"registry-canary-2842"));
        Ok(())
    }
}
