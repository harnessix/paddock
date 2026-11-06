//! POSIX ACLs for `protect = "uid"`: the helper account reaches the
//! working folder of a start, and the user keeps full access to what the
//! helper writes there.
//!
//! - A grant runs as the user, never as root (tackd starts `paddock tack
//!   acl-grant` with the user's uid): it can only change files that the
//!   user owns, so a request cannot make tackd open a folder of root or of
//!   another user to the helper.
//! - It adds a named user entry for the helper to the folder and, inside
//!   it, to every folder and file of the user on the same file system: `r`
//!   and `x` for folders and programs, `w` too when the entry says
//!   `write = true`. Folders get the same entry as a default ACL, so new
//!   files inherit it, and with `write` a default entry `rwx` for the user,
//!   so the user keeps full access to files that the helper creates.
//! - The folders above it get `x` (enter, not list) for the helper when
//!   others cannot enter them already.
//! - Each grant is in the ledger (`acl.jsonl` in tackd's state folder)
//!   before it starts, so that `sudo paddock tack remove` revokes it even
//!   when a crash cut it short.
//! - A revoke runs as root: it removes every entry for the helper, the
//!   default entry for the user that a write grant added, and gives files
//!   that the helper created to the user.
//!
//! Every file is opened without following a link (`O_PATH|O_NOFOLLOW`,
//! then by `openat` from its open folder) and changed through that
//! descriptor, so a link that appears in the middle of the walk is not
//! followed, also not by root. Only folders and regular files change.
//!
//! The format of the ACL attribute (`system.posix_acl_access`) is a plain
//! function with tests that run on every system; the walk is Linux only.

use serde::{Deserialize, Serialize};

pub const ACCESS: &str = "system.posix_acl_access";
pub const DEFAULT: &str = "system.posix_acl_default";

const VERSION: u32 = 2;
pub const USER_OBJ: u16 = 0x01;
pub const USER: u16 = 0x02;
pub const GROUP_OBJ: u16 = 0x04;
pub const GROUP: u16 = 0x08;
pub const MASK: u16 = 0x10;
pub const OTHER: u16 = 0x20;
const NO_ID: u32 = u32::MAX;

pub const R: u16 = 4;
pub const W: u16 = 2;
pub const X: u16 = 1;

/// The most files that one grant or revoke changes.
pub const MAX_FILES: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub tag: u16,
    pub id: u32,
    pub perm: u16,
}

/// An ACL, its entries in the order that the kernel wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acl(pub Vec<Entry>);

impl Acl {
    /// The ACL that a mode means, without an attribute.
    pub fn from_mode(mode: u32) -> Acl {
        let p = |shift: u32| ((mode >> shift) & 7) as u16;
        Acl(vec![
            Entry {
                tag: USER_OBJ,
                id: NO_ID,
                perm: p(6),
            },
            Entry {
                tag: GROUP_OBJ,
                id: NO_ID,
                perm: p(3),
            },
            Entry {
                tag: OTHER,
                id: NO_ID,
                perm: p(0),
            },
        ])
    }

    pub fn decode(b: &[u8]) -> Result<Acl, String> {
        if b.len() < 4 || !(b.len() - 4).is_multiple_of(8) {
            return Err(format!("an ACL of {} bytes", b.len()));
        }
        let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if v != VERSION {
            return Err(format!("an ACL of version {v}"));
        }
        let mut out = Vec::new();
        for c in b[4..].as_chunks::<8>().0 {
            out.push(Entry {
                tag: u16::from_le_bytes([c[0], c[1]]),
                perm: u16::from_le_bytes([c[2], c[3]]),
                id: u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            });
        }
        Ok(Acl(out))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = VERSION.to_le_bytes().to_vec();
        for e in &self.0 {
            b.extend_from_slice(&e.tag.to_le_bytes());
            b.extend_from_slice(&e.perm.to_le_bytes());
            let id = if matches!(e.tag, USER | GROUP) {
                e.id
            } else {
                NO_ID
            };
            b.extend_from_slice(&id.to_le_bytes());
        }
        b
    }

