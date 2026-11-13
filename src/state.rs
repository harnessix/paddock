//! Per-sandbox state on the host: sandbox homes, credential sync, the list of
//! running sandboxes, and the watch list of auto-run files.

use crate::env::HostEnv;
use crate::manifest::hex;
use crate::profile::Profile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn slug(project: &Path) -> String {
    let base: String = project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(32)
        .collect();
    let h = hex(&Sha256::digest(project.to_string_lossy().as_bytes()));
    format!("{base}-{}", &h[..10])
}

pub fn new_id(harness: &str, project: &Path) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!("{nanos}-{}-{}", std::process::id(), project.display());
    let h = hex(&Sha256::digest(seed.as_bytes()));
    let base: String = slug(project)
        .split('-')
        .next()
        .unwrap_or("p")
        .chars()
        .take(16)
        .collect();
    format!("{harness}-{base}-{}", &h[..6]).to_lowercase()
}

/// The sandbox `$HOME` for one harness in one project. Session history stays
/// here, so one project's transcripts are not visible from another.
pub fn sandbox_home(
    env: &HostEnv,
    harness: &str,
    auth: &AuthMode,
    project: &Path,
) -> Result<PathBuf> {
    // One home per login, so the account state of one login never shows in another.
    let dir = env
        .paddock_data()
        .join("homes")
        .join(harness)
        .join(crate::storage::login_dir(auth))
        .join(slug(project));
    crate::privfs::dir(&dir)?;
    Ok(dir)
}

/// Which login a sandbox uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    /// The host's own login. Credential files in the host home are the store.
    Shared,
    /// A separate login kept by paddock, with a name for several accounts.
    Isolated(String),
    /// Another account folder of the host, as Omarchy's account switch
    /// selects it (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME`). The
    /// folder is the store, like the home in the shared mode. `key` names
    /// the sandbox home; `dir` is empty when the mode was read back from a
    /// record.
    Account { key: String, dir: PathBuf },
}

impl AuthMode {
    /// The account mode for a host folder.
    pub fn account(dir: &Path) -> Self {
        Self::Account {
            key: slug(dir),
            dir: dir.to_path_buf(),
        }
    }

    /// The host's own logins: the home or an account folder of the host.
    pub fn is_host_login(&self) -> bool {
        matches!(self, Self::Shared | Self::Account { .. })
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl std::str::FromStr for AuthMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        if let Some(key) = s.strip_prefix("account:") {
            if !valid_name(key) {
                anyhow::bail!("account key '{key}' must use letters, digits, '-' or '_'");
            }
            return Ok(Self::Account {
                key: key.to_string(),
                dir: PathBuf::new(),
            });
        }
        let name = match s {
            "shared" => return Ok(Self::Shared),
            "isolated" => "default",
            other => other.strip_prefix("isolated:").ok_or_else(|| {
                anyhow::anyhow!("unknown auth mode '{s}': use shared, isolated or isolated:NAME")
            })?,
        };
        if !valid_name(name) {
            anyhow::bail!("auth name '{name}' must use letters, digits, '-' or '_'");
        }
        Ok(Self::Isolated(name.to_string()))
    }
}

impl std::fmt::Display for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shared => write!(f, "shared"),
            Self::Isolated(n) => write!(f, "isolated:{n}"),
            Self::Account { key, .. } => write!(f, "account:{key}"),
        }
    }
}

/// The directory that holds the credential files for a mode.
pub fn auth_root(env: &HostEnv, harness: &str, mode: &AuthMode) -> PathBuf {
    match mode {
        AuthMode::Shared | AuthMode::Account { .. } => env.home.clone(),
        AuthMode::Isolated(name) => env.paddock_data().join("auth").join(harness).join(name),
    }
}

/// The isolated logins in paddock's own store, as `harness` and `name`.
pub fn isolated_logins(env: &HostEnv) -> Vec<(String, String)> {
    let names = |dir: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        v.sort();
        v
    };
    let root = env.paddock_data().join("auth");
    names(&root)
        .into_iter()
        .flat_map(|h| {
            names(&root.join(&h))
                .into_iter()
                .map(move |n| (h.clone(), n))
        })
        .collect()
}

/// The account folder for `rel`, a path relative to HOME in the profile, in
/// the account mode (`[state.account]`). None in the other modes, and for
/// paths that do not move with the account.
fn account_file(profile: &Profile, mode: &AuthMode, rel: &str) -> Option<PathBuf> {
    match (mode, &profile.state.account) {
        (AuthMode::Account { dir, .. }, Some(a)) if !dir.as_os_str().is_empty() => a.map(dir, rel),
        _ => None,
    }
}

/// The host file that a settings entry (`import_ro`, `seed`, `json_import`)
/// reads. In the account mode, the files of the account folder come from the
/// selected folder.
pub fn host_file(env: &HostEnv, profile: &Profile, mode: &AuthMode, rel: &str) -> PathBuf {
    account_file(profile, mode, rel).unwrap_or_else(|| env.home.join(rel))
}

/// The stored copy of a credential file (`auth`) for a mode.
pub fn auth_file(env: &HostEnv, profile: &Profile, mode: &AuthMode, rel: &str) -> PathBuf {
    account_file(profile, mode, rel)
        .unwrap_or_else(|| auth_root(env, &profile.name, mode).join(rel))
}

/// The hash of a regular file, streamed, of at most 1 GiB. A FIFO or a device
/// is never opened, so this cannot hang the cleanup after a run.
fn file_hash(p: &Path) -> Option<String> {
    hash_open(p, true)
}

