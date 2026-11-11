//! A fake disk tool for the tests of the offline seed and trust and of
//! the setup driver. `attach` makes a folder with the files of a model of
//! the Data volume, `detach` writes the folder back into the image as APFS
//! (`apfs_fixture`), the new paths with the owner of a mount without
//! owners, 99:99. So the owner patch runs on a real APFS layout, and the
//! TCC database in the folder survives a detach.

use super::apfs;
use super::apfs_fixture::{self as fx, Entry, Fixture};
use super::diskimage::{Attached, DiskTool, Fsck};
use anyhow::Result;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A disk tool over a model of the Data volume: `attach` makes a folder
/// with the files of the model, `detach` writes the folder back into the
/// image as APFS (`apfs_fixture`), the new paths with the owner of a
/// mount without owners, 99:99. So the owner patch of the seed runs on a
/// real APFS layout.
pub struct FakeDisk {
    pub model: Vec<Entry>,
    pub calls: Vec<String>,
    pub fail_clone: bool,
    pub fail_attach: bool,
    pub fail_detach: bool,
    pub fsck: Result<Fsck, String>,
    /// The TCC database of the guest: copied into each mount and back.
    pub tcc: Option<PathBuf>,
}

impl FakeDisk {
    pub fn new(image: &Path) -> FakeDisk {
        let model = vec![
            dir("Library", 0, 0),
            dir("Library/LaunchDaemons", 0, 0),
            dir("Library/LaunchAgents", 0, 0),
            dir("Library/Application Support", 0, 0),
            dir("Library/Application Support/com.apple.TCC", 0, 0),
            dir("Users", 0, 0),
            dir("private", 0, 0),
            dir("private/var", 0, 0),
            dir("private/var/db", 0, 0),
        ];
        let d = FakeDisk {
            model,
            calls: Vec::new(),
            fail_clone: false,
            fail_attach: false,
            fail_detach: false,
            fsck: Ok(Fsck::Clean),
            tcc: None,
        };
        d.store(image);
        d
    }

    pub fn store(&self, image: &Path) {
        let mut f = Fixture::new();
        f.entries = self.model.clone();
        f.gpt = true;
        fx::write(&f, image);
    }

    /// The owner of a path in the image, read through `apfs.rs`.
    pub fn owner(image: &Path, path: &str) -> (u32, u32) {
        apfs::owners(image, apfs::ROLE_DATA, &[path]).unwrap()[0]
    }
}

fn dir(p: &str, uid: u32, gid: u32) -> Entry {
    Entry {
        path: p.into(),
        dir: true,
        uid,
        gid,
    }
}

fn walk(root: &Path, rel: &Path, out: &mut Vec<(String, bool)>) {
    let mut names: Vec<_> = std::fs::read_dir(root.join(rel))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    names.sort();
    for n in names {
        let r = rel.join(&n);
        let is_dir = root.join(&r).is_dir();
        out.push((r.to_string_lossy().into_owned(), is_dir));
        if is_dir {
            walk(root, &r, out);
        }
    }
}

impl DiskTool for FakeDisk {
    fn attach(&mut self, image: &Path, root: &Path) -> Result<Attached> {
        self.calls.push("attach".into());
        if self.fail_attach {
            anyhow::bail!("hdiutil: attach failed - not recognized"); // not user text
        }
        assert!(apfs::owners(image, apfs::ROLE_DATA, &["Library"]).is_ok());
        // The owners on the disk are the truth: a patch since the last
        // detach changed them.
        for e in &mut self.model {
            if let Ok(v) = apfs::owners(image, apfs::ROLE_DATA, &[e.path.as_str()]) {
                (e.uid, e.gid) = v[0];
            }
        }
        let data = root.join("Data");
        // A fresh mount: what a failed detach left in the folder goes.
        if std::fs::symlink_metadata(&data).is_ok() {
            std::fs::remove_dir_all(&data).unwrap();
        }
        std::fs::create_dir_all(&data).unwrap();
        for e in &self.model {
            let p = data.join(&e.path);
            if e.dir {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "").unwrap();
            }
        }
        if let Some(db) = &self.tcc {
            let to = data.join(crate::macvm::tcc::DB);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(db, &to).unwrap();
        }
        Ok(Attached {
            disk: "/dev/disk9".into(),
            data,
        })
    }

    fn detach(&mut self, a: &Attached) -> Result<()> {
        self.calls.push("detach".into());
        if self.fail_detach {
            anyhow::bail!("hdiutil: detach failed - Resource busy"); // not user text
        }
        if let Some(db) = &self.tcc {
            let from = a.data.join(crate::macvm::tcc::DB);
            if from.is_file() {
                std::fs::copy(&from, db).unwrap();
            }
        }
        let mut now = Vec::new();
        walk(&a.data, Path::new(""), &mut now);
        let old = std::mem::take(&mut self.model);
        for (p, is_dir) in now {
            let kept = old.iter().find(|e| e.path == p && e.dir == is_dir);
            self.model.push(match kept {
                Some(e) => e.clone(),
                None => Entry {
                    path: p,
                    dir: is_dir,
                    uid: apfs::NOOWNERS_ID,
                    gid: apfs::NOOWNERS_ID,
                },
            });
        }
        std::fs::remove_dir_all(&a.data).unwrap();
        Ok(())
    }

    fn fsck(&mut self, image: &Path) -> Result<Fsck> {
        self.calls.push("fsck".into());
        apfs::owners(image, apfs::ROLE_DATA, &["Library"])?;
        self.fsck.clone().map_err(anyhow::Error::msg)
    }

    fn clone_file(&mut self, from: &Path, to: &Path) -> Result<()> {
        self.calls.push("clone".into());
        if self.fail_clone {
            anyhow::bail!("clonefile: Operation not supported"); // not user text
        }
        crate::macvm::diskimage::clone_file(from, to)
    }
}

/// The fake disk, writing its model into the image at each detach.
pub struct Store<'a> {
    pub d: &'a mut FakeDisk,
    pub image: PathBuf,
}

impl DiskTool for Store<'_> {
    fn attach(&mut self, image: &Path, root: &Path) -> Result<Attached> {
        self.d.attach(image, root)
    }
    fn detach(&mut self, a: &Attached) -> Result<()> {
        self.d.detach(a)?;
        self.d.store(&self.image);
        Ok(())
    }
    fn fsck(&mut self, image: &Path) -> Result<Fsck> {
        self.d.fsck(image)
    }
    fn clone_file(&mut self, from: &Path, to: &Path) -> Result<()> {
        self.d.clone_file(from, to)
    }
}

/// The fake disk shared between the calls of a backend: each
/// `disk_tool()` gives one of these.
pub struct Shared {
    pub d: Rc<RefCell<FakeDisk>>,
    pub image: PathBuf,
}

impl DiskTool for Shared {
    fn attach(&mut self, image: &Path, root: &Path) -> Result<Attached> {
        self.d.borrow_mut().attach(image, root)
    }
    fn detach(&mut self, a: &Attached) -> Result<()> {
        let mut d = self.d.borrow_mut();
        d.detach(a)?;
        d.store(&self.image);
        Ok(())
    }
    fn fsck(&mut self, image: &Path) -> Result<Fsck> {
        self.d.borrow_mut().fsck(image)
    }
    fn clone_file(&mut self, from: &Path, to: &Path) -> Result<()> {
        self.d.borrow_mut().clone_file(from, to)
    }
}