    fn sort(&mut self) {
        self.0.sort_by_key(|e| (e.tag, e.id));
    }

    fn get(&self, tag: u16, id: u32) -> Option<u16> {
        self.0
            .iter()
            .find(|e| e.tag == tag && (e.id == id || !matches!(tag, USER | GROUP)))
            .map(|e| e.perm)
    }

    /// The permissions of the named user entry of `uid`.
    pub fn user(&self, uid: u32) -> Option<u16> {
        self.get(USER, uid)
    }

    /// Whether it has a named user or group entry.
    pub fn named(&self) -> bool {
        self.0.iter().any(|e| matches!(e.tag, USER | GROUP))
    }

    /// The mask that `setfacl -m` computes: every permission of the group
    /// class. Without named entries there is no mask.
    fn remask(&mut self) {
        self.0.retain(|e| e.tag != MASK);
        if self.named() {
            let m = self
                .0
                .iter()
                .filter(|e| matches!(e.tag, USER | GROUP | GROUP_OBJ))
                .fold(0, |m, e| m | e.perm);
            self.0.push(Entry {
                tag: MASK,
                id: NO_ID,
                perm: m,
            });
        }
        self.sort();
    }

    /// Give `uid` at least `perm` (its old permissions stay). Returns
    /// whether anything changed.
    pub fn add_user(&mut self, uid: u32, perm: u16) -> bool {
        let before = self.clone();
        match self.0.iter_mut().find(|e| e.tag == USER && e.id == uid) {
            Some(e) => e.perm |= perm,
            None => self.0.push(Entry {
                tag: USER,
                id: uid,
                perm,
            }),
        }
        self.remask();
        *self != before
    }

    /// Remove the named entry of `uid`. Returns whether it was there.
    pub fn remove_user(&mut self, uid: u32) -> bool {
        let n = self.0.len();
        self.0.retain(|e| !(e.tag == USER && e.id == uid));
        if self.0.len() == n {
            return false;
        }
        self.remask();
        true
    }

    /// Whether it gives `uid` at least `perm`, through the mask.
    pub fn grants(&self, uid: u32, perm: u16) -> bool {
        let mask = self.get(MASK, NO_ID).unwrap_or(7);
        self.user(uid).is_some_and(|p| p & mask & perm == perm)
    }
}

/// The permissions that a grant gives the helper on one file: read, write
/// with `write`, and `x` for a folder or a file that someone may run.
pub fn perm_for(dir: bool, mode: u32, write: bool) -> u16 {
    let mut p = R;
    if write {
        p |= W;
    }
    if dir || mode & 0o111 != 0 {
        p |= X;
    }
    p
}

/// What a grant did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantReport {
    /// The folder had the entry already: nothing was walked.
    pub already: bool,
    pub changed: usize,
    /// Files of other users, which the user cannot change.
    pub not_owned: usize,
    /// Entries that the walk did not change: links, devices, other file
    /// systems.
    pub skipped: usize,
    pub limit: bool,
    /// The first errors.
    pub errors: Vec<String>,
}

impl GrantReport {
    /// One line for the audit.
    pub fn summary(&self) -> String {
        if self.already {
            return "ACL of the folder already in place".into();
        }
        let mut s = format!("ACL added to {} files", self.changed);
        if self.not_owned > 0 {
            s.push_str(&format!(
                ", {} files of other users left as they are",
                self.not_owned
            ));
        }
        if self.limit {
            s.push_str(&format!(", stopped at {MAX_FILES} files"));
        }
        if !self.errors.is_empty() {
            s.push_str(&format!(", {} errors", self.errors.len()));
        }
        s
    }
}

/// What a revoke did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevokeReport {
    pub changed: usize,
    /// Files of the helper that now belong to the user.
    pub given_back: usize,
    pub errors: Vec<String>,
}

#[cfg(target_os = "linux")]
#[path = "acl_linux.rs"]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{grant, probe, read, revoke, Ledger};

#[cfg(test)]
#[path = "acl_tests.rs"]
mod tests;