fn hash_open(p: &Path, follow: bool) -> Option<String> {
    let f = crate::safefs::open_file(p, follow).ok()??;
    let mut h = Sha256::new();
    let mut r = std::io::Read::take(f, 1 << 30);
    std::io::copy(&mut r, &mut h).ok()?;
    Some(hex(&h.finalize()))
}

/// The state of a watched file in the sandbox-writable project. A symlink is
/// not followed: its target text is the state, so a planted link shows as a
/// change. A FIFO or a device shows as a change too and is never opened.
fn watch_hash(p: &Path) -> Option<String> {
    let m = std::fs::symlink_metadata(p).ok()?;
    if m.file_type().is_symlink() {
        let t = std::fs::read_link(p).ok()?;
        return Some(format!("link:{}", t.display()));
    }
    if !m.is_file() {
        use std::os::unix::fs::MetadataExt;
        return Some(format!("special:{:o}", m.mode() & 0o170000));
    }
    hash_open(p, false).or_else(|| Some("unreadable".into()))
}

fn copy_private(from: &Path, to: &Path) -> Result<()> {
    // Write next to the target, then rename: the same atomic pattern the
    // harnesses use, so a crash never leaves half a token. The copy is 0600
    // from its first byte.
    crate::privfs::copy_atomic(from, to)
        .with_context(|| format!("copy {} -> {}", from.display(), to.display()))
}

/// Write bytes to a host path that only the host can write, atomically.
fn write_private(bytes: &[u8], to: &Path) -> Result<()> {
    crate::privfs::write_atomic(to, bytes).with_context(|| format!("write {}", to.display()))
}

/// Where a credential is stored on the host.
enum Store {
    /// A path that only the host can write: the home or paddock's store.
    Host,
    /// A file in an account folder of the host (the account mode). Every
    /// access goes through `safefs` from that folder, so no symlink in it is
    /// followed.
    Account { root: PathBuf, rel: PathBuf },
}

impl Store {
    fn for_file(mode: &AuthMode, stored: &Path) -> Self {
        match mode {
            AuthMode::Account { dir, .. } if !dir.as_os_str().is_empty() => {
                match stored.strip_prefix(dir) {
                    Ok(rel) if !rel.as_os_str().is_empty() => Store::Account {
                        root: dir.clone(),
                        rel: rel.to_path_buf(),
                    },
                    _ => Store::Host,
                }
            }
            _ => Store::Host,
        }
    }

    fn read(&self, stored: &Path) -> Result<Option<Vec<u8>>> {
        match self {
            Store::Host => {
                if !stored.is_file() {
                    return Ok(None);
                }
                std::fs::read(stored)
                    .map(Some)
                    .with_context(|| format!("cannot read {}", stored.display()))
            }
            Store::Account { root, rel } => {
                if !root.is_dir() {
                    return Ok(None);
                }
                crate::safefs::Root::open(root)?.read(rel, crate::safefs::MAX_READ)
            }
        }
    }

    fn hash(&self, stored: &Path) -> Option<String> {
        match self {
            Store::Host => file_hash(stored),
            Store::Account { .. } => self
                .read(stored)
                .ok()
                .flatten()
                .map(|b| hex(&Sha256::digest(&b))),
        }
    }

    fn write(&self, stored: &Path, bytes: &[u8]) -> Result<()> {
        match self {
            Store::Host => write_private(bytes, stored),
            Store::Account { root, rel } => crate::safefs::Root::open(root)?
                .write(rel, bytes, 0o600)
                .with_context(|| format!("write {}", stored.display())),
        }
    }
}

struct AuthItem {
    store: Store,
    stored: PathBuf,
    /// Relative to the sandbox home, which the sandbox can write.
    rel: PathBuf,
    /// The hashes of the sandbox copy and of the store at the last sync
    /// (at the start, then after each `sync_now`).
    inside_at_start: Option<String>,
    stored_at_start: Option<String>,
    /// A login that a keeper keeps (keeper.rs): the sandbox got it without
    /// its refresh token, and it is never copied back.
    kept: bool,
    /// The profile's expiry field for this file (`[state] auth_expiry`).
    expiry: Option<String>,
}

/// Credentials are copied, not bind-mounted: harnesses replace token files
/// with a rename, which a single-file bind mount does not survive.
///
/// While the sandbox runs, `sync_now` keeps the copy and the store the same
/// in both directions. Many OAuth logins rotate their refresh token: the
/// old one stops working at the first refresh. A copy that went back only
/// at the end left the store, and every sandbox that started meanwhile,
/// with a dead token.
///
/// The sandbox home is sandbox-controlled: every access to it goes through
/// `safefs`, so a symlink planted there is never followed by the host.
pub struct AuthSync {
    pub mode: AuthMode,
    home: PathBuf,
    /// Where the locks between paddock processes on one login live.
    locks: PathBuf,
    items: Vec<AuthItem>,
    /// Folder entries (`auth = ["dir/"]`): the folder relative to the
    /// sandbox home, and its store on the host. A login that the sandbox
    /// adds there is copied out too.
    folders: Vec<(PathBuf, PathBuf)>,
    /// Files with flags that only go from false to true (`done_flags`).
    flags: Vec<FlagItem>,
}

/// The names of the regular files in a host folder. Symlinks are left out.
fn host_file_names(dir: &Path, store: &Store) -> Vec<String> {
    let names: Vec<std::ffi::OsString> = match store {
        Store::Account { root, rel } => crate::safefs::Root::open(root)
            .and_then(|r| r.dir(rel, false))
            .and_then(|d| d.file_names())
            .unwrap_or_default(),
        Store::Host => std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name())
            .collect(),
    };
    names
        .into_iter()
        .filter_map(|n| n.into_string().ok())
        .collect()
}

