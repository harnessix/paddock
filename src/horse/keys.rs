//! Authenticated wrapping of horse data keys.
//!
//! A wrapped key is bound to its horse ID. The master key and unwrapped
//! data keys must never be written to the horse store or exposed to a run.

use anyhow::{bail, Context as _, Result};
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8] = b"paddock-horse-key\x01";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// Secret key material. Deliberately does not implement `Debug` or `Display`.
pub struct Key([u8; KEY_LEN]);

impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Key {
    pub fn generate() -> Result<Self> {
        Ok(Self(super::random_bytes()?))
    }

    /// Load exactly one key from an explicit backup or keyring entry.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; KEY_LEN] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("horse key must be exactly {KEY_LEN} bytes"))?;
        Ok(Self(bytes))
    }

    /// Expose the key only to the caller responsible for saving a backup.
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Wrap a data key with a fresh nonce and bind it to one horse.
    pub fn wrap(&self, horse: &str, data: &Key) -> Result<Vec<u8>> {
        validate_id(horse)?;
        let key = cipher(&self.0)?;
        let nonce = super::random_bytes::<NONCE_LEN>()?;
        let mut body = Zeroizing::new(data.0.to_vec());
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(horse)),
            &mut *body,
        )
        .map_err(|_| anyhow::anyhow!("cannot wrap horse data key"))?;
        let mut result = Vec::with_capacity(MAGIC.len() + NONCE_LEN + body.len());
        result.extend_from_slice(MAGIC);
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&body);
        Ok(result)
    }

    /// Refuse damaged data, a different horse ID, or a wrong master key.
    pub fn unwrap(&self, horse: &str, wrapped: &[u8]) -> Result<Key> {
        validate_id(horse)?;
        if wrapped.len() != MAGIC.len() + NONCE_LEN + KEY_LEN + TAG_LEN
            || !wrapped.starts_with(MAGIC)
        {
            bail!("invalid wrapped horse key; store was not changed");
        }
        let nonce: [u8; NONCE_LEN] = wrapped[MAGIC.len()..MAGIC.len() + NONCE_LEN]
            .try_into()
            .expect("checked wrapped key length");
        let mut body = Zeroizing::new(wrapped[MAGIC.len() + NONCE_LEN..].to_vec());
        let plaintext = cipher(&self.0)?
            .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad(horse)), &mut body[..])
            .map_err(|_| anyhow::anyhow!("horse key cannot open the store: wrong key or damaged data; store was not changed"))?;
        Key::from_bytes(plaintext)
    }
}

fn cipher(key: &[u8; KEY_LEN]) -> Result<LessSafeKey> {
    let key = UnboundKey::new(&aead::CHACHA20_POLY1305, key)
        .map_err(|_| anyhow::anyhow!("cannot initialize horse key cipher"))?;
    Ok(LessSafeKey::new(key))
}

fn validate_id(horse: &str) -> Result<()> {
    if horse.is_empty()
        || horse.len() > 64
        || !horse
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("invalid horse id for key wrapping");
    }
    Ok(())
}

fn aad(horse: &str) -> Vec<u8> {
    let mut aad = MAGIC.to_vec();
    aad.extend_from_slice(horse.as_bytes());
    aad
}

/// Keyring and store paths are supplied separately: the master key must not
/// live under the horse store, which may be copied or mounted independently.
#[derive(Clone, Debug)]
pub struct Keyring {
    master: PathBuf,
    store: PathBuf,
}

impl Keyring {
    pub fn new(master: PathBuf, store: PathBuf) -> Self {
        Self { master, store }
    }

    pub(crate) fn for_standalone(store: &Path) -> Result<Self> {
        let name = store
            .file_name()
            .context("horse store needs a named root")?;
        let mut key_dir = std::ffi::OsString::from(".");
        key_dir.push(name);
        key_dir.push(".paddock-keyring");
        let parent = store.parent().context("horse store needs a parent")?;
        Ok(Self::new(
            parent.join(key_dir).join("master"),
            store.to_path_buf(),
        ))
    }

    /// Serialize initialization before inspecting a partially created store.
    pub(crate) fn initialization_lock(&self) -> Result<File> {
        use std::os::fd::AsRawFd as _;
        let dir = self
            .master
            .parent()
            .context("horse keyring needs a parent")?;
        crate::privfs::dir(dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("initialize.lock"))?;
        // SAFETY: flock operates on this live file descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(file)
    }

