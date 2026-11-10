//! The FUSE file system that serves `/dev/paddock` on Linux.

use super::folders;
use super::{catalog_with, Call, Device, DeviceCtx, CATALOG, SECRET_DIR};
use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, INodeNo, LockOwner, OpenAccMode,
    OpenFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyWrite, Request, WriteFlags,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

mod horse;

/// Open device files at the same time, per sandbox.
const MAX_OPEN: usize = 64;
/// Requests that run at the same time, per sandbox.
const MAX_RUNNING: usize = 16;

const ROOT: u64 = 1;
const CATALOG_INO: u64 = 2;
const SECRET_DIR_INO: u64 = 3;
const FIRST_DEVICE_INO: u64 = 16;
/// The inode of the first secret file. Devices stay below it.
const FIRST_SECRET_INO: u64 = 1 << 20;
const TTL: Duration = Duration::from_secs(1);

enum Phase {
    /// The client writes the request.
    Collect,
    /// The device works on it.
    Running,
    Done(Result<Arc<Vec<u8>>, Errno>),
}

enum Kind {
    Catalog,
    Device(usize),
    /// A secret file: its name and the value, held while the file is open.
    Secret(String, Arc<crate::credentials::secret::Secret>),
    /// A file of the horse tree: its path, its bytes and whether it is
    /// open for writing.
    Horse(String, Vec<u8>, bool),
}

struct Handle {
    kind: Kind,
    request: Vec<u8>,
    phase: Phase,
    /// Reads that wait for the response: the reply, its size and the
    /// thread that reads (a host thread id; FUSE translates it).
    waiting: Vec<(ReplyData, u32, u32)>,
    /// How much of the response the client has read. The kernel offset
    /// counts the request bytes too, so the response keeps its own.
    cursor: usize,
    cancel: Arc<AtomicBool>,
}

struct Inner {
    sandbox: String,
    uid: u32,
    gid: u32,
    devices: Vec<Arc<dyn Device>>,
    secrets: Arc<SecretFiles>,
    /// `/dev/paddock/horse`, in horse mode.
    horse: Option<horse::HorseFs>,
    /// The names of the secret files by inode, from `FIRST_SECRET_INO` on.
    /// A name keeps its inode after a revoke.
    secret_inos: Mutex<Vec<String>>,
    handles: Mutex<HashMap<u64, Handle>>,
    next_fh: AtomicU64,
    running: AtomicU64,
    started: SystemTime,
}

/// The file system of one sandbox.
#[derive(Clone)]
pub struct DeviceFs {
    inner: Arc<Inner>,
}

fn reply_slice(reply: ReplyData, data: &[u8], offset: u64, size: u32) {
    let start = (offset as usize).min(data.len());
    let end = start.saturating_add(size as usize).min(data.len());
    reply.data(&data[start..end]);
}

/// The next part of the response for a read of `size` bytes.
fn take(cursor: &mut usize, data: &[u8], size: u32) -> std::ops::Range<usize> {
    let start = (*cursor).min(data.len());
    let end = start.saturating_add(size as usize).min(data.len());
    *cursor = end;
    start..end
}

use crate::credentials::secret::{FileError, SecretFiles};

impl DeviceFs {
    pub fn new(sandbox: &str, devices: Vec<Arc<dyn Device>>) -> Self {
        Self::with_secrets(sandbox, devices, SecretFiles::new(None, None))
    }

