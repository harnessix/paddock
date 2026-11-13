//! The files of a folder: the scan, the copy, and the single reads and
//! writes of an apply.
//!
//! Every step walks from the top folder down through `safefs` and never
//! follows a symlink, also not in the middle of a path. The working copy
//! is sandbox-writable, and another sandbox can write the original.

use crate::safefs::{Dir, Kind, Root};
use crate::snapshot::Skip;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// More files than this: paddock asks for `--direct`.
pub const MAX_FILES: usize = 500_000;
const MAX_DEPTH: usize = 256;

/// What paddock records of one file or symlink. Folders have no entry:
/// their files carry the changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub size: u64,
    /// Nanoseconds since 1970. 0 for a symlink.
    pub mtime: i64,
    pub mode: u32,
    /// The target of a symlink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The BLAKE3 hash of the content, where paddock needed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

impl Entry {
    // The field types differ by platform: `st_mode` is 16 bits on macOS.
    #[allow(clippy::unnecessary_cast)]
    fn of(st: &libc::stat) -> Self {
        Entry {
            size: st.st_size as u64,
            mtime: (st.st_mtime as i64).saturating_mul(1_000_000_000) + st.st_mtime_nsec as i64,
            mode: (st.st_mode as u32) & 0o7777,
            link: None,
            hash: None,
        }
    }

    fn symlink(target: String) -> Self {
        Entry {
            size: 0,
            mtime: 0,
            mode: 0o777,
            link: Some(target),
            hash: None,
        }
    }

    /// The same size, time and mode, or the same link target. True means
    /// the same content without a read. False means "compare the content".
    pub fn same_fp(&self, o: &Entry) -> bool {
        match (&self.link, &o.link) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.size == o.size && self.mtime == o.mtime && self.mode == o.mode,
            _ => false,
        }
    }

    /// The change time in seconds since 1970.
    pub fn secs(&self) -> u64 {
        u64::try_from(self.mtime / 1_000_000_000).unwrap_or(0)
    }
}

pub type Manifest = BTreeMap<String, Entry>;

/// How a copy holds its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// `FICLONE`: the copy shares its data with the project.
    Reflink,
    /// `clonefileat` on APFS: the same.
    Clone,
    /// Every byte is copied.
    Copy,
}

impl Method {
    pub fn of(shared: bool) -> Self {
        match (shared, cfg!(target_os = "macos")) {
            (true, true) => Method::Clone,
            (true, false) => Method::Reflink,
            (false, _) => Method::Copy,
        }
    }

    pub fn shared(self) -> bool {
        self != Method::Copy
    }

    pub fn word(self) -> &'static str {
        match self {
            Method::Reflink => "reflink",
            Method::Clone => "clone",
            Method::Copy => "copy",
        }
    }
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    }
}

fn split(rel: &str) -> (&str, &str) {
    rel.rsplit_once('/').unwrap_or(("", rel))
}

fn lstat(dir: &Dir, name: &OsStr) -> Option<libc::stat> {
    let n = CString::new(name.as_bytes()).ok()?;
    // SAFETY: fstatat fills a zeroed stat; AT_SYMLINK_NOFOLLOW keeps links.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let r = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            n.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    (r == 0).then_some(st)
}

/// The files and symlinks of a folder.
#[derive(Debug, Default)]
pub struct Scan {
    pub files: Manifest,
    /// The top of each part that the rules leave out, such as `.git` or
    /// `target`.
    pub left_out: Vec<String>,
    /// Sockets, FIFOs, devices and names that are not UTF-8.
    pub special: Vec<String>,
}

/// Scan `root` without what `skip` names.
pub fn scan(root: &Path, skip: &Skip) -> Result<Scan> {
    let top = Root::open(root)?.dir(Path::new(""), false)?;
    let mut out = Scan::default();
    walk(&top, "", skip, 0, &mut out)?;
    Ok(out)
}