    fn check_path(&self) -> PathBuf {
        self.store.join("key-check")
    }

    pub fn keys_ready(&self) -> Result<bool> {
        let marker = std::fs::read(self.check_path())?;
        let state = self.read_master()?.unwrap("store", &marker)?;
        match state.as_bytes() {
            bytes if *bytes == [0; KEY_LEN] => Ok(false),
            bytes if *bytes == [1; KEY_LEN] => Ok(true),
            _ => bail!("horse key provisioning marker is damaged; store was not changed"),
        }
    }

    pub fn mark_keys_ready(&self) -> Result<()> {
        let master = self.checked_master()?;
        let ready = Key::from_bytes(&[1; KEY_LEN])?;
        let wrapped = master.wrap("store", &ready)?;
        crate::privfs::write_atomic(&self.check_path(), &wrapped)?;
        super::store::sync_dir(&self.store)?;
        Ok(())
    }

    pub fn for_host(env: &crate::env::HostEnv) -> Self {
        Self::for_store(env, super::root(env))
    }

    pub(crate) fn for_store(env: &crate::env::HostEnv, store: PathBuf) -> Self {
        Self::new(
            // The storage cleaner treats direct files in `keyring` as
            // disposable sandbox-home passwords. Keep this user's asset key
            // in its own directory, outside that inventory.
            env.paddock_state().join("keyring/horse/master"),
            store,
        )
    }