/// The names of the regular files in a folder of the sandbox home.
fn inside_file_names(home: &crate::safefs::Root, rel: &Path) -> Vec<String> {
    home.dir(rel, false)
        .and_then(|d| d.file_names())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.into_string().ok())
        .collect()
}

/// The hash of a credential file inside the sandbox home. A symlink or a
/// special file in its place is an error, never followed.
fn inside_hash(root: &crate::safefs::Root, rel: &Path) -> Result<Option<String>> {
    Ok(root
        .read(rel, crate::safefs::MAX_READ)?
        .map(|b| hex(&Sha256::digest(&b))))
}

pub struct SyncResult {
    pub saved: Vec<PathBuf>,
    /// Files that changed in the store while the sandbox ran, where the
    /// sandbox copy is not newer. The store wins.
    pub conflicts: Vec<PathBuf>,
}

/// One step of `sync_now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStep {
    /// The sandbox copy went to the store.
    ToStore(PathBuf),
    /// The store went into the sandbox.
    ToSandbox(PathBuf),
}

/// When a login expires: the field at the dotted `path` of a JSON file
/// (`claudeAiOauth.expiresAt`), a number in seconds or milliseconds, or an
/// RFC 3339 time. Seconds. None when the field is missing or the file is
/// not JSON. Only the field that the profile names counts: a file can hold
/// other logins (MCP servers) whose expiry says nothing about this one.
pub fn login_expiry(bytes: &[u8], path: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let field = path.split('.').try_fold(&v, |cur, k| cur.get(k))?;
    let t = match field {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => {
            crate::forage::parse_time(s).or_else(|| s.parse::<u64>().ok())
        }
        _ => None,
    }?;
    // Milliseconds since 1970 are above 1e12.
    Some(if t > 1_000_000_000_000 { t / 1000 } else { t })
}

/// Which of two versions of a login file expires later, by the profile's
/// field. None when that is not known.
fn later(a: &[u8], b: &[u8], path: Option<&str>) -> Option<bool> {
    let path = path?;
    match (login_expiry(a, path), login_expiry(b, path)) {
        (Some(x), Some(y)) if x != y => Some(x > y),
        _ => None,
    }
}

fn is_json(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok()
}

/// A lock between the paddock processes that sync the same stored login:
/// held while one of them reads, compares and writes it.
struct StoreLock {
    /// Held for its lock, which the kernel drops with the file.
    _file: std::fs::File,
}

impl StoreLock {
    fn take(dir: &Path, stored: &Path) -> Option<Self> {
        let name = hex(&Sha256::digest(stored.as_os_str().as_encoded_bytes()));
        crate::privfs::dir(dir).ok()?;
        let f = crate::privfs::append(&dir.join(format!("{}.lock", &name[..16]))).ok()?;
        // SAFETY: flock on an open file descriptor that this struct owns.
        (unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX) } == 0)
            .then_some(Self { _file: f })
    }
}

