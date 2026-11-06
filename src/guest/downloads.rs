//! The `Downloads` folder of the guest home is a link to the `downloads`
//! share, so every download of the app lands in `profile/downloads/` on the
//! host. The golden build makes the link (`configure` step `downloads-link`),
//! and the agent checks it at each boot, because a macOS update of the golden
//! can put a plain folder back. Pure, with a Linux test.

use std::io;
use std::path::{Path, PathBuf};

/// The folder name in the home.
pub const DOWNLOADS: &str = "Downloads";

/// What the check found.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The link is right.
    Linked,
    /// The link was missing or wrong and is made now. A folder that stood
    /// there had its files moved into the share first.
    Relinked,
    /// The share is not mounted yet: ask again later.
    Waiting,
}

/// The guest path of the downloads share.
pub fn share_path() -> PathBuf {
    Path::new(crate::macvm::proto::SHARE_ROOT).join("downloads")
}

/// Make `home/Downloads` a link to `target`, when `target` is mounted.
pub fn ensure_link(home: &Path, target: &Path) -> io::Result<Outcome> {
    if !target.is_dir() {
        return Ok(Outcome::Waiting);
    }
    let link = home.join(DOWNLOADS);
    match std::fs::symlink_metadata(&link) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(md) if md.file_type().is_symlink() => {
            if std::fs::read_link(&link)? == target {
                return Ok(Outcome::Linked);
            }
            std::fs::remove_file(&link)?;
        }
        Ok(md) if md.is_dir() => {
            for e in std::fs::read_dir(&link)? {
                let e = e?;
                let dst = target.join(e.file_name());
                if dst.exists() {
                    continue;
                }
                std::fs::rename(e.path(), &dst)?;
            }
            std::fs::remove_dir(&link)?;
        }
        Ok(_) => std::fs::remove_file(&link)?,
    }
    std::os::unix::fs::symlink(target, &link)?;
    Ok(Outcome::Relinked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_made_kept_and_repaired() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let share = t.path().join("share");
        std::fs::create_dir_all(&home).unwrap();
        // The share is not mounted yet: nothing happens.
        assert_eq!(ensure_link(&home, &share).unwrap(), Outcome::Waiting);
        assert!(!home.join("Downloads").exists());
        std::fs::create_dir_all(&share).unwrap();
        assert_eq!(ensure_link(&home, &share).unwrap(), Outcome::Relinked);
        assert_eq!(std::fs::read_link(home.join("Downloads")).unwrap(), share);
        assert_eq!(ensure_link(&home, &share).unwrap(), Outcome::Linked);
        // A macOS update put a folder with a file back: the file moves
        // into the share and the link returns.
        std::fs::remove_file(home.join("Downloads")).unwrap();
        std::fs::create_dir(home.join("Downloads")).unwrap();
        std::fs::write(home.join("Downloads/got.zip"), "z").unwrap();
        assert_eq!(ensure_link(&home, &share).unwrap(), Outcome::Relinked);
        assert_eq!(std::fs::read_to_string(share.join("got.zip")).unwrap(), "z");
        assert!(std::fs::symlink_metadata(home.join("Downloads"))
            .unwrap()
            .file_type()
            .is_symlink());
        // A link elsewhere is replaced.
        std::fs::remove_file(home.join("Downloads")).unwrap();
        std::os::unix::fs::symlink(t.path(), home.join("Downloads")).unwrap();
        assert_eq!(ensure_link(&home, &share).unwrap(), Outcome::Relinked);
        assert_eq!(std::fs::read_link(home.join("Downloads")).unwrap(), share);
        assert_eq!(
            share_path(),
            PathBuf::from("/Volumes/My Shared Files/downloads")
        );
    }
}
