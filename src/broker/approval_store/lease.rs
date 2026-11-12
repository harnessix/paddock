//! Prepare the owner lease before entering the global store lock.
use super::*;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

pub(super) struct PreparedLease {
    path: PathBuf,
    file: Option<File>,
}

impl PreparedLease {
    pub(super) fn new(dir: &Path) -> io::Result<Self> {
        crate::privfs::dir(dir)?;
        let path = dir.join(format!(".lease-{}", crate::broker::new_request_id()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(crate::privfs::FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        if let Err(e) = file
            .write_all(LEASE.as_bytes())
            .and_then(|()| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = io::Error::last_os_error();
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    // The target is never opened under the store lock. The hard link must
    // name the inode already locked by this helper, including after a race.
    pub(super) fn publish(mut self, owner: &Path) -> io::Result<File> {
        std::fs::hard_link(&self.path, owner)?;
        let actual = std::fs::symlink_metadata(owner)?;
        let file = self.file.as_ref().expect("prepared lease");
        let expected = file.metadata()?;
        if !actual.file_type().is_file()
            || actual.dev() != expected.dev()
            || actual.ino() != expected.ino()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "owner lease identity changed",
            ));
        }
        std::fs::remove_file(&self.path)?;
        Ok(self.file.take().expect("prepared lease"))
    }
}

impl Drop for PreparedLease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
