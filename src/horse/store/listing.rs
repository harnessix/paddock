//! The writer lock, and the lists of mutable files under it.
//!
//! A writer replaces `sessions/<id>.json` by a rename. So do the writers of
//! horses, ctl and peek grants, memory names and views. On some file
//! systems (btrfs) the new name gets a new place in the folder, and a
//! listing that runs at that moment does not see the file at all. So a
//! list of such a folder takes the lock shared: it waits for the
//! writer, and the writer waits for it. A thread that holds the writer
//! lock lists without a second lock, because `flock` on a second file
//! descriptor of the same process waits for the first one forever.

use super::{Lock, Store};
use crate::privfs;
use anyhow::{bail, Result};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::io::AsRawFd as _;

thread_local! {
    /// The lock files (device, inode) that this thread holds exclusively.
    static HELD: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

fn flock(f: &File, op: libc::c_int) -> Result<()> {
    // SAFETY: flock operates on this live file descriptor.
    if unsafe { libc::flock(f.as_raw_fd(), op) } != 0 {
        bail!(
            "cannot lock the horse store: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

fn id(f: &File) -> Result<(u64, u64)> {
    let m = f.metadata()?;
    Ok((m.dev(), m.ino()))
}

pub(super) fn writer_lock(store: &Store) -> Result<Lock<'_>> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(privfs::FILE_MODE)
        .open(store.root.join("lock"))?;
    flock(&f, libc::LOCK_EX)?;
    let key = id(&f)?;
    HELD.with(|h| h.borrow_mut().push(key));
    Ok(Lock {
        _file: f,
        key,
        store,
    })
}

impl Drop for Lock<'_> {
    fn drop(&mut self) {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            if let Some(i) = h.iter().position(|k| *k == self.key) {
                h.swap_remove(i);
            }
        });
    }
}

impl Store {
    /// The names in a folder of mutable files, sorted, read while no
    /// writer changes them. Every writer of that folder holds the store
    /// lock while it renames a file into it.
    pub(crate) fn list_locked_refs(&self, dir: &str) -> Result<Vec<String>> {
        let path = self.root.join("lock");
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .mode(privfs::FILE_MODE)
                    .open(&path)
                {
                    Ok(f) => f,
                    // A store that cannot have a lock file has no writer.
                    Err(e) if e.kind() == std::io::ErrorKind::ReadOnlyFilesystem => {
                        return self.list_refs(dir)
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        };
        let key = id(&f)?;
        if !HELD.with(|h| h.borrow().contains(&key)) {
            flock(&f, libc::LOCK_SH)?;
        }
        self.list_refs(dir)
    }
}

/// A folder for the race tests on the file system of the build, not in
/// `/tmp`. `/tmp` is often tmpfs, where a rename never hides a file from a
/// listing, so the races of FLAKE3 do not show there.
#[cfg(test)]
pub(crate) fn race_dir() -> tempfile::TempDir {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("race-")
        .tempdir_in(base)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::race_dir;
    use crate::horse::Store;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// The race of FLAKE3: a listing next to a writer that replaces the
    /// session file. Without the shared lock, btrfs drops the session from
    /// some listings. tmpfs does not, so this test can pass there anyway.
    #[test]
    fn a_listing_sees_a_session_that_a_writer_replaces() {
        let t = race_dir();
        let st = Store::open(&t.path().join("store")).unwrap();
        let horse = st.new_default_horse().unwrap().id;
        let b = crate::horse::claude::binding(Some("native".into()));
        let id = st.new_session(&horse, b).unwrap().id;
        let done = Arc::new(AtomicBool::new(false));
        let w = {
            let (st, done) = (st.clone(), done.clone());
            std::thread::spawn(move || {
                for _ in 0..300 {
                    st.update_session(&id, |_| {}).unwrap();
                }
                done.store(true, Ordering::Release);
            })
        };
        let mut misses = 0;
        while !done.load(Ordering::Acquire) {
            misses += usize::from(st.sessions().unwrap().is_empty());
        }
        w.join().unwrap();
        assert_eq!(misses, 0, "listings without the session");
    }

    /// A writer lists under its own lock, and the lock is free after it.
    #[test]
    fn a_writer_lists_under_its_own_lock() {
        let t = tempfile::tempdir().unwrap();
        let st = Store::open(&t.path().join("store")).unwrap();
        let horse = st.new_default_horse().unwrap().id;
        {
            let l = st.lock().unwrap();
            assert_eq!(st.horses().unwrap().len(), 1);
            let b = crate::horse::claude::binding(None);
            l.new_session(&horse, b).unwrap();
            assert_eq!(st.sessions().unwrap().len(), 1);
        }
        let other = st.clone();
        let n = std::thread::spawn(move || other.sessions().unwrap().len())
            .join()
            .unwrap();
        assert_eq!(n, 1);
    }
}