    pub fn has_check(&self) -> Result<bool> {
        match std::fs::symlink_metadata(self.check_path()) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn read_master(&self) -> Result<Key> {
        let bytes = Zeroizing::new(std::fs::read(&self.master).map_err(|e| {
            anyhow::anyhow!(
                "cannot read horse master key at {}: {e}; store was not changed",
                self.master.display()
            )
        })?);
        Key::from_bytes(&bytes).context("invalid horse master key; store was not changed")
    }

    /// Check an existing store before making any change to it.
    pub fn check(&self) -> Result<()> {
        self.checked_master().map(|_| ())
    }

    /// Refuse a missing key check when wrapped horse keys are present.
    pub fn preflight(&self) -> Result<()> {
        if self.has_check()? {
            return self.check();
        }
        match std::fs::read_dir(self.store.join("keys")) {
            Ok(mut entries) => {
                if entries.next().transpose()?.is_some() {
                    bail!("horse key check is missing but data keys exist; store was not changed")
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    fn checked_master(&self) -> Result<Key> {
        let key = self.read_master()?;
        let check = std::fs::read(self.check_path()).map_err(|e| {
            anyhow::anyhow!("cannot read horse key check: {e}; store was not changed")
        })?;
        key.unwrap("store", &check)?;
        Ok(key)
    }

    /// Establish a key check for a new keyring. The caller must serialize
    /// initialization and must not call this for an encrypted store whose
    /// key check is missing.
    pub fn initialize(&self) -> Result<()> {
        self.preflight()?;
        if self.has_check()? {
            return Ok(());
        }
        let key = match self.read_master() {
            Ok(key) => key,
            Err(_) if !self.master.exists() => {
                let key = Key::generate()?;
                match write_new_synced(&self.master, key.as_bytes(), true) {
                    Ok(()) => key,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        self.read_master()?
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Err(e) => return Err(e),
        };
        let check = key.wrap("store", &Key::from_bytes(&[0; KEY_LEN])?)?;
        match write_new_synced(&self.check_path(), &check, true) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.check(),
            Err(e) => Err(e.into()),
        }
    }

    /// Export without replacing an existing file.
    pub fn export(&self, destination: &Path) -> Result<()> {
        let key = self.checked_master()?;
        write_new_synced(destination, key.as_bytes(), false)?;
        Ok(())
    }

    /// Restore only into an empty keyring, after authenticating against the
    /// existing store. A wrong key does not touch the store or the keyring.
    pub fn import(&self, source: &Path) -> Result<()> {
        let bytes = Zeroizing::new(std::fs::read(source)?);
        let key = Key::from_bytes(&bytes)?;
        let check = std::fs::read(self.check_path())?;
        key.unwrap("store", &check)?;
        write_new_synced(&self.master, key.as_bytes(), true)?;
        Ok(())
    }

    /// Create a horse's data key without ever replacing an existing key.
    pub fn create_horse_key(&self, horse: &str) -> Result<()> {
        validate_id(horse)?;
        if horse == "store" {
            bail!("reserved horse id");
        }
        let key = self.checked_master()?;
        let wrapped = key.wrap(horse, &Key::generate()?)?;
        write_new_synced(&self.store.join("keys").join(horse), &wrapped, true)?;
        Ok(())
    }

    pub fn horse_key(&self, horse: &str) -> Result<Key> {
        validate_id(horse)?;
        if horse == "store" {
            bail!("reserved horse id");
        }
        let key = self.checked_master()?;
        let wrapped = std::fs::read(self.store.join("keys").join(horse)).with_context(|| {
            format!("cannot read data key for horse {horse}; store was not changed")
        })?;
        key.unwrap(horse, &wrapped)
    }

    /// Provision a legacy horse once, or verify its existing wrapped key.
    pub fn ensure_horse_key(&self, horse: &str) -> Result<()> {
        validate_id(horse)?;
        let path = self.store.join("keys").join(horse);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => self.horse_key(horse).map(|_| ()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.create_horse_key(horse),
            Err(e) => Err(e.into()),
        }
    }
}

pub(super) fn write_new_synced(
    path: &Path,
    bytes: &[u8],
    private_parent: bool,
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if private_parent {
        crate::privfs::dir(parent)?;
    }
    let temp = parent.join(format!(
        ".horse-key-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(crate::privfs::FILE_MODE)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::hard_link(&temp, path)?;
        File::open(parent)?.sync_all()
    })();
    let _ = std::fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_keys_are_random_and_bound_to_the_master_and_horse() -> Result<()> {
        let master = Key::generate()?;
        let data = Key::generate()?;
        let first = master.wrap("horse-1", &data)?;
        let second = master.wrap("horse-1", &data)?;
        assert_ne!(first, second);
        assert_eq!(
            master.unwrap("horse-1", &first)?.as_bytes(),
            data.as_bytes()
        );
        assert!(master.unwrap("horse-2", &first).is_err());
        assert!(Key::generate()?.unwrap("horse-1", &first).is_err());
        let mut damaged = first.clone();
        *damaged.last_mut().context("missing tag")? ^= 1;
        assert!(master.unwrap("horse-1", &damaged).is_err());
        assert!(master.unwrap("horse-1", &first[..first.len() - 1]).is_err());
        Ok(())
    }

    #[test]
    fn exported_key_bytes_restore_the_same_key() -> Result<()> {
        let master = Key::generate()?;
        let data = Key::generate()?;
        let wrapped = master.wrap("horse-1", &data)?;
        let restored = Key::from_bytes(master.as_bytes())?;
        assert_eq!(
            restored.unwrap("horse-1", &wrapped)?.as_bytes(),
            data.as_bytes()
        );
        assert!(Key::from_bytes(&[0; 31]).is_err());
        Ok(())
    }

    #[test]
    fn recovery_checks_before_writing_and_never_replaces_keys() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir()?;
        let store = temp.path().join("store");
        let master = temp.path().join("state/keyring/master");
        let keys = Keyring::new(master.clone(), store.clone());
        keys.initialize()?;
        keys.create_horse_key("h-1")?;
        let expected = keys.horse_key("h-1")?;
        let backup = temp.path().join("backup");
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755))?;
        keys.export(&backup)?;
        assert_eq!(
            std::fs::metadata(temp.path())?.permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&backup)?.permissions().mode() & 0o777,
            0o600
        );
        assert!(keys.export(&backup).is_err());
        assert!(keys.create_horse_key("h-1").is_err());
        assert_eq!(expected.as_bytes(), keys.horse_key("h-1")?.as_bytes());

        let fresh = Keyring::new(temp.path().join("fresh/keyring/master"), store.clone());
        let before_check = std::fs::read(store.join("key-check"))?;
        let before_horse = std::fs::read(store.join("keys/h-1"))?;
        let bad = temp.path().join("bad");
        std::fs::write(&bad, Key::generate()?.as_bytes())?;
        assert!(fresh.import(&bad).is_err());
        assert!(!fresh.master.exists());
        assert_eq!(std::fs::read(store.join("key-check"))?, before_check);
        assert_eq!(std::fs::read(store.join("keys/h-1"))?, before_horse);
        assert!(fresh.check().is_err());
        fresh.import(&backup)?;
        fresh.check()?;
        assert_eq!(expected.as_bytes(), fresh.horse_key("h-1")?.as_bytes());
        assert!(fresh.import(&backup).is_err());
        Ok(())
    }

    #[test]
    fn wrong_or_missing_master_does_not_change_store() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = temp.path().join("store");
        let master = temp.path().join("keyring/master");
        let keys = Keyring::new(master.clone(), store.clone());
        keys.initialize()?;
        let before = std::fs::read(store.join("key-check"))?;
        std::fs::write(&master, Key::generate()?.as_bytes())?;
        assert!(keys.initialize().is_err());
        assert_eq!(std::fs::read(store.join("key-check"))?, before);
        std::fs::remove_file(&master)?;
        assert!(keys.initialize().is_err());
        assert!(!master.exists());
        assert_eq!(std::fs::read(store.join("key-check"))?, before);
        Ok(())
    }

    #[test]
    fn host_open_checks_key_before_touching_store_and_keys_new_horses() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::env::HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = crate::horse::Store::open_host(&env)?;
        let horse = store.new_default_horse()?;
        let keys = Keyring::for_host(&env);
        keys.horse_key(&horse.id)?;
        let marker = std::fs::read(keys.check_path())?;
        let horse_ref = std::fs::read(
            store
                .root()
                .join("horses")
                .join(format!("{}.json", horse.id)),
        )?;
        std::fs::write(&keys.master, Key::generate()?.as_bytes())?;
        assert!(crate::horse::Store::open_host(&env).is_err());
        assert_eq!(std::fs::read(keys.check_path())?, marker);
        assert_eq!(
            std::fs::read(
                store
                    .root()
                    .join("horses")
                    .join(format!("{}.json", horse.id))
            )?,
            horse_ref
        );
        Ok(())
    }