    /// A file system that also serves the secret files of `secrets`.
    pub fn with_secrets(
        sandbox: &str,
        devices: Vec<Arc<dyn Device>>,
        secrets: Arc<SecretFiles>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sandbox: sandbox.to_string(),
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                devices,
                secrets,
                horse: None,
                secret_inos: Mutex::new(Vec::new()),
                handles: Mutex::new(HashMap::new()),
                next_fh: AtomicU64::new(1),
                running: AtomicU64::new(0),
                started: SystemTime::now(),
            }),
        }
    }

    /// The same file system with `/dev/paddock/horse`: the run's view of
    /// its horse, when `setup` is set. Before the mount.
    pub fn with_horse(
        mut self,
        setup: Option<crate::horse::device::Access>,
        ctx: DeviceCtx,
    ) -> Self {
        let inner = Arc::get_mut(&mut self.inner).expect("a file system that is not shared yet");
        inner.horse = setup.map(|set| horse::HorseFs::new(set, ctx));
        self
    }

    fn catalog(&self) -> Arc<Vec<u8>> {
        Arc::new(catalog_with(
            &self.inner.devices,
            &self.inner.secrets.names(),
        ))
    }

    fn names(&self) -> Vec<&str> {
        self.inner.devices.iter().map(|d| d.name()).collect()
    }

    /// The name of a secret file inode.
    fn secret_name(&self, ino: u64) -> Option<String> {
        let idx = usize::try_from(ino.checked_sub(FIRST_SECRET_INO)?).ok()?;
        self.inner.secret_inos.lock().unwrap().get(idx).cloned()
    }

    /// The inode of a secret file that the store holds now.
    fn secret_ino(&self, name: &str) -> Option<u64> {
        if !self.inner.secrets.contains(name) {
            return None;
        }
        let mut inos = self.inner.secret_inos.lock().unwrap();
        let idx = match inos.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                inos.push(name.to_string());
                inos.len() - 1
            }
        };
        Some(FIRST_SECRET_INO + idx as u64)
    }

    fn attr(&self, ino: u64) -> Option<FileAttr> {
        let i = &self.inner;
        let (kind, perm, size, nlink) = match ino {
            ROOT => (FileType::Directory, 0o555, 0, 2),
            CATALOG_INO => (FileType::RegularFile, 0o444, self.catalog().len() as u64, 1),
            SECRET_DIR_INO => (FileType::Directory, 0o500, 0, 2),
            n if folders::dir_name(&self.names(), n).is_some() => {
                (FileType::Directory, 0o500, 0, 2)
            }
            n if n >= horse::FIRST => {
                let horse = i.horse.as_ref()?;
                let path = horse.path(n)?;
                match horse.entry(&path)? {
                    crate::horse::device::Entry::Dir => (
                        FileType::Directory,
                        if path == "self/memory" { 0o700 } else { 0o555 },
                        0,
                        2,
                    ),
                    crate::horse::device::Entry::File(bytes) => (
                        FileType::RegularFile,
                        if crate::horse::device::writable(&path) {
                            0o600
                        } else {
                            0o444
                        },
                        bytes.len() as u64,
                        1,
                    ),
                }
            }
            n if n >= FIRST_SECRET_INO => {
                let name = self.secret_name(n)?;
                let v = i.secrets.check(&name).ok()?;
                (FileType::RegularFile, 0o400, v.len() as u64, 1)
            }
            n if n >= FIRST_DEVICE_INO && ((n - FIRST_DEVICE_INO) as usize) < i.devices.len() => {
                (FileType::RegularFile, 0o600, 0, 1)
            }
            _ => return None,
        };
        Some(FileAttr {
            ino: INodeNo(ino),
            size,
            blocks: size.div_ceil(512),
            atime: i.started,
            mtime: i.started,
            ctime: i.started,
            crtime: i.started,
            kind,
            perm,
            nlink,
            uid: i.uid,
            gid: i.gid,
            rdev: 0,
            flags: 0,
            blksize: 4096,
        })
    }

    /// Store the result and answer every read that waits for it.
    fn finish(inner: &Inner, fh: u64, result: Result<Arc<Vec<u8>>, Errno>) {
        let waiting = {
            let mut h = inner.handles.lock().unwrap();
            let Some(handle) = h.get_mut(&fh) else {
                return;
            };
            if !matches!(handle.phase, Phase::Running) {
                return;
            }
            handle.phase = Phase::Done(result.clone());
            let mut out = Vec::new();
            for (reply, size, _) in std::mem::take(&mut handle.waiting) {
                let part = match &result {
                    Ok(data) => Ok(take(&mut handle.cursor, data, size)),
                    Err(e) => Err(*e),
                };
                out.push((reply, part));
            }
            out
        };
        for (reply, part) in waiting {
            match (&result, part) {
                (Ok(data), Ok(r)) => reply.data(&data[r]),
                (_, Err(e)) => reply.error(e),
                (Err(e), _) => reply.error(*e),
            }
        }
    }

    /// True when the handle is closed, or every read that waits on it comes
    /// from a thread with a fatal signal pending.
    fn readers_gone(inner: &Inner, fh: u64) -> bool {
        let h = inner.handles.lock().unwrap();
        let Some(handle) = h.get(&fh) else {
            return true;
        };
        !handle.waiting.is_empty() && handle.waiting.iter().all(|w| fatal_pending(w.2))
    }

    /// Run the request of `fh` on its own thread, under the deadline.
    fn dispatch(&self, fh: u64, dev: Arc<dyn Device>, request: Vec<u8>, cancel: Arc<AtomicBool>) {
        let inner = self.inner.clone();
        let info = dev.info();
        let deadline = Instant::now() + Duration::from_secs(info.limits.deadline_seconds);
        inner.running.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let start = Instant::now();
            let call = Call {
                sandbox: inner.sandbox.clone(),
                deadline,
                cancel: cancel.clone(),
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let worker_dev = dev.clone();
            let req_len = request.len();
            std::thread::spawn(move || {
                let r = worker_dev.call(&call, &request);
                let _ = tx.send(r);
            });
            // Wait for the device, the deadline, a close, or the end of every
            // reader. A reader that gets a fatal signal cannot leave a FUSE
            // read on its own, so paddock ends the read for it.
            let result = loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Ok(out)) if out.len() > info.limits.max_response => {
                        break Err(Errno::EMSGSIZE)
                    }
                    Ok(Ok(out)) => break Ok(Arc::new(out)),
                    Ok(Err(_)) if Instant::now() >= deadline => break Err(Errno::ETIMEDOUT),
                    Ok(Err(e)) => break Err(Errno::from_i32(e.errno())),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err(Errno::EIO),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if Instant::now() >= deadline {
                    cancel.store(true, Ordering::SeqCst);
                    break Err(Errno::ETIMEDOUT);
                }
                if cancel.load(Ordering::SeqCst) || Self::readers_gone(&inner, fh) {
                    cancel.store(true, Ordering::SeqCst);
                    break Err(Errno::EINTR);
                }
            };
            inner.running.fetch_sub(1, Ordering::SeqCst);
            // Sizes and the outcome only: never the request or the response.
            let outcome = match &result {
                Ok(d) => format!("{} bytes back", d.len()),
                Err(e) if cancel.load(Ordering::SeqCst) && *e != Errno::ETIMEDOUT => {
                    "cancelled".into()
                }
                Err(e) => format!("error {}", i32::from(*e)),
            };
            log::info!(
                "sandbox {}: device {}: request of {req_len} bytes, {outcome}, {} ms",
                inner.sandbox,
                dev.name(),
                start.elapsed().as_millis()
            );
            Self::finish(&inner, fh, result);
        });
    }
}

/// Whether the thread `tid` has a fatal signal pending. The kernel marks
/// every fatal signal as SIGKILL in the pending set of each thread.
fn fatal_pending(tid: u32) -> bool {
    let Ok(text) = std::fs::read_to_string(format!("/proc/{tid}/status")) else {
        // Gone already.
        return true;
    };
    text.lines()
        .filter_map(|l| {
            l.strip_prefix("SigPnd:")
                .or_else(|| l.strip_prefix("ShdPnd:"))
        })
        .filter_map(|v| u64::from_str_radix(v.trim(), 16).ok())
        .any(|bits| bits & (1 << (libc::SIGKILL - 1)) != 0)
}