fn walk(dir: &Dir, rel: &str, skip: &Skip, depth: usize, out: &mut Scan) -> Result<()> {
    if depth > MAX_DEPTH {
        bail!(
            "{} is deeper than {MAX_DEPTH} folders",
            dir.path().display()
        );
    }
    let mut entries = dir.entries()?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (os, kind) in entries {
        let Some(name) = os.to_str() else {
            out.special.push(join(rel, &os.to_string_lossy()));
            continue;
        };
        let r = join(rel, name);
        if skip.skips(&r, name, kind == Kind::Dir) {
            out.left_out.push(r);
            continue;
        }
        match kind {
            Kind::Dir => {
                // A folder that went away or became a link: not a file.
                let Ok(sub) = dir.subdir(&os, false) else {
                    continue;
                };
                walk(&sub, &r, skip, depth + 1, out)?;
            }
            Kind::File => {
                if let Some(st) = lstat(dir, &os) {
                    out.files.insert(r, Entry::of(&st));
                }
            }
            Kind::Symlink => match dir
                .read_link(&os)
                .ok()
                .and_then(|t| t.to_str().map(str::to_string))
            {
                Some(t) => {
                    out.files.insert(r, Entry::symlink(t));
                }
                None => out.special.push(r),
            },
            Kind::Socket | Kind::Other => out.special.push(r),
        }
        if out.files.len() > MAX_FILES {
            bail!("{} has more than {MAX_FILES} files", dir.path().display());
        }
    }
    Ok(())
}

/// One folder, opened once, for single reads and writes by relative path.
pub struct Side {
    root: Root,
    path: std::path::PathBuf,
}

impl Side {
    pub fn open(path: &Path) -> Result<Side> {
        Ok(Side {
            root: Root::open(path)?,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder `rel`. With `create`, missing folders are made with mode
    /// 0755 (less the umask), as a tool of the user makes them.
    pub fn dir(&self, rel: &str, create: bool) -> Result<Dir> {
        if !create {
            return self.root.dir(Path::new(rel), false);
        }
        let mut d = self.root.dir(Path::new(""), false)?;
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            let name = OsStr::new(part);
            d.make_dir(name, 0o755)?;
            d = d.subdir(name, false)?;
        }
        Ok(d)
    }

    /// The entry at `rel`: a file or a symlink. None for a missing path, a
    /// folder or a special file.
    pub fn entry(&self, rel: &str) -> Option<Entry> {
        let (parent, name) = split(rel);
        let d = self.root.dir(Path::new(parent), false).ok()?;
        let st = lstat(&d, OsStr::new(name))?;
        match st.st_mode & libc::S_IFMT {
            libc::S_IFREG => Some(Entry::of(&st)),
            libc::S_IFLNK => {
                let t = d.read_link(OsStr::new(name)).ok()?;
                Some(Entry::symlink(t.to_str()?.to_string()))
            }
            _ => None,
        }
    }

    /// Something is at `rel`, of any kind.
    pub fn exists(&self, rel: &str) -> bool {
        let (parent, name) = split(rel);
        self.root
            .dir(Path::new(parent), false)
            .ok()
            .and_then(|d| lstat(&d, OsStr::new(name)))
            .is_some()
    }

    pub fn is_dir(&self, rel: &str) -> bool {
        self.root.dir(Path::new(rel), false).is_ok()
    }

    /// Open the regular file `rel`. None when it is missing or not a file.
    pub fn file(&self, rel: &str) -> Option<File> {
        let (parent, name) = split(rel);
        let d = self.root.dir(Path::new(parent), false).ok()?;
        d.open_read(OsStr::new(name)).ok()?
    }

    /// Replace the file `rel` with the content of `from` in one step: a
    /// new file next to it, then a rename. The file gets `mode`, and
    /// `mtime` when given.
    pub fn put_file(
        &self,
        rel: &str,
        from: &mut File,
        mode: u32,
        mtime: Option<i64>,
    ) -> Result<()> {
        let (parent, name) = split(rel);
        let d = self.dir(parent, true)?;
        let name = OsStr::new(name);
        if d.kind(name)? == Some(Kind::Dir) {
            bail!(
                "{} is a folder. Move it away first",
                self.path.join(rel).display()
            );
        }
        d.write_from(name, from, mode & 0o777)?;
        if let (Some(t), Ok(Some(f))) = (mtime, d.open_read(name)) {
            let t = std::time::UNIX_EPOCH
                + std::time::Duration::new(
                    t.div_euclid(1_000_000_000).max(0) as u64,
                    t.rem_euclid(1_000_000_000) as u32,
                );
            let _ = f.set_modified(t);
        }
        Ok(())
    }

    /// Replace `rel` with a symlink to `target`.
    pub fn put_link(&self, rel: &str, target: &str) -> Result<()> {
        let (parent, name) = split(rel);
        let d = self.dir(parent, true)?;
        let name = OsStr::new(name);
        match d.kind(name)? {
            Some(Kind::Dir) => bail!(
                "{} is a folder. Move it away first",
                self.path.join(rel).display()
            ),
            Some(_) => d.remove(name)?,
            None => {}
        }
        d.symlink(Path::new(target), name)
    }

    /// Put `e` at `rel`: the symlink, or the content of `from`.
    pub fn put(
        &self,
        rel: &str,
        e: &Entry,
        from: Option<&mut File>,
        keep_time: bool,
    ) -> Result<()> {
        match (&e.link, from) {
            (Some(t), _) => self.put_link(rel, t),
            (None, Some(f)) => self.put_file(rel, f, e.mode, keep_time.then_some(e.mtime)),
            (None, None) => bail!("no content for {rel}"),
        }
    }

    /// Remove the file or symlink `rel`, then the folders that it left
    /// empty, as long as `keep_dir` says no. Missing is fine.
    pub fn remove(&self, rel: &str, keep_dir: &dyn Fn(&str) -> bool) -> Result<()> {
        let (parent, name) = split(rel);
        let Ok(d) = self.root.dir(Path::new(parent), false) else {
            return Ok(());
        };
        if d.kind(OsStr::new(name))? == Some(Kind::Dir) {
            bail!(
                "{} is a folder. Move it away first",
                self.path.join(rel).display()
            );
        }
        d.remove(OsStr::new(name))?;
        let mut at = parent;
        while !at.is_empty() && !keep_dir(at) {
            let (up, n) = split(at);
            let Ok(d) = self.root.dir(Path::new(up), false) else {
                break;
            };
            // Only an empty folder goes.
            if d.remove_dir(OsStr::new(n)).is_err()
                || d.kind(OsStr::new(n)).ok().flatten().is_some()
            {
                break;
            }
            at = up;
        }
        Ok(())
    }
}

/// The BLAKE3 hash of a file, as hex.
pub fn hash(f: &mut File) -> Result<String> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_hex().to_string())
}

