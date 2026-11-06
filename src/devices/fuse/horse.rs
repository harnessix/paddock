//! Dynamic inode mapping for the horse's restricted tree.
use crate::devices::{ledger, DeviceCtx};
use crate::horse::device::{self, Access, Entry};
use std::sync::Mutex;

pub(super) const FIRST: u64 = 1 << 40;
pub(super) struct HorseFs {
    setup: Access,
    ctx: DeviceCtx,
    paths: Mutex<Vec<String>>,
    /// Open handles of `self/dna.toml` with writes that are not loaded yet.
    dirty: Mutex<std::collections::HashSet<u64>>,
}
impl HorseFs {
    pub(super) fn new(setup: Access, ctx: DeviceCtx) -> Self {
        Self {
            setup,
            ctx,
            paths: Mutex::new(vec![String::new()]),
            dirty: Mutex::default(),
        }
    }
    pub(super) fn tree(&self) -> anyhow::Result<device::Tree> {
        self.setup.tree()
    }
    pub(super) fn path(&self, ino: u64) -> Option<String> {
        self.paths
            .lock()
            .unwrap()
            .get(usize::try_from(ino.checked_sub(FIRST)?).ok()?)
            .cloned()
    }
    pub(super) fn ino(&self, path: &str) -> u64 {
        let mut paths = self.paths.lock().unwrap();
        let idx = paths.iter().position(|s| s == path).unwrap_or_else(|| {
            paths.push(path.into());
            paths.len() - 1
        });
        FIRST + idx as u64
    }
    pub(super) fn entry(&self, path: &str) -> Option<Entry> {
        if path.is_empty() {
            return Some(Entry::Dir);
        }
        self.tree().ok()?.paths.remove(path)
    }
    pub(super) fn write(&self, name: &str, data: &[u8]) -> anyhow::Result<()> {
        device::write_memory(&self.setup.store, &self.setup.horse, name, data)
    }
    /// Load a whole `self/dna.toml`: invalid DNA is EINVAL and stores
    /// nothing.
    pub(super) fn load_dna(&self, data: &[u8]) -> Result<(), Errno> {
        let text = std::str::from_utf8(data).map_err(|_| Errno::EINVAL)?;
        crate::horse::dna::parse(text).map_err(|_| Errno::EINVAL)?;
        device::write_dna(&self.setup.store, &self.setup.horse, data).map_err(|e| {
            log::warn!("horse DNA: {e:#}");
            Errno::EIO
        })?;
        Ok(())
    }
    pub(super) fn set_dirty(&self, fh: u64, dirty: bool) -> bool {
        let mut d = self.dirty.lock().unwrap();
        if dirty {
            d.insert(fh)
        } else {
            d.remove(&fh)
        }
    }
    pub(super) fn audit(&self, path: &str) {
        let project = self.ctx.project.to_string_lossy();
        let payer = ledger::Payer {
            sandbox: &self.ctx.sandbox,
            root: ledger::root_of(&self.ctx.sandbox, &self.ctx.started_by),
            project: &project,
            device: "horse",
        };
        if let Err(e) = ledger::Ledger::open(&ledger::path(&self.ctx.env))
            .and_then(|l| l.record_read(&payer, path))
        {
            log::warn!("horse ledger: {e:#}");
        }
    }
}

use super::*;

impl DeviceFs {
    /// Load the writes of a `self/dna.toml` handle, once.
    fn dna_close(&self, fh: u64) -> Result<(), Errno> {
        let Some(horse) = &self.inner.horse else {
            return Ok(());
        };
        if !horse.set_dirty(fh, false) {
            return Ok(());
        }
        let bytes = match self.inner.handles.lock().unwrap().get(&fh) {
            Some(Handle {
                kind: Kind::Horse(_, bytes, true),
                ..
            }) => bytes.clone(),
            _ => return Ok(()),
        };
        horse.load_dna(&bytes)
    }
}

