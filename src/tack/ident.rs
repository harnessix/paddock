//! The identity of the program that a launch request names: its real path,
//! the file (device, inode), the SHA-256 of its content, its owner, and on
//! macOS the identity that signed it.
//!
//! tackd opens the real file once, without following a link at the last
//! step, and hashes what it opened: a file that a link swap replaces after
//! the path was resolved is not the one that gets hashed. The starter later
//! checks that it runs the same device and inode.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The largest program file that tackd hashes.
pub const MAX_PROGRAM: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    /// The path that the request named.
    pub path: PathBuf,
    /// The path with every link resolved.
    pub real: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub sha256: String,
    /// Why a process of `uid` (or of anyone) could change this file or put
    /// another one in its place, or None.
    pub changeable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentError {
    /// Not there, or not a file: said the same way for every path, so that
    /// a client cannot probe files it cannot see.
    NotFound,
    NotExecutable,
    TooLarge,
    Unreadable(String),
}

impl std::fmt::Display for IdentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentError::NotFound => write!(f, "the program is not there"),
            IdentError::NotExecutable => write!(f, "the program is not an executable file"),
            IdentError::TooLarge => write!(f, "the program is larger than {MAX_PROGRAM} bytes"),
            IdentError::Unreadable(e) => write!(f, "the program cannot be read: {e}"),
        }
    }
}

/// The key of a cached hash: the file and every stamp that a write changes.
type Key = (u64, u64, i64, i64, i64, i64, u64);

/// Hashes of program files, so that a large program is not read at every
/// start. A write changes the change time, so a changed file is read again.
#[derive(Default)]
pub struct HashCache(Mutex<HashMap<Key, String>>);

const CACHE_MAX: usize = 512;

impl HashCache {
    fn get(&self, k: &Key) -> Option<String> {
        self.0.lock().ok()?.get(k).cloned()
    }
    fn put(&self, k: Key, h: String) {
        if let Ok(mut m) = self.0.lock() {
            if m.len() >= CACHE_MAX {
                m.clear();
            }
            m.insert(k, h);
        }
    }
}

/// Identify the program at `path` for a request of `uid`.
pub fn identify(path: &Path, uid: u32, cache: &HashCache) -> Result<Ident, IdentError> {
    let real = std::fs::canonicalize(path).map_err(|_| IdentError::NotFound)?;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&real)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ENOENT) | Some(libc::ELOOP) => IdentError::NotFound,
            _ => IdentError::Unreadable(e.to_string()),
        })?;
    let m = f
        .metadata()
        .map_err(|e| IdentError::Unreadable(e.to_string()))?;
    if !m.is_file() {
        return Err(IdentError::NotFound);
    }
    if m.mode() & 0o111 == 0 {
        return Err(IdentError::NotExecutable);
    }
    if m.len() > MAX_PROGRAM {
        return Err(IdentError::TooLarge);
    }
    let key: Key = (
        m.dev(),
        m.ino(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.len(),
    );
    let sha256 = match cache.get(&key) {
        Some(h) => h,
        None => {
            let h =
                hash(f.take(MAX_PROGRAM + 1)).map_err(|e| IdentError::Unreadable(e.to_string()))?;
            cache.put(key, h.clone());
            h
        }
    };
    Ok(Ident {
        path: path.to_path_buf(),
        changeable: changeable(&real, &m, uid),
        real,
        dev: m.dev(),
        ino: m.ino(),
        sha256,
    })
}

/// The SHA-256 of a program file that is open already, through the cache:
/// the starter checks with it that the file it runs is the one that was
/// decided on.
pub fn identify_open(
    f: &std::fs::File,
    m: &std::fs::Metadata,
    cache: &HashCache,
) -> Result<String, IdentError> {
    if m.len() > MAX_PROGRAM {
        return Err(IdentError::TooLarge);
    }
    let key: Key = (
        m.dev(),
        m.ino(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.len(),
    );
    if let Some(h) = cache.get(&key) {
        return Ok(h);
    }
    let h = hash(f.take(MAX_PROGRAM + 1)).map_err(|e| IdentError::Unreadable(e.to_string()))?;
    cache.put(key, h.clone());
    Ok(h)
}

fn hash(mut r: impl Read) -> std::io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(crate::manifest::hex(&h.finalize()))
}

/// Why `uid` (or anyone who is not root) could change the file at `real`,
/// or replace it through one of its folders. Root-owned files in
/// root-owned folders that only root writes are safe without a hash.
pub fn changeable(real: &Path, m: &std::fs::Metadata, uid: u32) -> Option<String> {
    let who = |owner: u32| {
        if owner == uid {
            "you own it".to_string()
        } else {
            format!("user {owner} owns it")
        }
    };
    if m.uid() != 0 {
        return Some(format!("{}: {}", real.display(), who(m.uid())));
    }
    if m.mode() & 0o022 != 0 {
        return Some(format!(
            "{}: its group or others can write it",
            real.display()
        ));
    }
    let mut dir = real.parent();
    while let Some(d) = dir {
        let Ok(dm) = std::fs::metadata(d) else {
            return Some(format!("{}: cannot be checked", d.display()));
        };
        if dm.uid() != 0 {
            return Some(format!("the folder {}: {}", d.display(), who(dm.uid())));
        }
        // A sticky folder (/tmp) lets others add files, not replace ours.
        if dm.mode() & 0o022 != 0 && dm.mode() & 0o1000 == 0 {
            return Some(format!(
                "the folder {}: its group or others can write it",
                d.display()
            ));
        }
        dir = d.parent();
    }
    None
}