impl AuthSync {
    pub fn copy_in(
        env: &HostEnv,
        profile: &Profile,
        mode: &AuthMode,
        home_src: &Path,
    ) -> Result<Self> {
        let home = crate::safefs::Root::open(home_src)?;
        let mut items = Vec::new();
        let mut folders = Vec::new();
        let mut rels = Vec::new();
        for rel in &profile.state.auth {
            // A folder of credential files whose names the profile cannot
            // know, such as one file per provider.
            if let Some(dir) = rel.strip_suffix('/') {
                let stored = auth_file(env, profile, mode, dir);
                let store = Store::for_file(mode, &stored);
                let mut names = host_file_names(&stored, &store);
                for n in inside_file_names(&home, Path::new(dir)) {
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
                names.sort();
                rels.extend(names.into_iter().map(|n| format!("{dir}/{n}")));
                folders.push((PathBuf::from(dir), stored));
            } else {
                rels.push(rel.clone());
            }
        }
        for rel in &rels {
            let stored = auth_file(env, profile, mode, rel);
            let store = Store::for_file(mode, &stored);
            let rel = PathBuf::from(rel);
            let mut kept = false;
            if let Some(bytes) = store.read(&stored)? {
                // A login that the keeper refreshes: the sandbox gets it
                // without the refresh token.
                let keep = crate::keeper::kind_for(&profile.name)
                    .filter(|k| Path::new(k.file) == rel)
                    .filter(|_| crate::keeper::Cfg::load(env).is_ok_and(|c| c.enabled))
                    .and_then(|_| crate::keeper::strip(&bytes));
                kept = keep.is_some();
                home.write(&rel, keep.as_deref().unwrap_or(&bytes), 0o600)
                    .with_context(|| format!("credential {}", rel.display()))?;
            } else if mode.is_host_login() {
                // The host is logged out: do not keep an older login inside.
                home.remove(&rel)?;
            }
            items.push(AuthItem {
                inside_at_start: inside_hash(&home, &rel)?,
                stored_at_start: store.hash(&stored),
                expiry: profile
                    .state
                    .auth_expiry
                    .get(rel.to_string_lossy().as_ref())
                    .cloned(),
                store,
                stored,
                rel,
                kept,
            });
        }
        let mut flags = Vec::new();
        for (rel, keys) in &profile.state.done_flags {
            let stored = auth_file(env, profile, mode, rel);
            let it = FlagItem {
                store: Store::for_file(mode, &stored),
                stored,
                rel: PathBuf::from(rel),
                keys: keys.clone(),
            };
            it.to_sandbox(&home);
            flags.push(it);
        }
        Ok(Self {
            mode: mode.clone(),
            home: home_src.to_path_buf(),
            locks: env.paddock_state().join("locks"),
            items,
            folders,
            flags,
        })
    }

    /// The logins that a keeper keeps for this sandbox.
    pub fn kept_logins(&self) -> Vec<HeldLogin> {
        self.items
            .iter()
            .filter(|i| i.kept)
            .map(|i| HeldLogin {
                home: self.home.clone(),
                rel: i.rel.clone(),
                store: i.stored.clone(),
            })
            .collect()
    }

    /// The host logins that the sandbox got a copy of. None for an isolated
    /// login, which the host never reads.
    pub fn host_logins(&self) -> Vec<HeldLogin> {
        if !self.mode.is_host_login() {
            return Vec::new();
        }
        self.items
            .iter()
            .map(|i| HeldLogin {
                home: self.home.clone(),
                rel: i.rel.clone(),
                store: i.stored.clone(),
            })
            .collect()
    }

    /// Bring the sandbox copies and the store into line, while the sandbox
    /// runs. A side that changed since the last sync goes to the other one.
    /// When both changed, the one that expires later wins; when neither file
    /// says when it expires, nothing moves and the end of the run decides.
    /// A file that a side removed is never removed on the other side. Logins
    /// that a keeper keeps are left to the keeper.
    pub fn sync_now(&mut self) -> Vec<SyncStep> {
        let mut steps = Vec::new();
        let Ok(home) = crate::safefs::Root::open(&self.home) else {
            return steps;
        };
        for it in self.items.iter_mut().filter(|i| !i.kept) {
            // One process at a time reads, compares and writes this login.
            let _lock = StoreLock::take(&self.locks, &it.stored);
            let inside = match home.read(&it.rel, crate::safefs::MAX_READ) {
                Ok(b) => b,
                Err(e) => {
                    log::debug!("login sync: {}: {e:#}", it.rel.display());
                    continue;
                }
            };
            let stored = it.store.read(&it.stored).ok().flatten();
            let (Some(inside), Some(stored)) = (inside, stored) else {
                continue;
            };
            // Only JSON logins move while the harness runs. A database file
            // (SQLite) is open in the harness; it waits for the end.
            if !is_json(&inside) || !is_json(&stored) {
                continue;
            }
            let (hi, hs) = (
                Some(hex(&Sha256::digest(&inside))),
                Some(hex(&Sha256::digest(&stored))),
            );
            if hi == hs {
                (it.inside_at_start, it.stored_at_start) = (hi, hs);
                continue;
            }
            let inside_changed = hi != it.inside_at_start;
            let store_changed = hs != it.stored_at_start;
            let inside_wins = match (inside_changed, store_changed) {
                (true, false) => true,
                (false, true) => false,
                (false, false) => continue,
                (true, true) => match later(&inside, &stored, it.expiry.as_deref()) {
                    Some(w) => w,
                    None => continue,
                },
            };
            // Read the target again right before the write: a refresh that
            // landed since the first read must not be overwritten.
            let done = if inside_wins {
                if it.store.hash(&it.stored) != hs {
                    continue;
                }
                it.store
                    .write(&it.stored, &inside)
                    .map(|_| SyncStep::ToStore(it.stored.clone()))
            } else {
                if inside_hash(&home, &it.rel).ok().flatten() != hi {
                    continue;
                }
                home.write(&it.rel, &stored, 0o600)
                    .map(|_| SyncStep::ToSandbox(it.stored.clone()))
            };
            match done {
                Ok(step) => {
                    let h = if inside_wins { hi } else { hs };
                    (it.inside_at_start, it.stored_at_start) = (h.clone(), h);
                    steps.push(step);
                }
                Err(e) => log::warn!("login sync: {}: {e:#}", it.rel.display()),
            }
        }
        for f in &self.flags {
            f.to_store(&home, &self.locks);
        }
        steps
    }

    /// Copy back every credential that changed inside the sandbox, for example
    /// after a login or a token refresh. If the store changed too (the harness
    /// also runs on the host), the store keeps its version.
    pub fn copy_out(&self) -> Result<SyncResult> {
        let mut r = SyncResult {
            saved: vec![],
            conflicts: vec![],
        };
        let home = crate::safefs::Root::open(&self.home)?;
        // Logins that the sandbox added to a folder entry.
        let mut added = Vec::new();
        for (dir, stored_dir) in &self.folders {
            for n in inside_file_names(&home, dir) {
                let rel = dir.join(&n);
                if self.items.iter().any(|i| i.rel == rel) {
                    continue;
                }
                let stored = stored_dir.join(&n);
                let store = Store::for_file(&self.mode, &stored);
                added.push(AuthItem {
                    stored_at_start: store.hash(&stored),
                    inside_at_start: None,
                    store,
                    stored,
                    rel,
                    kept: false,
                    expiry: None,
                });
            }
        }
        for it in self.items.iter().chain(added.iter()) {
            // The keeper wrote the store; the sandbox's copy has no refresh
            // token and must never replace it.
            if it.kept {
                continue;
            }
            // A symlink or special file in place of a credential is refused:
            // reading through it would copy a host file into the store.
            let bytes = match home.read(&it.rel, crate::safefs::MAX_READ) {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(e) => {
                    log::warn!("not saving the credential {}: {e:#}", it.rel.display());
                    continue;
                }
            };
            let now = Some(hex(&Sha256::digest(&bytes)));
            if now == it.inside_at_start {
                continue;
            }
            let _lock = StoreLock::take(&self.locks, &it.stored);
            if it.store.hash(&it.stored) != it.stored_at_start {
                // Both changed: the copy wins only when it expires later.
                let stored = it.store.read(&it.stored).ok().flatten().unwrap_or_default();
                let newer = later(&bytes, &stored, it.expiry.as_deref()) == Some(true);
                if !newer {
                    r.conflicts.push(it.stored.clone());
                    continue;
                }
            }
            it.store.write(&it.stored, &bytes)?;
            r.saved.push(it.stored.clone());
        }
        for f in &self.flags {
            f.to_store(&home, &self.locks);
        }
        Ok(r)
    }
}

/// The largest `done_flags` file that paddock reads.
const FLAGS_MAX: u64 = 64 * 1024;

/// A `done_flags` file: its store (the host home, an account folder or the
/// isolated store, as for `auth`) and the keys that only go to true.
struct FlagItem {
    store: Store,
    stored: PathBuf,
    rel: PathBuf,
    keys: Vec<String>,
}

type JsonMap = serde_json::Map<String, serde_json::Value>;

/// A `done_flags` file as a JSON object. Ok(None): missing. Err: larger
/// than `FLAGS_MAX`, not JSON, not an object, or not readable.
fn read_flags(bytes: Result<Option<Vec<u8>>>) -> Result<Option<(Vec<u8>, JsonMap)>> {
    let Some(b) = bytes? else {
        return Ok(None);
    };
    if b.len() as u64 > FLAGS_MAX {
        anyhow::bail!("larger than {FLAGS_MAX} bytes");
    }
    match serde_json::from_slice::<serde_json::Value>(&b)? {
        serde_json::Value::Object(m) => Ok(Some((b, m))),
        _ => anyhow::bail!("not a JSON object"),
    }
}

/// The listed keys that are `true` in `from` and not `true` in `to`.
fn missing_true<'a>(keys: &'a [String], from: &JsonMap, to: Option<&JsonMap>) -> Vec<&'a str> {
    let on = |m: &JsonMap, k: &str| m.get(k) == Some(&serde_json::Value::Bool(true));
    keys.iter()
        .filter(|k| on(from, k) && !to.is_some_and(|t| on(t, k)))
        .map(String::as_str)
        .collect()
}

