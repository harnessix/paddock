//! The pointer: `.paddock/pointer.json` in the original folder.
//!
//! It holds the format, the store id and the creation time. It holds no
//! path, no key and no project data, so `cp`, a file manager or `zip`
//! carry only this file along. `.paddock` is a real folder: a symlink
//! would lead `zip` into the notes.
//!
//! The store records the device and the inode of the folder (store.rs).
//! With the pointer, that tells a moved folder from a copy of it:
//!
//! - same inode, same path: linked.
//! - same inode, new path: the user moved the folder. The store gets the
//!   new path.
//! - other inode, and the recorded folder is still there: this folder is a
//!   copy. It gets a new store when the user says so.
//! - other inode, and the recorded folder is gone: a restore from a backup
//!   or a move to another disk. The user confirms the link.

use super::store::{folder_id, Store};
use super::Config;
use crate::env::HostEnv;
use crate::safefs::{Kind, Root};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::Path;

pub const DIR: &str = ".paddock";
pub const FILE: &str = "pointer.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pointer {
    pub format: u32,
    pub store: String,
    pub created: u64,
}

/// The pointer of `project`. None when the folder has none. A `.paddock`
/// that is a symlink is never followed: it is no pointer.
pub fn read(project: &Path) -> Result<Option<Pointer>> {
    let Ok(dir) = Root::open(project).and_then(|r| r.dir(Path::new(DIR), false)) else {
        return Ok(None);
    };
    let file = project.join(DIR).join(FILE);
    let Some(bytes) = dir.read(OsStr::new(FILE), 64 * 1024).ok().flatten() else {
        return Ok(None);
    };
    let p: Pointer = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "{} is invalid. Remove it, and paddock makes a new store for this folder",
            file.display()
        )
    })?;
    if p.format > super::FORMAT {
        bail!(
            "{} has format {}, which a newer paddock wrote. Update paddock",
            file.display(),
            p.format
        );
    }
    if !super::store::valid_id(&p.store) {
        bail!(
            "{} names no store id. Remove it, and paddock makes a new store for this folder",
            file.display()
        );
    }
    Ok(Some(p))
}

/// Write the pointer of `project`. A `.paddock` symlink to the loft notes
/// becomes the pointer folder. True: it replaced that symlink.
pub fn write(project: &Path, p: &Pointer, loft_notes: &Path) -> Result<bool> {
    let top = Root::open(project)?.dir(Path::new(""), false)?;
    let name = OsStr::new(DIR);
    let mut replaced = false;
    match top.kind(name)? {
        None | Some(Kind::Dir) => {}
        Some(Kind::Symlink) => {
            if top.read_link(name)? != loft_notes {
                bail!(
                    "{} is a symlink to another place. Remove it, then run again",
                    project.join(DIR).display()
                );
            }
            top.remove(name)?;
            replaced = true;
        }
        Some(_) => bail!(
            "{} is not a folder. Remove it, then run again",
            project.join(DIR).display()
        ),
    }
    top.make_dir(name, 0o755)?;
    let mut text = serde_json::to_vec_pretty(p)?;
    text.push(b'\n');
    top.subdir(name, false)?
        .write(OsStr::new(FILE), &text, 0o644)?;
    Ok(replaced)
}

/// What the pointer and the store say about a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Linked,
    Moved,
    Copy,
    Restored,
    NoStore,
    DiskAway,
    NoPointer,
    /// A run that works in the folder itself.
    Direct,
}

impl State {
    /// The word for lists and JSON.
    pub fn word(self) -> &'static str {
        match self {
            State::Linked => "linked",
            State::Moved => "moved",
            State::Copy => "copy",
            State::Restored => "restored",
            State::NoStore => "no store",
            State::DiskAway => "disk away",
            State::NoPointer => "no pointer",
            State::Direct => "direct",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            State::Linked => "The pointer and the store agree.",
            State::Moved => "The folder has a new path. paddock updated the store.",
            State::Copy => "This folder is a copy of another project folder.",
            State::Restored => "The folder came back from a backup or moved to another disk.",
            State::NoStore => "The store of this pointer is not on this machine.",
            State::DiskAway => "The store is on a disk that is not connected.",
            State::NoPointer => "The folder has no .paddock/pointer.json.",
            State::Direct => "The run works in the folder. There is no copy.",
        }
    }

    /// The command that the state asks for.
    pub fn action(self) -> Option<&'static str> {
        match self {
            State::Copy | State::NoStore => Some("paddock pointer new"),
            State::Restored => Some("paddock pointer link"),
            State::NoPointer => Some("paddock pointer create"),
            _ => None,
        }
    }

    /// The pointer leads to a store that paddock can use.
    pub fn ok(self) -> bool {
        matches!(self, State::Linked | State::Moved)
    }
}

pub struct Status {
    pub state: State,
    pub pointer: Option<Pointer>,
    /// The store of the pointer id, also when it belongs to another folder.
    pub store: Option<Store>,
}