/// Both files hold the same bytes.
pub fn same_bytes(a: &mut File, b: &mut File) -> Result<bool> {
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let mut x = vec![0u8; 128 * 1024];
    let mut y = vec![0u8; 128 * 1024];
    loop {
        let n = read_full(a, &mut x)?;
        let m = read_full(b, &mut y)?;
        if n != m || x[..n] != y[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

/// A symlink at `rel` with `target` leads out of the folder, read by its
/// text alone: an absolute target, or more `..` than folders above it.
pub fn link_leaves(rel: &str, target: &str) -> bool {
    if target.starts_with('/') {
        return true;
    }
    let mut depth = rel.split('/').count() as i64 - 1;
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

/// What a copy made.
#[derive(Debug, Default)]
pub struct Cloned {
    pub files: usize,
    pub bytes: u64,
    /// Files of the list that went away before their copy.
    pub gone: Vec<String>,
}

/// Whether a file of `from` can share its data with a file in `to`.
pub fn shares(from: &Path, to: &Path, files: &Manifest) -> bool {
    #[cfg(test)]
    if NO_SHARE.with(|c| c.get()) {
        return false;
    }
    let Some((rel, _)) = files.iter().find(|(_, e)| e.link.is_none() && e.size > 0) else {
        return true;
    };
    let Ok(root) = Root::open(from) else {
        return false;
    };
    crate::shadow::clone::probe_reflink(&root, to, Path::new(rel))
}

fn open_up(d: &Dir) {
    // SAFETY: fchmod on an open folder fd.
    unsafe { libc::fchmod(d.as_raw_fd(), 0o755) };
}

/// Copy the listed files and symlinks of `from` into `to`, with shared
/// data when `shared`. The files keep their mode and change time.
pub fn clone_files(from: &Path, to: &Path, files: &Manifest, shared: bool) -> Result<Cloned> {
    let src = Root::open(from)?;
    let dst = Root::open(to)?;
    let mut out = Cloned::default();
    // The folders of the last file: the list is sorted, so neighbours
    // share them.
    let mut last: Option<(String, Dir, Dir)> = None;
    for (rel, e) in files {
        let (parent, name) = split(rel);
        if last.as_ref().is_none_or(|(p, _, _)| p != parent) {
            last = None;
            let Ok(f) = src.dir(Path::new(parent), false) else {
                out.gone.push(rel.clone());
                continue;
            };
            let t = make_dirs(&dst, parent)?;
            last = Some((parent.to_string(), f, t));
        }
        let (_, f, t) = last.as_ref().expect("set above");
        let name = OsStr::new(name);
        if let Some(target) = &e.link {
            let _ = t.remove(name);
            t.symlink(Path::new(target), name)?;
            out.files += 1;
            continue;
        }
        match crate::shadow::clone::copy_file(f, name, t, name, shared)
            .with_context(|| format!("cannot copy {rel}"))?
        {
            Some(n) => {
                out.files += 1;
                out.bytes += n;
            }
            None => out.gone.push(rel.clone()),
        }
    }
    Ok(out)
}

/// The folder `rel` of `root`, made with mode 0755 where it is missing.
fn make_dirs(root: &Root, rel: &str) -> Result<Dir> {
    let mut d = root.dir(Path::new(""), false)?;
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        let name = OsStr::new(part);
        let fresh = d.kind(name)?.is_none();
        d = d.subdir(name, true)?;
        if fresh {
            open_up(&d);
        }
    }
    Ok(d)
}

/// Copy `rel` of `from` into `to` with everything below it, as it is: no
/// rules, no record. For `.git` and build output. A plain copy counts
/// against `budget`. Symlinks are copied, never followed. Special files
/// stay out.
pub fn carry(from: &Path, to: &Path, rel: &str, shared: bool, budget: &mut u64) -> Result<()> {
    let src = Root::open(from)?;
    let dst = Root::open(to)?;
    let (parent, name) = split(rel);
    let Ok(f) = src.dir(Path::new(parent), false) else {
        return Ok(());
    };
    let name = OsStr::new(name);
    let Some(kind) = f.kind(name)? else {
        return Ok(());
    };
    let t = make_dirs(&dst, parent)?;
    carry_one(&f, &t, name, kind, shared, budget, 0)
}

fn carry_one(
    from: &Dir,
    to: &Dir,
    name: &OsStr,
    kind: Kind,
    shared: bool,
    budget: &mut u64,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DEPTH {
        bail!(
            "{} is deeper than {MAX_DEPTH} folders",
            from.path().display()
        );
    }
    match kind {
        Kind::Dir => {
            let Ok(sub) = from.subdir(name, false) else {
                return Ok(());
            };
            let fresh = to.kind(name)?.is_none();
            let out = to.subdir(name, true)?;
            if fresh {
                open_up(&out);
            }
            for (n, k) in sub.entries()? {
                carry_one(&sub, &out, &n, k, shared, budget, depth + 1)?;
            }
        }
        Kind::File => {
            if !shared {
                let size = lstat(from, name).map(|s| s.st_size as u64).unwrap_or(0);
                if size > *budget {
                    bail!("a plain copy passes the limit [copy] max");
                }
                *budget -= size;
            }
            crate::shadow::clone::copy_file(from, name, to, name, shared)?;
        }
        Kind::Symlink => {
            if let Ok(t) = from.read_link(name) {
                let _ = to.remove(name);
                to.symlink(&t, name)?;
            }
        }
        Kind::Socket | Kind::Other => {}
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Tests of the plain copy: act as if the file system cannot share data.
    pub(crate) static NO_SHARE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