/// The signing identity in the output of `codesign -dvv PATH` (it writes
/// to stderr): the first `Authority=` line. None for an unsigned or ad-hoc
/// signed program.
pub fn parse_signer(codesign: &str) -> Option<String> {
    codesign
        .lines()
        .find_map(|l| l.strip_prefix("Authority="))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The signer of a program on macOS, after `codesign --verify --strict`
/// has checked the signature against the file. None when it is unsigned,
/// the signature does not verify, or this is not macOS.
pub fn signer(real: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let codesign = Path::new("/usr/bin/codesign");
        let run = |args: &[&std::ffi::OsStr]| {
            std::process::Command::new(codesign)
                .args(args)
                .env_clear()
                .stdin(std::process::Stdio::null())
                .output()
                .ok()
        };
        use std::ffi::OsStr;
        let p = real.as_os_str();
        let ok = run(&[OsStr::new("--verify"), OsStr::new("--strict"), p])?;
        if !ok.status.success() {
            return None;
        }
        let out = run(&[OsStr::new("-dvv"), p])?;
        parse_signer(&String::from_utf8_lossy(&out.stderr))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = real;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn exe(p: &Path, body: &str) {
        std::fs::write(p, body).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_link_is_resolved_and_the_real_file_is_hashed() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("gh-2.0");
        exe(&real, "#!/bin/true\n");
        let link = t.path().join("gh");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let cache = HashCache::default();
        let id = identify(&link, 1000, &cache).unwrap();
        assert_eq!(id.real, std::fs::canonicalize(&real).unwrap());
        assert_eq!(id.path, link);
        assert_eq!(
            id.sha256,
            super::super::program::sha256_file(&real).unwrap()
        );
        // The tests run as a user that owns the file: it is changeable.
        assert!(id.changeable.is_some());
    }

    #[test]
    fn a_changed_file_is_hashed_again() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("tool");
        exe(&p, "one");
        let cache = HashCache::default();
        let a = identify(&p, 1, &cache).unwrap().sha256;
        assert_eq!(identify(&p, 1, &cache).unwrap().sha256, a);
        // Same size, new content: the change time differs.
        std::thread::sleep(std::time::Duration::from_millis(20));
        exe(&p, "two");
        assert_ne!(identify(&p, 1, &cache).unwrap().sha256, a);
    }

    #[test]
    fn missing_folders_and_plain_files_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let cache = HashCache::default();
        assert_eq!(
            identify(&t.path().join("no"), 1, &cache),
            Err(IdentError::NotFound)
        );
        assert_eq!(identify(t.path(), 1, &cache), Err(IdentError::NotFound));
        let p = t.path().join("data");
        std::fs::write(&p, "x").unwrap();
        assert_eq!(identify(&p, 1, &cache), Err(IdentError::NotExecutable));
        // A fifo would block a reader: it is not a file.
        let fifo = t.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o755) }, 0);
        assert_eq!(identify(&fifo, 1, &cache), Err(IdentError::NotFound));
    }

    #[test]
    fn root_owned_system_programs_are_not_changeable() {
        // /usr/bin/env exists on every Linux and macOS host, owned by root.
        let p = Path::new("/usr/bin/env");
        let Ok(m) = std::fs::metadata(p) else { return };
        if m.uid() != 0 {
            return;
        }
        let real = std::fs::canonicalize(p).unwrap();
        let m = std::fs::metadata(&real).unwrap();
        assert_eq!(changeable(&real, &m, 1000), None);
    }

    #[test]
    fn a_user_owned_program_or_folder_is_changeable() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("tool");
        exe(&p, "x");
        let m = std::fs::metadata(&p).unwrap();
        let me = unsafe { libc::getuid() };
        let why = changeable(&p, &m, me).unwrap();
        if me != 0 {
            assert!(why.contains("you own it"), "{why}");
            assert!(changeable(&p, &m, me + 1)
                .unwrap()
                .contains(&format!("user {me} owns it")));
        }
    }

    #[test]
    fn the_signer_is_the_first_authority_line() {
        let out = "Executable=/usr/local/bin/gh\nIdentifier=gh\nAuthority=Developer ID Application: GitHub (VEKTX9H2N7)\nAuthority=Developer ID Certification Authority\nAuthority=Apple Root CA\n";
        assert_eq!(
            parse_signer(out).as_deref(),
            Some("Developer ID Application: GitHub (VEKTX9H2N7)")
        );
        assert_eq!(parse_signer("Signature=adhoc\n"), None);
        assert_eq!(parse_signer("Authority=\n"), None);
    }
}
