//! The horse store: a persistent, content-addressed DAG.
//!
//! Layout under the store root (all folders 0700, all files 0600):
//!
//! - `objects/ab/cdef...`: one object per file, named by the sha256 of its
//!   plaintext metadata and body. Authenticated index metadata stays clear;
//!   the full object is encrypted. Equal rows on equal parents deduplicate.
//! - `sessions/<id>.json`, `horses/<id>.json`: encrypted mutable objects.
//! - `tmp/`: files that are not complete yet.
//! - `lock`: the writer lock for the mutable objects.
//!
//! Every write goes to `tmp/` first and is synced. An object then gets a hard
//! link to its final name (an object that is there already is the same
//! object), and a mutable file gets a rename. A crash leaves at most a file
//! in `tmp/`, which the next open removes when it is old.

use crate::privfs;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

mod encryption;
mod index;

/// A temporary file older than this is left over from a crash.
const STALE_TMP: Duration = Duration::from_secs(3600);

/// The sha256 of an object, as 64 lower-case hex digits.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Hash(String);

impl Hash {
    pub fn of(bytes: &[u8]) -> Hash {
        Hash(super::hex(&Sha256::digest(bytes)))
    }

    pub fn parse(s: &str) -> Result<Hash> {
        if s.len() != 64
            || !s
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("not a node hash: {s}");
        }
        Ok(Hash(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 12 digits, for lists.
    pub fn short(&self) -> &str {
        &self.0[..12]
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `kind` of a node that paddock built from a live channel of a
/// harness, not read from a native file of it. Such a node is no native
/// original, so no codec projects a chain that holds one.
pub const LIVE: &str = "live";

/// The metadata line of an object. The field order is fixed, so the same
/// object always has the same bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// `native` for a row of a harness that paddock read from its native
    /// file, [`LIVE`] for a row that paddock built from a live channel,
    /// `blob` for other content.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// The container format of the row, for example `dsh-session-v4-jsonl`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The origin version supplied by the adapter. For Codex native rows
    /// added by the current codec, this is the ingest reader version and
    /// projected rows retain that context. Legacy nodes can lack reliable
    /// reader provenance; row writer versions are generally unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Hash>,
    /// The identity of the record in its native session, for example `seq:12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The type of a blob, for example `toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
}

/// One object of the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub hash: Hash,
    pub meta: Meta,
    /// The body: the raw native row, byte for byte, or the blob.
    pub raw: Vec<u8>,
}

/// Authenticated, non-content metadata used to navigate an encrypted DAG.
#[derive(Clone, Debug)]
pub(crate) struct IndexedNode {
    pub hash: Hash,
    pub kind: String,
    pub parent: Option<Hash>,
    pub size: u64,
    pub time: Option<u64>,
}

impl Node {
    /// paddock built the node from a live channel ([`LIVE`]).
    pub fn is_live(&self) -> bool {
        self.meta.kind == LIVE
    }
}

/// Where a native row comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin<'a> {
    pub harness: &'a str,
    pub container: &'a str,
    pub version: &'a str,
}

/// The store at one root folder.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    pub(crate) keyring: Option<super::keys::Keyring>,
    encrypted: bool,
    owner: Option<String>,
    host_env: Option<crate::env::HostEnv>,
}

/// The writer lock of the store. It is released on drop.
pub struct Lock<'a> {
    _file: File,
    pub(crate) store: &'a Store,
}

impl Store {
    /// Open an encrypted standalone store, with its keyring beside the root.
    /// Host callers should use `open_host` to use the user's paddock keyring.
    pub fn open(root: &Path) -> Result<Store> {
        let keys = super::keys::Keyring::for_standalone(root)?;
        Self::open_keyed(root, keys)
    }