/// `to` with the keys set to true, or a new object with the keys when `to` is missing.
fn with_true(to: Option<JsonMap>, keys: &[&str]) -> Result<Vec<u8>> {
    let mut m = to.unwrap_or_default();
    for k in keys {
        m.insert((*k).to_string(), serde_json::Value::Bool(true));
    }
    Ok(serde_json::to_vec_pretty(&serde_json::Value::Object(m))?)
}

impl FlagItem {
    fn read_store(&self) -> Result<Option<Vec<u8>>> {
        self.store.read(&self.stored)
    }

    fn skip(&self, side: &str, e: anyhow::Error) {
        log::debug!(
            "done flags: skipping {} in {side}: {e:#}",
            self.rel.display()
        );
    }

    /// At start: the keys that are true in the store become true in the
    /// sandbox copy.
    fn to_sandbox(&self, home: &crate::safefs::Root) {
        let src = match read_flags(self.read_store()) {
            Ok(Some((_, m))) => m,
            Ok(None) => return,
            Err(e) => return self.skip("the store", e),
        };
        let (before, inside) = match read_flags(home.read(&self.rel, FLAGS_MAX)) {
            Ok(Some((b, m))) => (Some(b), Some(m)),
            Ok(None) => (None, None),
            Err(e) => return self.skip("the sandbox", e),
        };
        let keys = missing_true(&self.keys, &src, inside.as_ref());
        if keys.is_empty() {
            return;
        }
        let bytes = match with_true(inside, &keys) {
            Ok(b) => b,
            Err(e) => return self.skip("the sandbox", e),
        };
        // Read the target again right before the write.
        if home.read(&self.rel, FLAGS_MAX).ok().flatten() != before {
            return;
        }
        if let Err(e) = home.write(&self.rel, &bytes, 0o600) {
            log::warn!("done flags: {}: {e:#}", self.rel.display());
        }
    }

    /// The keys that are true in the sandbox become true in the store. True
    /// when it wrote.
    fn to_store(&self, home: &crate::safefs::Root, locks: &Path) -> bool {
        let inside = match read_flags(home.read(&self.rel, FLAGS_MAX)) {
            Ok(Some((_, m))) => m,
            Ok(None) => return false,
            Err(e) => {
                self.skip("the sandbox", e);
                return false;
            }
        };
        if missing_true(&self.keys, &inside, None).is_empty() {
            return false;
        }
        let _lock = StoreLock::take(locks, &self.stored);
        let (before, stored) = match read_flags(self.read_store()) {
            Ok(Some((b, m))) => (Some(b), Some(m)),
            Ok(None) => (None, None),
            Err(e) => {
                self.skip("the store", e);
                return false;
            }
        };
        let keys = missing_true(&self.keys, &inside, stored.as_ref());
        if keys.is_empty() {
            return false;
        }
        let bytes = match with_true(stored, &keys) {
            Ok(b) => b,
            Err(e) => {
                self.skip("the store", e);
                return false;
            }
        };
        // Read the target again right before the write.
        if self.read_store().ok().flatten() != before {
            return false;
        }
        match self.store.write(&self.stored, &bytes) {
            Ok(()) => {
                log::info!("saved the done flags {keys:?} to {}", self.stored.display());
                true
            }
            Err(e) => {
                log::warn!("done flags: {}: {e:#}", self.stored.display());
                false
            }
        }
    }
}