    #[test]
    fn backup_opens_copied_store_in_a_fresh_home() -> Result<()> {
        fn copy_tree(from: &Path, to: &Path) -> Result<()> {
            std::fs::create_dir_all(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                let target = to.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    copy_tree(&entry.path(), &target)?;
                } else {
                    std::fs::copy(entry.path(), target)?;
                }
            }
            Ok(())
        }
        let temp = tempfile::tempdir()?;
        let env = |name: &str| {
            crate::env::HostEnv::from_parts(
                temp.path().join(name),
                "test".into(),
                0,
                0,
                Default::default(),
            )
        };
        let original = env("original");
        let fresh = env("fresh");
        let store = crate::horse::Store::open_host(&original)?;
        let horse = store.new_default_horse()?;
        let backup = temp.path().join("key.backup");
        Keyring::for_host(&original).export(&backup)?;
        copy_tree(store.root(), &crate::horse::root(&fresh))?;
        assert!(crate::horse::Store::open_host(&fresh).is_err());
        Keyring::for_host(&fresh).import(&backup)?;
        let restored = crate::horse::Store::open_host(&fresh)?;
        assert_eq!(restored.horse(&horse.id)?, horse);
        assert_eq!(
            Keyring::for_host(&fresh).horse_key(&horse.id)?.as_bytes(),
            Keyring::for_host(&original)
                .horse_key(&horse.id)?
                .as_bytes()
        );
        Ok(())
    }

    #[test]
    fn concurrent_host_openers_share_the_same_key() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::env::HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let env = env.clone();
                std::thread::spawn(move || {
                    crate::horse::Store::open_host(&env)
                        .map(|_| ())
                        .map_err(|e| format!("{e:#}"))
                })
            })
            .collect();
        for thread in threads {
            thread
                .join()
                .expect("host opener panicked")
                .map_err(anyhow::Error::msg)?;
        }
        Keyring::for_host(&env).check()?;
        Ok(())
    }

    #[test]
    fn missing_horse_key_is_not_replaced_after_provisioning() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::env::HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = crate::horse::Store::open_host(&env)?;
        let horse = store.new_default_horse()?;
        let key_path = store.root().join("keys").join(&horse.id);
        let marker = std::fs::read(store.root().join("key-check"))?;
        std::fs::remove_file(&key_path)?;
        assert!(crate::horse::Store::open_host(&env).is_err());
        assert!(!key_path.exists());
        assert_eq!(std::fs::read(store.root().join("key-check"))?, marker);
        Ok(())
    }
}