    fn open_without_cleanup(root: &Path) -> Result<Store> {
        for d in ["", "objects", "sessions", "horses", "tmp"] {
            privfs::dir(&root.join(d))
                .with_context(|| format!("cannot create {}", root.join(d).display()))?;
        }
        let s = Store {
            root: root.to_path_buf(),
            keyring: None,
            encrypted: false,
            owner: None,
            host_env: None,
        };
        Ok(s)
    }

    /// Read an existing plaintext store during explicit migration. This does
    /// not create directories or remove temporary files.
    pub(crate) fn open_plain_for_migration(root: &Path) -> Result<Store> {
        if !std::fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_dir())
            || encryption::is_encrypted(root)?
            || encryption::contains_encrypted_records(root)?
        {
            bail!("the migration source is missing or contains encrypted records");
        }
        Ok(Store {
            root: root.to_path_buf(),
            keyring: None,
            encrypted: false,
            owner: None,
            host_env: None,
        })
    }

    pub(crate) fn is_encrypted_path(root: &Path) -> Result<bool> {
        encryption::is_encrypted(root)
    }

    /// Open a host store only after checking its master key. Initialization is
    /// serialized with the existing store writer lock.
    pub fn open_host(env: &crate::env::HostEnv) -> Result<Store> {
        let keys = super::keys::Keyring::for_host(env);
        let root = super::root(env);
        if !root.exists()
            && (super::migrate::backup_path(&root)?.exists()
                || super::migrate::stage_root(&root)?.exists())
        {
            bail!("horse migration is interrupted; run paddock horse migrate --verify to recover; no store was changed");
        }
        let mut store = Self::open_keyed(&root, keys)?;
        let mut safe_env = env.clone();
        safe_env.vars.clear();
        store.host_env = Some(safe_env);
        Ok(store)
    }

    pub(crate) fn host_env(&self) -> Option<&crate::env::HostEnv> {
        self.host_env.as_ref()
    }

    pub(crate) fn open_migration_stage(env: &crate::env::HostEnv, root: &Path) -> Result<Store> {
        let keys = super::keys::Keyring::for_store(env, root.to_path_buf());
        Self::open_keyed(root, keys)
    }

    fn open_keyed(root: &Path, keys: super::keys::Keyring) -> Result<Store> {
        let _initialization = keys.initialization_lock()?;
        keys.preflight()?;
        let encrypted = encryption::is_encrypted(root)?;
        if !encrypted {
            encryption::require_empty(root)?;
        } else if !keys.has_check()? {
            bail!("horse key check is missing; encrypted store was not changed");
        } else if !keys.keys_ready()? {
            encryption::require_empty(root)?;
        }
        if encrypted && keys.has_check()? && keys.keys_ready()? {
            keys.horse_key("shared")?;
            let existing = Store {
                root: root.to_path_buf(),
                keyring: Some(keys.clone()),
                encrypted,
                owner: None,
                host_env: None,
            };
            for horse in existing.horses()? {
                keys.horse_key(&horse.id)?;
            }
        }
        let mut store = Store::open_without_cleanup(root)?;
        store.keyring = Some(keys.clone());
        store.encrypted = encrypted;
        {
            let _lock = store.lock()?;
            keys.initialize()?;
            encryption::initialize(&store)?;
            let ready = keys.keys_ready()?;
            let encrypted_store = Store {
                encrypted: true,
                ..store.clone()
            };
            for horse in encrypted_store.horses()? {
                if ready {
                    keys.horse_key(&horse.id)?;
                } else {
                    keys.ensure_horse_key(&horse.id)?;
                }
            }
            if !ready {
                keys.mark_keys_ready()?;
            }
        }
        store.encrypted = true;
        store.clean_tmp();
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Select the horse data key for new content. Existing shared objects
    /// keep the key of their first writer so their plaintext hashes remain
    /// stable across forks and moves.
    pub fn for_horse(&self, horse: &str) -> Result<Store> {
        if let Some(keys) = &self.keyring {
            keys.horse_key(horse)?;
        }
        let mut scoped = self.clone();
        scoped.owner = Some(horse.to_string());
        Ok(scoped)
    }

    pub(crate) fn for_session(&self, session: &str) -> Result<Store> {
        if self.owner.is_some() {
            return Ok(self.clone());
        }
        self.for_horse(&self.session(session)?.horse)
    }

    pub(crate) fn seal_registry(&self, name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        if self.encrypted {
            encryption::seal(self, "registry", name, bytes)
        } else {
            Ok(bytes.to_vec())
        }
    }

    pub(crate) fn open_registry(&self, name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        if self.encrypted {
            encryption::open(self, "registry", name, bytes)
        } else {
            Ok(bytes.to_vec())
        }
    }

    pub(crate) fn seal_mount(&self, horse: &str, path: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        if !self.encrypted {
            bail!("horse mount requires an encrypted store");
        }
        encryption::seal(&self.for_horse(horse)?, "mount", path, bytes)
    }

    fn clean_tmp(&self) {
        let Ok(rd) = std::fs::read_dir(self.root.join("tmp")) else {
            return;
        };
        let now = SystemTime::now();
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| now.duration_since(t).unwrap_or_default() > STALE_TMP)
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// Take the writer lock. Writers of sessions and horses hold it while
    /// they read, change and write them.
    pub fn lock(&self) -> Result<Lock<'_>> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(privfs::FILE_MODE)
            .open(self.root.join("lock"))?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!(
                "cannot lock the horse store: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Lock {
            _file: f,
            store: self,
        })
    }

    fn object_path(&self, h: &Hash) -> PathBuf {
        self.root.join("objects").join(&h.0[..2]).join(&h.0[2..])
    }

    /// Write `bytes` to a new synced file in `tmp/`. The caller links or
    /// renames it into place.
    pub(crate) fn write_tmp(&self, bytes: &[u8]) -> Result<PathBuf> {
        let name = format!(
            "{}-{}",
            std::process::id(),
            super::hex(&super::random_bytes::<8>()?)
        );
        let p = self.root.join("tmp").join(name);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(privfs::FILE_MODE)
            .open(&p)?;
        let r = f.write_all(bytes).and_then(|_| f.sync_all());
        if let Err(e) = r {
            let _ = std::fs::remove_file(&p);
            return Err(e.into());
        }
        Ok(p)
    }

    /// Add an object. An object with the same bytes is there already when
    /// the hash is known: the store keeps one copy.
    pub fn put(&self, meta: &Meta, raw: &[u8]) -> Result<Hash> {
        let bytes = object_bytes(meta, raw)?;
        let h = Hash::of(&bytes);
        let path = self.object_path(&h);
        if path.exists() {
            return Ok(h);
        }
        let dir = path.parent().expect("object path has a parent");
        privfs::dir(dir)?;
        let stored = if self.encrypted {
            index::seal(self, &h, meta, raw, &bytes)?
        } else {
            bytes
        };
        let tmp = self.write_tmp(&stored)?;
        let linked = std::fs::hard_link(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => sync_dir(dir)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).context("cannot add an object to the horse store"),
        }
        Ok(h)
    }

    /// Add a native row with its parent node.
    pub fn put_native(
        &self,
        origin: &Origin,
        parent: Option<&Hash>,
        key: &str,
        raw: &[u8],
    ) -> Result<Hash> {
        let meta = Meta {
            kind: "native".into(),
            harness: Some(origin.harness.into()),
            container: Some(origin.container.into()),
            version: Some(origin.version.into()),
            parent: parent.cloned(),
            key: Some(key.into()),
            media: None,
        };
        self.put(&meta, raw)
    }

    /// Add a row that paddock built from a live channel of the harness
    /// of `origin`, in the row format of its container, with its parent
    /// node. The node is marked [`LIVE`]: it is no native original.
    pub fn put_live(
        &self,
        origin: &Origin,
        parent: Option<&Hash>,
        key: &str,
        raw: &[u8],
    ) -> Result<Hash> {
        let meta = Meta {
            kind: LIVE.into(),
            harness: Some(origin.harness.into()),
            container: Some(origin.container.into()),
            version: Some(origin.version.into()),
            parent: parent.cloned(),
            key: Some(key.into()),
            media: None,
        };
        self.put(&meta, raw)
    }

    /// Add a blob, for example a DNA or a native session header.
    pub fn put_blob(&self, media: &str, raw: &[u8]) -> Result<Hash> {
        self.put(&blob_meta(media), raw)
    }

    /// Read an object and check its hash. A damaged object is an error.
    pub fn get(&self, h: &Hash) -> Result<Node> {
        let path = self.object_path(h);
        let stored = std::fs::read(&path)
            .with_context(|| format!("no node {} in the horse store", h.short()))?;
        let (bytes, public) = if self.encrypted {
            index::open(self, h, &stored)?
        } else {
            (stored, None)
        };
        if Hash::of(&bytes) != *h {
            bail!(
                "node {} is damaged: its content does not match its hash",
                h.short()
            );
        }
        let nl = bytes
            .iter()
            .position(|&b| b == b'\n')
            .with_context(|| format!("node {} has no metadata line", h.short()))?;
        let meta: Meta = serde_json::from_slice(&bytes[..nl])
            .with_context(|| format!("node {} has bad metadata", h.short()))?;
        let node = Node {
            hash: h.clone(),
            meta,
            raw: bytes[nl + 1..].to_vec(),
        };
        if let Some(public) = public {
            index::verify_node(&public, &node)?;
        }
        Ok(node)
    }

    /// The nodes from the root to `head`, root first.
    pub fn chain(&self, head: Option<&Hash>) -> Result<Vec<Node>> {
        let mut out = Vec::new();
        let mut next = head.cloned();
        while let Some(h) = next {
            let n = self.get(&h)?;
            next = n.meta.parent.clone();
            out.push(n);
        }
        out.reverse();
        Ok(out)
    }

    pub(crate) fn object_stored_size(&self, hash: &Hash) -> Result<u64> {
        Ok(std::fs::metadata(self.object_path(hash))?.len())
    }

    pub(crate) fn object_ciphertext(&self, hash: &Hash) -> Result<Vec<u8>> {
        if !self.encrypted {
            bail!("horse object is not encrypted");
        }
        Ok(std::fs::read(self.object_path(hash))?)
    }

    pub(crate) fn indexed_chain(
        &self,
        head: Option<&Hash>,
        limit: u64,
        inspected: &mut u64,
    ) -> Result<Vec<IndexedNode>> {
        let mut out = Vec::new();
        let mut next = head.cloned();
        while let Some(hash) = next {
            let item = match index::read(self, &hash)? {
                Some(info) => info,
                None => {
                    let node = self.get_bounded(&hash, limit, inspected)?;
                    IndexedNode {
                        hash,
                        kind: node.meta.kind,
                        parent: node.meta.parent,
                        size: node.raw.len() as u64,
                        time: None,
                    }
                }
            };
            next = item.parent.clone();
            out.push(item);
        }
        out.reverse();
        Ok(out)
    }

    pub(crate) fn get_bounded(&self, hash: &Hash, limit: u64, inspected: &mut u64) -> Result<Node> {
        let total = inspected
            .checked_add(self.object_stored_size(hash)?)
            .context("peek byte count overflow")?;
        if total > limit {
            bail!("peek grant byte limit reached");
        }
        *inspected = total;
        self.get(hash)
    }

    /// The nodes from the root to `head` for a projection: every node must
    /// be a native original. A node that paddock built from a live channel
    /// is refused, because the harness never wrote it.
    pub fn native_chain(&self, head: Option<&Hash>) -> Result<Vec<Node>> {
        let nodes = self.chain(head)?;
        if let Some(n) = nodes.iter().find(|n| n.is_live()) {
            bail!(
                "node {} is not a native original; persist it with the harness first",
                n.hash.short()
            );
        }
        Ok(nodes)
    }

    /// Write a mutable file (`sessions/x.json`) atomically: a synced
    /// temporary file, a rename, and a synced folder.
    pub(crate) fn write_ref(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        let path = self.root.join(rel);
        let dir = path.parent().expect("ref path has a parent").to_path_buf();
        privfs::dir(&dir)?;
        let stored = if self.encrypted {
            encryption::seal(self, "ref", rel, bytes)?
        } else {
            bytes.to_vec()
        };
        let tmp = self.write_tmp(&stored)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("cannot write {}", path.display()));
        }
        sync_dir(&dir)
    }

    pub(crate) fn read_ref(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(self.root.join(rel)) {
            Ok(b) if self.encrypted => Ok(Some(encryption::open(self, "ref", rel, &b)?)),
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {rel} in the horse store")),
        }
    }

    /// Remove a mutable file. A missing file is no error.
    pub(crate) fn remove_ref(&self, rel: &str) -> Result<()> {
        match std::fs::remove_file(self.root.join(rel)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot remove {rel} in the horse store"))
            }
            _ => Ok(()),
        }
    }

    /// The names of the files in a folder of the store, sorted.
    pub(crate) fn list_refs(&self, dir: &str) -> Result<Vec<String>> {
        let mut out: Vec<String> = std::fs::read_dir(self.root.join(dir))?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        out.sort();
        Ok(out)
    }
}