/// Copy the host's credential files for a harness into an isolated store.
pub fn auth_import(env: &HostEnv, profile: &Profile, mode: &AuthMode) -> Result<Vec<PathBuf>> {
    if mode.is_host_login() {
        anyhow::bail!("the shared mode already uses the host login; import into an isolated store, for example --auth isolated:work");
    }
    let root = auth_root(env, &profile.name, mode);
    let mut done = Vec::new();
    for rel in &profile.state.auth {
        let host = env.home.join(rel);
        if host.is_file() {
            copy_private(&host, &root.join(rel))?;
            done.push(host);
        }
    }
    if root.is_dir() {
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(done)
}

/// Copy `seed` paths from the host home into the sandbox home.
pub fn seed(env: &HostEnv, profile: &Profile, mode: &AuthMode, home_src: &Path) -> Result<()> {
    let home = crate::safefs::Root::open(home_src)?;
    for rel in &profile.state.seed {
        let host = host_file(env, profile, mode, rel);
        if host.exists() {
            home.copy_in(&host, Path::new(rel))
                .with_context(|| format!("seed {rel}"))?;
        }
    }
    Ok(())
}

pub fn json_get<'a>(v: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value> {
    path.iter().try_fold(v, |cur, k| cur.get(k))
}

pub(crate) fn json_set(v: &mut serde_json::Value, path: &[String], val: serde_json::Value) {
    let mut cur = v;
    for (i, k) in path.iter().enumerate() {
        if !cur.is_object() {
            *cur = serde_json::json!({});
        }
        let obj = cur.as_object_mut().unwrap();
        if i + 1 == path.len() {
            obj.insert(k.clone(), val);
            return;
        }
        cur = obj
            .entry(k.clone())
            .or_insert_with(|| serde_json::json!({}));
    }
}

pub fn expand_path(path: &[String], project: &Path) -> Vec<String> {
    path.iter()
        .map(|k| k.replace("{project}", &project.display().to_string()))
        .collect()
}

/// Copy selected keys from host JSON files into the sandbox copies.
pub fn json_import(
    env: &HostEnv,
    profile: &Profile,
    mode: &AuthMode,
    home_src: &Path,
    project: &Path,
) -> Result<()> {
    for ji in &profile.state.json_import {
        let host_file = host_file(env, profile, mode, &ji.file);
        let Ok(text) = std::fs::read_to_string(&host_file) else {
            continue;
        };
        let Ok(host): std::result::Result<serde_json::Value, _> = serde_json::from_str(&text)
        else {
            continue;
        };
        let home = crate::safefs::Root::open(home_src)?;
        let rel = Path::new(&ji.file);
        let mut inside: serde_json::Value = home
            .read(rel, crate::safefs::MAX_READ)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let mut keys: Vec<&Vec<String>> = ji.keys.iter().collect();
        if mode.is_host_login() {
            keys.extend(ji.shared_keys.iter());
        }
        for k in keys {
            let k = expand_path(k, project);
            if let Some(v) = json_get(&host, &k) {
                json_set(&mut inside, &k, v.clone());
            }
        }
        home.write(
            rel,
            serde_json::to_string_pretty(&inside)?.as_bytes(),
            0o600,
        )?;
    }
    Ok(())
}

/// A minimal git configuration with only the identity, so commits work but
/// host credential helpers, aliases and includes stay outside. Pushes go to
/// the host through the `paddock::` remote helper (see broker/git.rs).
pub fn write_gitconfig(
    env: &HostEnv,
    home_src: &Path,
    repo: Option<&crate::broker::git::Repo>,
) -> Result<()> {
    let (name, email) = env.git_identity();
    let mut text = String::from("# Written by paddock on each start. Edits are lost.\n[user]\n");
    if let Some(n) = name {
        text.push_str(&format!("\tname = {n}\n"));
    }
    if let Some(e) = email {
        text.push_str(&format!("\temail = {e}\n"));
    }
    text.push_str("[init]\n\tdefaultBranch = main\n");
    text.push_str(&crate::broker::git::sandbox_gitconfig(repo));
    crate::safefs::Root::open(home_src)?.write(Path::new(".gitconfig"), text.as_bytes(), 0o644)?;
    Ok(())
}

pub fn snapshot(paths: &[PathBuf]) -> BTreeMap<PathBuf, Option<String>> {
    paths.iter().map(|p| (p.clone(), watch_hash(p))).collect()
}