impl Filesystem for DeviceFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        if u64::from(parent) >= horse::FIRST {
            let Some(horse) = &self.inner.horse else {
                return reply.error(Errno::ENOENT);
            };
            let Some(base) = horse.path(parent.into()) else {
                return reply.error(Errno::ENOENT);
            };
            let Some(name) = name.to_str() else {
                return reply.error(Errno::ENOENT);
            };
            let path = if base.is_empty() {
                name.to_string()
            } else {
                format!("{base}/{name}")
            };
            return match self.attr(horse.ino(&path)) {
                Some(a) => reply.entry(&TTL, &a, fuser::Generation(0)),
                None => reply.error(Errno::ENOENT),
            };
        }
        let ino = match u64::from(parent) {
            ROOT => None,
            SECRET_DIR_INO => name.to_str().and_then(|n| self.secret_ino(n)),
            _ => return reply.error(Errno::ENOENT),
        };
        let ino = if ino.is_some() {
            ino
        } else if u64::from(parent) == SECRET_DIR_INO {
            None
        } else if name == "horse" && self.inner.horse.is_some() {
            Some(horse::FIRST)
        } else if name == CATALOG {
            Some(CATALOG_INO)
        } else if name == SECRET_DIR && !self.inner.secrets.names().is_empty() {
            Some(SECRET_DIR_INO)
        } else {
            self.inner
                .devices
                .iter()
                .position(|d| OsStr::new(d.name()) == name)
                .map(|i| FIRST_DEVICE_INO + i as u64)
        };
        match ino.and_then(|i| self.attr(i)) {
            Some(a) => reply.entry(&TTL, &a, fuser::Generation(0)),
            None => reply.error(Errno::ENOENT),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.attr(ino.into()) {
            Some(a) => reply.attr(&TTL, &a),
            None => reply.error(Errno::ENOENT),
        }
    }

    /// Only the truncation that `open(O_TRUNC)` and shell redirections do,
    /// on a device file. Nothing else changes.
    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let ino = u64::from(ino);
        if ino >= horse::FIRST {
            let Some(horse) = &self.inner.horse else {
                return reply.error(Errno::ENOENT);
            };
            let Some(path) = horse.path(ino) else {
                return reply.error(Errno::ENOENT);
            };
            if !device::writable(&path) {
                return reply.error(Errno::EROFS);
            }
            if mode.is_some() || uid.is_some() || gid.is_some() {
                return reply.error(Errno::EACCES);
            }
            // A write of dna.toml replaces the whole document at close.
            if let (Some(size), false) = (size, path == device::DNA_PATH) {
                if size > 1024 * 1024 {
                    return reply.error(Errno::EFBIG);
                }
                let Some(crate::horse::device::Entry::File(mut bytes)) = horse.entry(&path) else {
                    return reply.error(Errno::ENOENT);
                };
                bytes.resize(size as usize, 0);
                if horse
                    .write(path.trim_start_matches("self/memory/"), &bytes)
                    .is_err()
                {
                    return reply.error(Errno::EIO);
                }
            }
            return match self.attr(ino) {
                Some(a) => reply.attr(&TTL, &a),
                None => reply.error(Errno::ENOENT),
            };
        }
        if !(FIRST_DEVICE_INO..FIRST_SECRET_INO).contains(&ino)
            || mode.is_some()
            || uid.is_some()
            || gid.is_some()
        {
            return reply.error(Errno::EACCES);
        }
        if size.is_some_and(|s| s != 0) {
            return reply.error(Errno::EACCES);
        }
        match self.attr(ino) {
            Some(a) => reply.attr(&TTL, &a),
            None => reply.error(Errno::ENOENT),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let ino = u64::from(ino);
        let kind = match ino {
            n if n >= horse::FIRST => {
                let Some(horse) = &self.inner.horse else {
                    return reply.error(Errno::ENOENT);
                };
                let Some(path) = horse.path(n) else {
                    return reply.error(Errno::ENOENT);
                };
                match horse.entry(&path) {
                    Some(crate::horse::device::Entry::Dir) => return reply.error(Errno::EISDIR),
                    Some(crate::horse::device::Entry::File(mut bytes)) => {
                        let writable =
                            device::writable(&path) && flags.acc_mode() != OpenAccMode::O_RDONLY;
                        if flags.acc_mode() != OpenAccMode::O_RDONLY && !writable {
                            return reply.error(Errno::EROFS);
                        }
                        if writable && path == device::DNA_PATH {
                            // A writer gives the whole DNA; it loads at close.
                            bytes.clear();
                        } else if writable && flags.0 & libc::O_TRUNC != 0 {
                            bytes.clear();
                            if horse
                                .write(path.trim_start_matches("self/memory/"), &bytes)
                                .is_err()
                            {
                                return reply.error(Errno::EIO);
                            }
                        }
                        if !writable {
                            horse.audit(&path);
                        }
                        Kind::Horse(path, bytes, writable)
                    }
                    None => return reply.error(Errno::ENOENT),
                }
            }
            CATALOG_INO => {
                if flags.acc_mode() != OpenAccMode::O_RDONLY {
                    return reply.error(Errno::EACCES);
                }
                Kind::Catalog
            }
            n if n >= FIRST_SECRET_INO => {
                if flags.acc_mode() != OpenAccMode::O_RDONLY {
                    return reply.error(Errno::EACCES);
                }
                let Some(name) = self.secret_name(n) else {
                    return reply.error(Errno::ENOENT);
                };
                match self.inner.secrets.open(&name) {
                    Ok(v) => {
                        log::info!("sandbox {}: secret file {name} opened", self.inner.sandbox);
                        Kind::Secret(name, v)
                    }
                    Err(FileError::Revoked) => return reply.error(Errno::EACCES),
                    Err(FileError::Missing) => return reply.error(Errno::ENOENT),
                }
            }
            SECRET_DIR_INO => return reply.error(Errno::EISDIR),
            n if self.attr(n).is_some() && n >= FIRST_DEVICE_INO => {
                Kind::Device((n - FIRST_DEVICE_INO) as usize)
            }
            ROOT => return reply.error(Errno::EISDIR),
            _ => return reply.error(Errno::ENOENT),
        };
        let mut h = self.inner.handles.lock().unwrap();
        if h.len() >= MAX_OPEN {
            return reply.error(Errno::EBUSY);
        }
        let fh = self.inner.next_fh.fetch_add(1, Ordering::SeqCst);
        h.insert(
            fh,
            Handle {
                kind,
                request: Vec::new(),
                phase: Phase::Collect,
                waiting: Vec::new(),
                cursor: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        // Direct I/O: every read and write reaches paddock, whatever size
        // the file shows.
        reply.opened(
            FileHandle(fh),
            FopenFlags::FOPEN_DIRECT_IO | FopenFlags::FOPEN_NONSEEKABLE,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let mut h = self.inner.handles.lock().unwrap();
        let Some(handle) = h.get_mut(&u64::from(fh)) else {
            return reply.error(Errno::EBADF);
        };
        if let Kind::Horse(path, bytes, writable) = &mut handle.kind {
            if !*writable {
                return reply.error(Errno::EROFS);
            }
            let offset = _offset as usize;
            let Some(end) = offset.checked_add(data.len()).filter(|n| *n <= 1024 * 1024) else {
                return reply.error(Errno::EFBIG);
            };
            bytes.resize(bytes.len().max(end), 0);
            bytes[offset..end].copy_from_slice(data);
            let Some(horse) = &self.inner.horse else {
                return reply.error(Errno::EIO);
            };
            if path == device::DNA_PATH {
                horse.set_dirty(u64::from(fh), true);
                return reply.written(data.len() as u32);
            }
            if horse
                .write(path.trim_start_matches("self/memory/"), bytes)
                .is_err()
            {
                return reply.error(Errno::EIO);
            }
            return reply.written(data.len() as u32);
        }
        let Kind::Device(i) = handle.kind else {
            return reply.error(Errno::EACCES);
        };
        if !matches!(handle.phase, Phase::Collect) {
            // One open is one request: the response is being read.
            return reply.error(Errno::EBUSY);
        }
        let max = self.inner.devices[i].info().limits.max_request;
        if handle.request.len() + data.len() > max {
            return reply.error(Errno::EFBIG);
        }
        handle.request.extend_from_slice(data);
        reply.written(data.len() as u32);
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let fh = u64::from(fh);
        let mut h = self.inner.handles.lock().unwrap();
        let Some(handle) = h.get_mut(&fh) else {
            return reply.error(Errno::EBADF);
        };
        let i = match &handle.kind {
            Kind::Catalog => {
                drop(h);
                return reply_slice(reply, &self.catalog(), offset, size);
            }
            Kind::Secret(name, value) => {
                // A revoke ends the file at once, also while it is open.
                if self.inner.secrets.check(name).is_err() {
                    return reply.error(Errno::EACCES);
                }
                let value = value.clone();
                drop(h);
                return reply_slice(reply, value.as_bytes(), offset, size);
            }
            Kind::Horse(_, bytes, _) => {
                let bytes = bytes.clone();
                drop(h);
                return reply_slice(reply, &bytes, offset, size);
            }
            Kind::Device(i) => *i,
        };
        match &handle.phase {
            Phase::Done(Ok(data)) => {
                let data = data.clone();
                let r = take(&mut handle.cursor, &data, size);
                drop(h);
                reply.data(&data[r])
            }
            Phase::Done(Err(e)) => reply.error(*e),
            Phase::Running => {
                if handle.waiting.len() >= 4 {
                    return reply.error(Errno::EBUSY);
                }
                handle.waiting.push((reply, size, req.pid()));
            }
            Phase::Collect => {
                if self.inner.running.load(Ordering::SeqCst) >= MAX_RUNNING as u64 {
                    return reply.error(Errno::EBUSY);
                }
                handle.phase = Phase::Running;
                handle.waiting.push((reply, size, req.pid()));
                let request = std::mem::take(&mut handle.request);
                let cancel = handle.cancel.clone();
                let dev = self.inner.devices[i].clone();
                drop(h);
                self.dispatch(fh, dev, request, cancel);
            }
        }
    }

    /// A close: a written `self/dna.toml` hot-loads now, and an invalid
    /// one fails the close with EINVAL.
    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        match self.dna_close(u64::from(fh)) {
            Err(e) => reply.error(e),
            Ok(()) => reply.ok(),
        }
    }

    /// The last close. A request that still runs is cancelled.
    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if let Err(e) = self.dna_close(u64::from(fh)) {
            log::warn!("horse DNA: a write at release failed: {e:?}");
        }
        let removed = self.inner.handles.lock().unwrap().remove(&u64::from(fh));
        if let Some(handle) = removed {
            handle.cancel.store(true, Ordering::SeqCst);
            for (r, _, _) in handle.waiting {
                r.error(Errno::ECANCELED);
            }
        }
        reply.ok();
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        if u64::from(ino) >= horse::FIRST {
            return match self.attr(ino.into()) {
                Some(a) if a.kind == FileType::Directory => {
                    reply.opened(FileHandle(0), FopenFlags::empty())
                }
                _ => reply.error(Errno::ENOTDIR),
            };
        }
        if !matches!(u64::from(ino), ROOT | SECRET_DIR_INO) {
            return reply.error(Errno::ENOTDIR);
        }
        reply.opened(FileHandle(0), FopenFlags::empty());
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        if u64::from(ino) >= horse::FIRST {
            let Some(horse) = &self.inner.horse else {
                return reply.error(Errno::ENOENT);
            };
            let Some(path) = horse.path(ino.into()) else {
                return reply.error(Errno::ENOENT);
            };
            if horse.entry(&path) != Some(crate::horse::device::Entry::Dir) {
                return reply.error(Errno::ENOTDIR);
            }
            let Ok(tree) = horse.tree() else {
                return reply.error(Errno::EIO);
            };
            let prefix = if path.is_empty() {
                String::new()
            } else {
                format!("{path}/")
            };
            let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
            let mut entries = vec![
                (u64::from(ino), FileType::Directory, ".".to_string()),
                (
                    if path.is_empty() {
                        ROOT
                    } else {
                        horse.ino(parent)
                    },
                    FileType::Directory,
                    "..".into(),
                ),
            ];
            for (p, entry) in tree.paths {
                if let Some(name) = p.strip_prefix(&prefix) {
                    if !name.is_empty() && !name.contains('/') {
                        entries.push((
                            horse.ino(&p),
                            match entry {
                                crate::horse::device::Entry::Dir => FileType::Directory,
                                _ => FileType::RegularFile,
                            },
                            name.into(),
                        ));
                    }
                }
            }
            for (n, (i, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
                if reply.add(INodeNo(i), (n + 1) as u64, kind, name) {
                    break;
                }
            }
            return reply.ok();
        }
        let names = self.inner.secrets.names();
        let mut entries: Vec<(u64, FileType, String)> = match u64::from(ino) {
            ROOT => {
                let mut e = vec![
                    (ROOT, FileType::Directory, ".".into()),
                    (ROOT, FileType::Directory, "..".into()),
                    (CATALOG_INO, FileType::RegularFile, CATALOG.into()),
                ];
                if self.inner.horse.is_some() {
                    e.push((horse::FIRST, FileType::Directory, "horse".into()));
                }
                if !names.is_empty() {
                    e.push((SECRET_DIR_INO, FileType::Directory, SECRET_DIR.into()));
                }
                e
            }
            SECRET_DIR_INO => vec![
                (SECRET_DIR_INO, FileType::Directory, ".".into()),
                (ROOT, FileType::Directory, "..".into()),
            ],
            _ => return reply.error(Errno::ENOTDIR),
        };
        if u64::from(ino) == SECRET_DIR_INO {
            for n in names {
                if let Some(i) = self.secret_ino(&n) {
                    entries.push((i, FileType::RegularFile, n));
                }
            }
        } else {
            for (i, d) in self.inner.devices.iter().enumerate() {
                entries.push((
                    FIRST_DEVICE_INO + i as u64,
                    FileType::RegularFile,
                    d.name().to_string(),
                ));
            }
        }
        for (n, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(INodeNo(ino), (n + 1) as u64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        if u64::from(_parent) >= horse::FIRST {
            let Some(horse) = &self.inner.horse else {
                return reply.error(Errno::ENOENT);
            };
            if horse.path(_parent.into()).as_deref() != Some("self/memory") {
                return reply.error(Errno::EROFS);
            }
            let Some(name) = _name.to_str() else {
                return reply.error(Errno::EINVAL);
            };
            if horse.write(name, &[]).is_err() {
                return reply.error(Errno::EINVAL);
            }
            let path = format!("self/memory/{name}");
            let ino = horse.ino(&path);
            let Some(attr) = self.attr(ino) else {
                return reply.error(Errno::EIO);
            };
            let fh = self.inner.next_fh.fetch_add(1, Ordering::SeqCst);
            self.inner.handles.lock().unwrap().insert(
                fh,
                Handle {
                    kind: Kind::Horse(path, Vec::new(), true),
                    request: Vec::new(),
                    phase: Phase::Collect,
                    waiting: Vec::new(),
                    cursor: 0,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            );
            return reply.created(
                &TTL,
                &attr,
                fuser::Generation(0),
                FileHandle(fh),
                FopenFlags::FOPEN_DIRECT_IO,
            );
        }
        reply.error(Errno::EACCES);
    }

    fn mknod(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        reply.error(if u64::from(_parent) >= horse::FIRST {
            Errno::EROFS
        } else {
            Errno::EACCES
        });
    }

    fn mkdir(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        reply.error(if u64::from(_parent) >= horse::FIRST {
            Errno::EROFS
        } else {
            Errno::EACCES
        });
    }

    fn unlink(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(if u64::from(_parent) >= horse::FIRST {
            Errno::EROFS
        } else {
            Errno::EACCES
        });
    }

    fn rmdir(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(if u64::from(_parent) >= horse::FIRST {
            Errno::EROFS
        } else {
            Errno::EACCES
        });
    }

    fn rename(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _newparent: INodeNo,
        _newname: &OsStr,
        _flags: fuser::RenameFlags,
        reply: ReplyEmpty,
    ) {
        reply.error(if u64::from(_parent) >= horse::FIRST {
            Errno::EROFS
        } else {
            Errno::EACCES
        });
    }

    fn setxattr(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _name: &OsStr,
        _value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        reply.error(Errno::EACCES);
    }
}