/// The state of `project`. With `update`, a moved folder gets its new path
/// into the store.
pub fn status(env: &HostEnv, cfg: &Config, project: &Path, update: bool) -> Result<Status> {
    let Some(p) = read(project)? else {
        return Ok(Status {
            state: State::NoPointer,
            pointer: None,
            store: None,
        });
    };
    let Some(mut store) = Store::find(&cfg.roots(env), &p.store)? else {
        let away = cfg.store_dir.as_ref().is_some_and(|d| !d.is_dir());
        return Ok(Status {
            state: if away {
                State::DiskAway
            } else {
                State::NoStore
            },
            pointer: Some(p),
            store: None,
        });
    };
    let here = folder_id(project);
    let recorded = (store.meta.dev, store.meta.ino);
    let state = if here == Some(recorded) {
        if store.meta.path == project {
            State::Linked
        } else {
            if update {
                store.meta.path = project.to_path_buf();
                store.save()?;
            }
            State::Moved
        }
    } else if store.meta.path != project && folder_id(&store.meta.path) == Some(recorded) {
        State::Copy
    } else {
        State::Restored
    };
    Ok(Status {
        state,
        pointer: Some(p),
        store: Some(store),
    })
}

/// A new, empty store for `project`, and its pointer. An old store stays
/// where it is, with the folder that it belongs to.
pub fn new_store(env: &HostEnv, cfg: &Config, project: &Path) -> Result<(Store, bool)> {
    let root = cfg.root(env);
    crate::privfs::dir(&root)
        .with_context(|| format!("cannot make the store folder {}", root.display()))?;
    let store = Store::create(&root, project)?;
    let p = Pointer {
        format: super::FORMAT,
        store: store.meta.id.clone(),
        created: store.meta.created,
    };
    let notes = crate::loft::Loft::of(env, project).notes();
    match write(project, &p, &notes) {
        Ok(replaced) => Ok((store, replaced)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&store.dir);
            Err(e)
        }
    }
}

/// Give the store to `project`: its path, device and inode.
pub fn link(store: &mut Store, project: &Path) -> Result<()> {
    let (dev, ino) =
        folder_id(project).with_context(|| format!("{} is not a folder", project.display()))?;
    store.meta.path = project.to_path_buf();
    store.meta.dev = dev;
    store.meta.ino = ino;
    store.save()
}

/// The store of `project` for a run, made or repaired as needed. `ask`
/// gets a state that needs a decision and the question. Returns the store
/// and the notices for the user.
pub fn ensure(
    env: &HostEnv,
    cfg: &Config,
    project: &Path,
    ask: &mut dyn FnMut(State, &str) -> bool,
) -> Result<(Store, Vec<String>)> {
    // Two runs that start together must not both make a store for one
    // folder: the second one finds the pointer of the first.
    let root = cfg.root(env);
    let _lock = match (cfg.store_dir.is_none() || root.is_dir()).then(|| crate::privfs::dir(&root))
    {
        Some(made) => {
            made.with_context(|| format!("cannot make the store folder {}", root.display()))?;
            let f = crate::privfs::append(&root.join("lock"))?;
            use std::os::fd::AsRawFd;
            // SAFETY: flock on an open file. The lock goes with the file.
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
            Some(f)
        }
        // A configured store folder that is away: `status` says so.
        None => None,
    };
    let st = status(env, cfg, project, true)?;
    let mut notes = Vec::new();
    let fresh = |notes: &mut Vec<String>| -> Result<Store> {
        let (store, replaced) = new_store(env, cfg, project)?;
        if replaced {
            notes.push(format!(
                "{DIR} of the project was a link to the notes. It is a folder with {FILE} now, and the notes stay in paddock's store. The agents see them at {DIR}/ as before"
            ));
        }
        Ok(store)
    };
    let refuse = |state: State| -> anyhow::Error {
        anyhow::anyhow!(
            "{} Run `{}` in the folder, or start with --direct to work in the folder itself",
            state.text(),
            state.action().unwrap_or("paddock pointer status")
        )
    };
    match (st.state, st.store) {
        (State::Linked, Some(s)) => Ok((s, notes)),
        (State::Moved, Some(s)) => {
            notes.push(format!(
                "the project moved to {}. Its copies and backups came along",
                project.display()
            ));
            Ok((s, notes))
        }
        (State::NoPointer, _) => {
            let s = fresh(&mut notes)?;
            notes.push(format!(
                "paddock made {DIR}/{FILE} in the project. It names the store of the copies and backups, and holds no project data"
            ));
            Ok((s, notes))
        }
        (State::Copy, Some(old)) => {
            let q = format!(
                "This folder is a copy of {}. Make a new, empty store for this folder?",
                old.meta.path.display()
            );
            if !ask(State::Copy, &q) {
                return Err(refuse(State::Copy));
            }
            let s = fresh(&mut notes)?;
            Ok((s, notes))
        }
        (State::Restored, Some(mut old)) => {
            let q = format!(
                "The store of this folder belonged to {}, which is gone. Link the store with its backups to this folder?",
                old.meta.path.display()
            );
            if !ask(State::Restored, &q) {
                return Err(refuse(State::Restored));
            }
            link(&mut old, project)?;
            Ok((old, notes))
        }
        (State::NoStore, _) => {
            if !ask(
                State::NoStore,
                "The store of this folder is not on this machine. Make a new, empty store?",
            ) {
                return Err(refuse(State::NoStore));
            }
            let s = fresh(&mut notes)?;
            Ok((s, notes))
        }
        (State::DiskAway, _) => bail!(
            "the store of this project is in {}, which is not there. Connect the disk, or start with --direct to work in the folder itself",
            cfg.root(env).display()
        ),
        (state, _) => Err(refuse(state)),
    }
}