pub fn changed(before: &BTreeMap<PathBuf, Option<String>>) -> Vec<PathBuf> {
    before
        .iter()
        .filter(|(p, h)| watch_hash(p) != **h)
        .map(|(p, _)| p.clone())
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Running {
    pub id: String,
    pub harness: String,
    pub project: PathBuf,
    pub backend: String,
    pub network: String,
    pub gui: String,
    pub pid: u32,
    pub started: u64,
    #[serde(default)]
    pub auth: String,
    /// Who approves the agent's steps: `auto`, `manual`, or `command`
    /// (`paddock wrap` and desktop apps: the command decides). A record
    /// of a paddock without approval modes reads as `auto`.
    #[serde(default = "auto_approvals")]
    pub approvals: String,
    /// The nested compositor of `window` mode, so the panel can focus it.
    #[serde(default)]
    pub window_pid: Option<u32>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The paddock process. Its ancestors include the terminal that runs it.
    #[serde(default)]
    pub host_pid: u32,
    /// For a sandbox that another sandbox started (`paddock-host spawn`):
    /// the id of that sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The id that the parent uses for it with `paddock-host status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_id: Option<String>,
    /// The host logins that this sandbox holds a copy of (the main login or
    /// an Omarchy account). While it runs, paddock checks their limits for
    /// the Omarchy agents panel from its side (omarchy_usage.rs).
    #[serde(default)]
    pub logins: Vec<HeldLogin>,
    /// The systemd scope that holds the sandbox (`paddock-…`), which
    /// `paddock freeze` freezes. None for container backends and without a
    /// systemd user instance.
    #[serde(default)]
    pub unit: Option<String>,
    /// The project of paddock kv for this sandbox (the repository of a
    /// worktree), where paddock writes the sandbox's state.
    #[serde(default)]
    pub kv_project: Option<PathBuf>,
    /// The pasture the sandbox joined (`--pasture NAME`). None: a pasture
    /// of its own.
    #[serde(default)]
    pub pasture: Option<String>,
    /// The credentials whose values the sandbox holds (`deliver`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<String>,
    /// The credentials that the sandbox uses through the gateway: it holds
    /// only stand-ins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gateway: Vec<String>,
    /// The credentials that the sandbox uses through the ssh agent channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent: Vec<String>,
    /// The credentials that the sandbox reads as files in
    /// `/dev/paddock/secret` (`deliver`, `as = "file"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_files: Vec<String>,
    /// Logins that a keeper keeps for this sandbox (keeper.rs): the sandbox
    /// holds them without their refresh token, and the keeper writes the
    /// new access token here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_logins: Vec<HeldLogin>,
    /// The caller gave paddock a terminal, and a window the user can see
    /// belongs to this process chain. False when there is no terminal, or
    /// the terminal belongs to a background process.
    #[serde(default)]
    pub watched: bool,
    /// paddock relays an invisible terminal. The harness has its own pty.
    /// The caller still sends and receives bytes, and owns the input.
    #[serde(default)]
    pub relay: bool,
    /// macOS: the window number of the terminal window that runs this
    /// sandbox, recorded at the start (mark.rs). The marker marks it.
    #[serde(default)]
    pub terminal_window: Option<u32>,
    /// macOS: the terminal app that owns `terminal_window`.
    #[serde(default)]
    pub terminal_pid: Option<u32>,
}

fn auto_approvals() -> String {
    crate::approvals::Approvals::Auto.name().into()
}

/// A host login file and the sandbox's copy of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldLogin {
    /// The sandbox home. The sandbox can write it, so it is read through
    /// `safefs` only.
    pub home: PathBuf,
    /// The copy, relative to `home`.
    pub rel: PathBuf,
    /// The login file on the host.
    pub store: PathBuf,
}

/// A project where a sandbox started, for the panel's "start again" list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recent {
    pub project: PathBuf,
    pub harness: String,
    pub last: u64,
}

fn recent_file(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("recent.json")
}

pub fn recent(env: &HostEnv) -> Vec<Recent> {
    let mut v: Vec<Recent> = std::fs::read_to_string(recent_file(env))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    v.retain(|r| r.project.is_dir());
    v
}