/// The bytes of an object: the metadata line, a newline and the body.
fn object_bytes(meta: &Meta, raw: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(meta)?;
    bytes.push(b'\n');
    bytes.extend_from_slice(raw);
    Ok(bytes)
}

/// The hash that `put_blob(media, raw)` gives, without a write.
pub fn blob_hash(media: &str, raw: &[u8]) -> Result<Hash> {
    Ok(Hash::of(&object_bytes(&blob_meta(media), raw)?))
}

fn blob_meta(media: &str) -> Meta {
    Meta {
        kind: "blob".into(),
        harness: None,
        container: None,
        version: None,
        parent: None,
        key: None,
        media: Some(media.into()),
    }
}

/// Sync a folder, so a new name in it survives a crash.
pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("cannot sync {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privfs::tests::{mode, with_umask_022};

    const O: Origin = Origin {
        harness: "dsh",
        container: "test-jsonl",
        version: "1",
    };

    #[test]
    fn objects_dedupe_by_hash_and_link_parents() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::open(t.path()).unwrap();
        let a = s.put_native(&O, None, "seq:0", b"{\"a\":1}").unwrap();
        let a2 = s.put_native(&O, None, "seq:0", b"{\"a\":1}").unwrap();
        assert_eq!(a, a2);
        let b = s.put_native(&O, Some(&a), "seq:1", b"{\"b\":2}").unwrap();
        // The same row on another parent is another node.
        let b_other = s.put_native(&O, None, "seq:1", b"{\"b\":2}").unwrap();
        assert_ne!(b, b_other);
        let chain = s.chain(Some(&b)).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].hash, a);
        assert_eq!(chain[1].raw, b"{\"b\":2}");
        assert_eq!(chain[1].meta.parent.as_ref(), Some(&a));
        assert_eq!(chain[1].meta.key.as_deref(), Some("seq:1"));
        let files = std::fs::read_dir(t.path().join("objects"))
            .unwrap()
            .flatten()
            .map(|d| std::fs::read_dir(d.path()).unwrap().count())
            .sum::<usize>();
        assert_eq!(files, 3);
    }

    #[test]
    fn store_is_private_under_umask_022() {
        let t = tempfile::tempdir().unwrap();
        with_umask_022(|| {
            let root = t.path().join("horse");
            let s = Store::open(&root).unwrap();
            let h = s.put_blob("toml", b"x = 1\n").unwrap();
            s.write_ref("sessions/a.json", b"{}").unwrap();
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(&root.join("objects")), 0o700);
            assert_eq!(mode(&s.object_path(&h)), 0o600);
            assert_eq!(mode(&root.join("sessions/a.json")), 0o600);
        });
    }

    #[test]
    fn rows_stay_byte_exact() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::open(t.path()).unwrap();
        let raw = b"{\"x\":\"\\u00e9\\n\",\"y\":1.50}\r\0\xff";
        let h = s.put_native(&O, None, "seq:0", raw).unwrap();
        assert_eq!(s.get(&h).unwrap().raw, raw);
    }

    #[test]
    fn a_crash_in_the_middle_leaves_no_object_and_no_half_ref() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::open(t.path()).unwrap();
        s.write_ref("sessions/a.json", b"{\"head\":1}").unwrap();
        // A writer dies after its temporary file and before the link or the
        // rename: readers see the old state only.
        let tmp = s.write_tmp(b"{\"head\":2").unwrap();
        assert_eq!(
            s.read_ref("sessions/a.json").unwrap().unwrap(),
            b"{\"head\":1}"
        );
        assert_eq!(s.list_refs("sessions").unwrap(), vec!["a.json"]);
        // A new open keeps a fresh temporary file (another writer may own
        // it) and removes an old one.
        Store::open(t.path()).unwrap();
        assert!(tmp.exists());
        let old = SystemTime::now() - Duration::from_secs(7200);
        File::options()
            .write(true)
            .open(&tmp)
            .unwrap()
            .set_modified(old)
            .unwrap();
        Store::open(t.path()).unwrap();
        assert!(!tmp.exists());
        // Writing again works.
        s.write_ref("sessions/a.json", b"{\"head\":2}").unwrap();
        assert_eq!(
            s.read_ref("sessions/a.json").unwrap().unwrap(),
            b"{\"head\":2}"
        );
    }

    #[test]
    fn a_damaged_object_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::open(t.path()).unwrap();
        let h = s.put_blob("text", b"hello").unwrap();
        let p = s.object_path(&h);
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.truncate(bytes.len() - 2);
        std::fs::write(&p, bytes).unwrap();
        let err = s.get(&h).unwrap_err().to_string();
        assert!(
            err.contains("authentication") || err.contains("damaged"),
            "{err}"
        );
    }

    #[test]
    fn a_chain_with_a_live_node_is_no_native_chain() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::open(t.path()).unwrap();
        let a = s.put_native(&O, None, "seq:0", b"a").unwrap();
        assert_eq!(s.native_chain(Some(&a)).unwrap().len(), 1);
        let b = s.put_live(&O, Some(&a), "seq:1", b"b").unwrap();
        // The same bytes as a native row are another node.
        assert_ne!(b, s.put_native(&O, Some(&a), "seq:1", b"b").unwrap());
        let chain = s.chain(Some(&b)).unwrap();
        assert!(!chain[0].is_live());
        assert!(chain[1].is_live());
        assert_eq!(chain[1].meta.kind, LIVE);
        assert_eq!(chain[1].raw, b"b");
        let c = s.put_native(&O, Some(&b), "seq:2", b"c").unwrap();
        let err = s.native_chain(Some(&c)).unwrap_err().to_string();
        assert_eq!(
            err,
            format!(
                "node {} is not a native original; persist it with the harness first",
                b.short()
            )
        );
    }

    #[test]
    fn hash_parse_refuses_other_text() {
        assert!(Hash::parse("../etc").is_err());
        assert!(Hash::parse(&"A".repeat(64)).is_err());
        let h = Hash::of(b"x");
        assert_eq!(Hash::parse(h.as_str()).unwrap(), h);
    }
}