/// Remember the project and harness of a start. Keeps the 20 most recent projects.
pub fn record_recent(env: &HostEnv, project: &Path, harness: &str) -> Result<()> {
    let mut v = recent(env);
    v.retain(|r| r.project != project);
    v.insert(
        0,
        Recent {
            project: project.to_path_buf(),
            harness: harness.to_string(),
            last: now(),
        },
    );
    v.truncate(20);
    let f = recent_file(env);
    crate::privfs::write(&f, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

/// The parent of a process, from /proc.
pub fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name can contain spaces and parentheses; fields follow the last ')'.
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

impl Running {
    pub fn file(env: &HostEnv, id: &str) -> PathBuf {
        env.paddock_run().join(id).join("info.json")
    }

    pub fn register(&self, env: &HostEnv) -> Result<()> {
        let f = Self::file(env, &self.id);
        crate::privfs::write(&f, serde_json::to_string(self)?)?;
        Ok(())
    }

    pub fn list(env: &HostEnv) -> Vec<Running> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(env.paddock_run()) else {
            return out;
        };
        for e in entries.flatten() {
            let f = e.path().join("info.json");
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let Ok(r) = serde_json::from_str::<Running>(&text) else {
                continue;
            };
            if alive(r.pid) {
                out.push(r);
            } else if r.host_pid > 1 && alive(r.host_pid) {
                // The sandbox ended, and its paddock process runs the end
                // steps. The run folder stays until they finish: the end
                // of a Seatbelt run reads it to find the processes of the
                // run (tmpclaim.rs, TMP1-F3).
            } else {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
        out.sort_by_key(|r| r.started);
        out
    }
}

pub fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Running sandboxes with each child right after its parent, in start
/// order. A child whose parent is gone is listed at the top level.
pub fn tree_order(list: Vec<Running>) -> Vec<Running> {
    fn add(r: &Running, all: &[Running], out: &mut Vec<Running>, depth: usize) {
        if depth > 8 || out.iter().any(|o| o.id == r.id) {
            return;
        }
        out.push(r.clone());
        for c in all.iter().filter(|c| c.parent.as_deref() == Some(&r.id)) {
            add(c, all, out, depth + 1);
        }
    }
    let roots: Vec<&Running> = list
        .iter()
        .filter(|r| {
            !r.parent
                .as_deref()
                .is_some_and(|p| list.iter().any(|o| o.id == p))
        })
        .collect();
    // The members of one pasture follow each other, where the first of
    // them is.
    let key = |r: &Running| r.pasture.clone().map(|n| (r.kv_project.clone(), n));
    let mut out = Vec::new();
    for (i, r) in roots.iter().enumerate() {
        if out.iter().any(|o: &Running| o.id == r.id) {
            continue;
        }
        add(r, &list, &mut out, 0);
        if let Some(k) = key(r) {
            for other in roots[i + 1..]
                .iter()
                .filter(|o| key(o).as_ref() == Some(&k))
            {
                add(other, &list, &mut out, 0);
            }
        }
    }
    out
}

/// How many running ancestors a sandbox has in `list`.
pub fn depth_in(list: &[Running], r: &Running) -> usize {
    let mut d = 0;
    let mut cur = r.parent.clone();
    while let Some(p) = cur {
        match list.iter().find(|o| o.id == p) {
            Some(o) if d < 8 => {
                d += 1;
                cur = o.parent.clone();
            }
            _ => break,
        }
    }
    d
}

/// The running members of the pasture `name` of `project`.
pub fn pasture_members<'a>(list: &'a [Running], project: &Path, name: &str) -> Vec<&'a Running> {
    list.iter()
        .filter(|r| r.pasture.as_deref() == Some(name) && r.kv_project.as_deref() == Some(project))
        .collect()
}

/// Every running sandbox that `id` started, and the ones they started.
pub fn descendants(list: &[Running], id: &str) -> Vec<Running> {
    let mut out: Vec<Running> = Vec::new();
    let mut queue = vec![id.to_string()];
    while let Some(p) = queue.pop() {
        for c in list.iter().filter(|c| c.parent.as_deref() == Some(&p)) {
            if !out.iter().any(|o| o.id == c.id) && out.len() < 64 {
                out.push(c.clone());
                queue.push(c.id.clone());
            }
        }
    }
    out
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// True when the secret-looking variable names for a project differ from the
/// last notice. Records the new list.
pub fn env_notice_changed(env: &HostEnv, project: &Path, names: &[String]) -> bool {
    let mut sorted = names.to_vec();
    sorted.sort();
    let hash = hex(&Sha256::digest(sorted.join("\n").as_bytes()));
    let file = env.paddock_state().join("env-notice").join(slug(project));
    if std::fs::read_to_string(&file)
        .map(|s| s.trim() == hash)
        .unwrap_or(false)
    {
        return false;
    }
    let _ = crate::privfs::write(&file, &hash);
    true
}

/// Add or remove names in `[env] hide` of the user configuration file,
/// keeping the rest of the file as it is.
pub fn edit_env_hide(env: &HostEnv, add: &[String], remove: &[String]) -> Result<Vec<String>> {
    let file = env.paddock_config().join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is invalid", file.display()))?;
    let envt = doc.entry("env").or_insert(toml_edit::table());
    let envt = envt
        .as_table_mut()
        .context("[env] in the configuration is not a table")?;
    let arr = envt
        .entry("hide")
        .or_insert(toml_edit::value(toml_edit::Array::new()));
    let arr = arr.as_array_mut().context("[env] hide is not an array")?;
    let mut names: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    for a in add {
        if !names.contains(a) {
            names.push(a.clone());
        }
    }
    names.retain(|n| !remove.contains(n));
    names.sort();
    arr.clear();
    for n in &names {
        arr.push(n.as_str());
    }
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, doc.to_string())?;
    Ok(names)
}

/// Set `[apps.NAME] roots` in the user configuration, keeping the rest.
pub fn set_app_roots(env: &HostEnv, name: &str, roots: &[PathBuf]) -> Result<()> {
    let file = env.paddock_config().join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is invalid", file.display()))?;
    let apps = doc.entry("apps").or_insert(toml_edit::table());
    let apps = apps.as_table_mut().context("[apps] is not a table")?;
    apps.set_implicit(true);
    let app = apps.entry(name).or_insert(toml_edit::table());
    let app = app.as_table_mut().context("[apps.NAME] is not a table")?;
    let mut arr = toml_edit::Array::new();
    for r in roots {
        arr.push(r.display().to_string());
    }
    app.insert("roots", toml_edit::value(arr));
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, doc.to_string())?;
    Ok(())
}

/// Apply `toml_set` entries to files in the sandbox home, keeping the rest of
/// each file as it is. `manual`: the run's approvals mode is manual, so the
/// `trust` entries stay out and the harness asks its own trust question.
///
/// The sandbox home is sandbox-controlled: the file is read and replaced
/// through `safefs`, so a symlink left there is never followed.
pub fn toml_set(profile: &Profile, home_src: &Path, project: &Path, manual: bool) -> Result<()> {
    let home = crate::safefs::Root::open(home_src)?;
    for t in profile
        .state
        .toml_set
        .iter()
        .filter(|t| !(t.trust && manual))
    {
        let file = home_src.join(&t.file);
        let rel = Path::new(&t.file);
        let text = match home.read(rel, crate::safefs::MAX_READ)? {
            Some(b) => {
                String::from_utf8(b).with_context(|| format!("{} is not UTF-8", file.display()))?
            }
            None => String::new(),
        };
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .with_context(|| format!("{} is invalid", file.display()))?;
        let path = expand_path(&t.path, project);
        let Some((last, parents)) = path.split_last() else {
            continue;
        };
        let mut table = doc.as_table_mut();
        for k in parents {
            let item = table.entry(k).or_insert(toml_edit::table());
            if let Some(t) = item.as_table_mut() {
                t.set_implicit(true);
            }
            table = item
                .as_table_mut()
                .with_context(|| format!("{k} in {} is not a table", file.display()))?;
        }
        table.insert(last, toml_edit::value(t.value.as_str()));
        home.write(rel, doc.to_string().as_bytes(), 0o600)?;
    }
    Ok(())
}

#[cfg(test)]
mod running_tests;
#[cfg(test)]
mod toml_set_tests;

#[cfg(test)]
mod auth_sync_tests;
#[cfg(test)]
mod toml_set_link_tests;
