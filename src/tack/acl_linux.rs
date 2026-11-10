//! The ACL walk on Linux: through descriptors, never following a link.

use super::super::protect::Grant;
use super::{
    perm_for, Acl, Entry, GrantReport, RevokeReport, ACCESS, DEFAULT, GROUP_OBJ, OTHER, R,
    USER_OBJ, W, X,
};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const MAX_ERRORS: usize = 5;

fn cpath(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

/// `/proc/self/fd/N`: the file of a descriptor, also an `O_PATH` one.
fn fd_path(fd: RawFd) -> CString {
    CString::new(format!("/proc/self/fd/{fd}")).expect("no NUL")
}

fn openat(dir: RawFd, name: &CString, flags: libc::c_int) -> io::Result<OwnedFd> {
    // SAFETY: name is a NUL-terminated string; the result is checked.
    let fd = unsafe {
        libc::openat(
            dir,
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    // SAFETY: st is a valid out-parameter.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

fn is_dir(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFDIR
}

fn is_file(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG
}

fn xname(which: &str) -> CString {
    CString::new(which).expect("no NUL")
}

/// The ACL attribute of a file, None when it has none.
fn get(p: &CString, which: &str) -> io::Result<Option<Acl>> {
    let name = xname(which);
    let mut buf = vec![0u8; 1024];
    loop {
        // SAFETY: buf is writable for its length.
        let n = unsafe {
            libc::getxattr(
                p.as_ptr(),
                name.as_ptr(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n >= 0 {
            buf.truncate(n as usize);
            return Acl::decode(&buf).map(Some).map_err(io::Error::other);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::ENODATA) => return Ok(None),
            Some(libc::ERANGE) if buf.len() < 1 << 20 => buf.resize(buf.len() * 4, 0),
            _ => return Err(e),
        }
    }
}

fn set(p: &CString, which: &str, a: &Acl) -> io::Result<()> {
    let name = xname(which);
    let b = a.encode();
    // SAFETY: b is readable for its length.
    if unsafe {
        libc::setxattr(
            p.as_ptr(),
            name.as_ptr(),
            b.as_ptr() as *const libc::c_void,
            b.len(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn del(p: &CString, which: &str) -> io::Result<()> {
    let name = xname(which);
    // SAFETY: both are NUL-terminated strings.
    if unsafe { libc::removexattr(p.as_ptr(), name.as_ptr()) } != 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ENODATA) {
            return Err(e);
        }
    }
    Ok(())
}

/// The ACL of a path (a link is not followed), for status and tests.
pub fn read(path: &Path, default: bool) -> io::Result<Option<Acl>> {
    let fd = open_path(path)?;
    get(
        &fd_path(fd.as_raw_fd()),
        if default { DEFAULT } else { ACCESS },
    )
}

fn open_path(path: &Path) -> io::Result<OwnedFd> {
    openat(libc::AT_FDCWD, &cpath(path)?, libc::O_PATH)
}

/// The base entries of an ACL: what a new default ACL starts from.
fn base(a: &Acl) -> Acl {
    let p = |tag| {
        a.0.iter()
            .find(|e| e.tag == tag)
            .map(|e| e.perm)
            .unwrap_or(0)
    };
    Acl(vec![
        Entry {
            tag: USER_OBJ,
            id: u32::MAX,
            perm: p(USER_OBJ),
        },
        Entry {
            tag: GROUP_OBJ,
            id: u32::MAX,
            perm: p(GROUP_OBJ),
        },
        Entry {
            tag: OTHER,
            id: u32::MAX,
            perm: p(OTHER),
        },
    ])
}

/// One entry of the walk: its folder and its name, opened when its turn
/// comes, so the walk holds about one descriptor per level.
struct Pending {
    dir: Rc<OwnedFd>,
    name: CString,
}

/// Walk `top` and every folder and file under it on the same file
/// system, through descriptors. `f` gets each folder and regular file.
fn walk(
    top: &Path,
    limit: usize,
    f: &mut dyn FnMut(&CString, &libc::stat) -> io::Result<()>,
    errors: &mut Vec<String>,
    skipped: &mut usize,
) -> io::Result<bool> {
    let first = openat(
        libc::AT_FDCWD,
        &cpath(top)?,
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    let st0 = fstat(first.as_raw_fd())?;
    let dev = st0.st_dev;
    let mut seen = 0usize;
    let err = |e: String, errors: &mut Vec<String>| {
        if errors.len() < MAX_ERRORS {
            errors.push(e);
        }
    };
    if let Err(e) = f(&fd_path(first.as_raw_fd()), &st0) {
        err(format!("{}: {e}", top.display()), errors);
    }
    let mut dirs: Vec<(Rc<OwnedFd>, PathBuf)> = vec![(Rc::new(first), top.to_path_buf())];
    let mut stack: Vec<(Pending, PathBuf)> = Vec::new();
    loop {
        if let Some((d, at)) = dirs.pop() {
            let list = std::fs::read_dir(format!("/proc/self/fd/{}", d.as_raw_fd()));
            match list {
                Ok(rd) => {
                    for e in rd.flatten() {
                        let name = e.file_name();
                        let Ok(c) = CString::new(name.as_bytes()) else {
                            continue;
                        };
                        stack.push((
                            Pending {
                                dir: d.clone(),
                                name: c,
                            },
                            at.join(&name),
                        ));
                    }
                }
                Err(e) => err(format!("{}: {e}", at.display()), errors),
            }
            continue;
        }
        let Some((p, at)) = stack.pop() else { break };
        seen += 1;
        if seen > limit {
            return Ok(true);
        }
        let fd = match openat(p.dir.as_raw_fd(), &p.name, libc::O_PATH) {
            Ok(fd) => fd,
            Err(e) => {
                err(format!("{}: {e}", at.display()), errors);
                continue;
            }
        };
        let st = fstat(fd.as_raw_fd())?;
        if st.st_dev != dev || !(is_dir(&st) || is_file(&st)) {
            *skipped += 1;
            continue;
        }
        if let Err(e) = f(&fd_path(fd.as_raw_fd()), &st) {
            err(format!("{}: {e}", at.display()), errors);
        }
        if is_dir(&st) {
            match openat(
                p.dir.as_raw_fd(),
                &p.name,
                libc::O_RDONLY | libc::O_DIRECTORY,
            ) {
                // Still the same folder: not swapped since it was checked.
                Ok(d)
                    if fstat(d.as_raw_fd())
                        .is_ok_and(|s| s.st_ino == st.st_ino && s.st_dev == dev) =>
                {
                    dirs.push((Rc::new(d), at))
                }
                Ok(_) => *skipped += 1,
                Err(e) => err(format!("{}: {e}", at.display()), errors),
            }
        }
    }
    Ok(false)
}

/// Whether the folder's own ACL has the grant already.
fn in_place(p: &CString, st: &libc::stat, g: &Grant) -> bool {
    let want = perm_for(true, 0, g.write);
    let access = get(p, ACCESS).ok().flatten();
    let default = get(p, DEFAULT).ok().flatten();
    is_dir(st)
        && access.is_some_and(|a| a.grants(g.helper, want))
        && default
            .is_some_and(|d| d.grants(g.helper, want) && (!g.write || d.grants(g.owner, R | W | X)))
}

/// Grant the helper `g.folder`, as the user `me` (tackd runs this in a
/// process of the user). See the module text.
pub fn grant(g: &Grant, me: u32, limit: usize) -> Result<GrantReport, String> {
    if me == 0 || me != g.owner {
        return Err(format!("a grant runs as uid {}, not as uid {me}", g.owner));
    }
    let mut rep = GrantReport::default();
    let show = |p: &Path| p.display().to_string();
    // The folders above: enter.
    for d in g.folder.ancestors().skip(1) {
        let fd = open_path(d).map_err(|e| format!("{}: {e}", show(d)))?;
        let st = fstat(fd.as_raw_fd()).map_err(|e| e.to_string())?;
        let p = fd_path(fd.as_raw_fd());
        let acl = get(&p, ACCESS).ok().flatten();
        if st.st_mode & 0o001 != 0 || acl.as_ref().is_some_and(|a| a.grants(g.helper, X)) {
            continue;
        }
        if st.st_uid != me {
            return Err(format!(
                "the helper account cannot enter {}: uid {} owns it and others cannot enter it, so a program at protect = \"uid\" cannot reach {}",
                show(d),
                st.st_uid,
                show(&g.folder)
            ));
        }
        let mut a = acl.unwrap_or_else(|| Acl::from_mode(st.st_mode));
        if a.add_user(g.helper, X) {
            set(&p, ACCESS, &a).map_err(|e| {
                format!(
                    "cannot let the helper enter {}: {e}{}",
                    show(d),
                    unsupported(&e)
                )
            })?;
            rep.changed += 1;
        }
    }
    let top = open_path(&g.folder).map_err(|e| format!("{}: {e}", show(&g.folder)))?;
    let st = fstat(top.as_raw_fd()).map_err(|e| e.to_string())?;
    if !is_dir(&st) {
        return Err(format!("{} is not a folder", show(&g.folder)));
    }
    if st.st_uid != me {
        // Not the user's: the user cannot change it, the helper reaches
        // it as any other user does.
        rep.not_owned = 1;
        return Ok(rep);
    }
    if in_place(&fd_path(top.as_raw_fd()), &st, g) {
        rep.already = true;
        return Ok(rep);
    }
    drop(top);
    let mut changed = 0usize;
    let mut not_owned = 0usize;
    let mut no_acls = false;
    let mut one = |p: &CString, st: &libc::stat| -> io::Result<()> {
        if st.st_uid != me {
            not_owned += 1;
            return Ok(());
        }
        let dir = is_dir(st);
        let mut a = match get(p, ACCESS)? {
            Some(a) => a,
            None => Acl::from_mode(st.st_mode),
        };
        let mut did = false;
        if a.add_user(g.helper, perm_for(dir, st.st_mode, g.write)) {
            if let Err(e) = set(p, ACCESS, &a) {
                no_acls |= e.raw_os_error() == Some(libc::EOPNOTSUPP);
                return Err(e);
            }
            did = true;
        }
        if dir {
            let mut d = get(p, DEFAULT)?.unwrap_or_else(|| base(&a));
            let mut ch = d.add_user(g.helper, perm_for(true, 0, g.write));
            if g.write {
                ch |= d.add_user(me, R | W | X);
            }
            if ch {
                set(p, DEFAULT, &d)?;
                did = true;
            }
        }
        if did {
            changed += 1;
        }
        Ok(())
    };
    let r = walk(
        &g.folder,
        limit,
        &mut one,
        &mut rep.errors,
        &mut rep.skipped,
    );
    rep.changed += changed;
    rep.not_owned += not_owned;
    rep.limit = r.map_err(|e| format!("{}: {e}", show(&g.folder)))?;
    if no_acls && rep.changed == 0 {
        return Err(format!(
            "the file system of {} has no POSIX ACLs{}",
            show(&g.folder),
            unsupported(&io::Error::from_raw_os_error(libc::EOPNOTSUPP))
        ));
    }
    Ok(rep)
}

fn unsupported(e: &io::Error) -> &'static str {
    if e.raw_os_error() == Some(libc::EOPNOTSUPP) {
        " (this file system has no POSIX ACLs: use protect = \"refs\" for programs that work there, or mount it with the acl option)"
    } else {
        ""
    }
}

/// Remove what `g` granted, as root: the helper's entries in the folder,
/// under it and above it; with write the user's default entry too. Files
/// of the helper go to the user (`owner_gid` is the user's group).
pub fn revoke(g: &Grant, owner_gid: u32, limit: usize) -> RevokeReport {
    let mut rep = RevokeReport::default();
    let strip =
        |p: &CString, st: &libc::stat, rep: &mut RevokeReport, inside: bool| -> io::Result<()> {
            let mut did = false;
            if let Some(mut a) = get(p, ACCESS)? {
                if a.remove_user(g.helper) {
                    // Only base entries left: the kernel turns it into the mode.
                    set(p, ACCESS, &a)?;
                    did = true;
                }
            }
            if is_dir(st) {
                if let Some(mut d) = get(p, DEFAULT)? {
                    let mut ch = d.remove_user(g.helper);
                    if g.write {
                        ch |= d.remove_user(g.owner);
                    }
                    if ch {
                        if d.named() {
                            set(p, DEFAULT, &d)?;
                        } else {
                            del(p, DEFAULT)?;
                        }
                        did = true;
                    }
                }
            }
            // Inside the folder only: a folder above is never the helper's.
            if inside && st.st_uid == g.helper {
                // p is /proc/self/fd/N: chown follows that link to the open
                // file itself, not to a path that could have changed.
                // SAFETY: p is a NUL-terminated string.
                if unsafe { libc::chown(p.as_ptr(), g.owner, owner_gid) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                rep.given_back += 1;
            }
            if did {
                rep.changed += 1;
            }
            Ok(())
        };
    for d in g.folder.ancestors().skip(1) {
        if let Ok(fd) = open_path(d) {
            if let Ok(st) = fstat(fd.as_raw_fd()) {
                if let Err(e) = strip(&fd_path(fd.as_raw_fd()), &st, &mut rep, false) {
                    rep.errors.push(format!("{}: {e}", d.display()));
                }
            }
        }
    }
    let mut errors = Vec::new();
    let mut skipped = 0;
    let mut inner = RevokeReport::default();
    let r = walk(
        &g.folder,
        limit,
        &mut |p, st| strip(p, st, &mut inner, true),
        &mut errors,
        &mut skipped,
    );
    rep.changed += inner.changed;
    rep.given_back += inner.given_back;
    rep.errors.extend(errors);
    match r {
        Ok(true) => rep
            .errors
            .push(format!("{}: stopped at {limit} files", g.folder.display())),
        Ok(false) => {}
        // A folder that is gone has nothing left to revoke.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => rep.errors.push(format!("{}: {e}", g.folder.display())),
    }
    rep
}

/// Whether the file system of `dir` takes an ACL entry for `uid`: a
/// temporary file gets one, it is read back, and the file is deleted.
pub fn probe(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let f = dir.join(format!(".paddock-acl-probe-{}-{nanos}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&f)
        .map_err(|e| format!("cannot make a file in {}: {e}", dir.display()))?;
    let r = (|| {
        let p = cpath(&f).map_err(|e| e.to_string())?;
        let mut a = Acl::from_mode(0o600);
        a.add_user(uid, R);
        set(&p, ACCESS, &a).map_err(|e| format!("{e}{}", unsupported(&e)))?;
        match get(&p, ACCESS).map_err(|e| e.to_string())? {
            Some(b) if b.grants(uid, R) => Ok(()),
            _ => Err("the entry did not stay".to_string()),
        }
    })();
    let _ = std::fs::remove_file(&f);
    r
}

/// The grants that tackd made: one JSON line each, in tackd's state
/// folder. A line is written before the grant starts.
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    pub fn new(state: &Path) -> Ledger {
        Ledger {
            path: state.join("acl.jsonl"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every grant, without repeats, oldest first. Lines that do not parse
    /// (a crash in the middle of a write) are skipped.
    pub fn all(&self) -> Vec<Grant> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut out: Vec<Grant> = Vec::new();
        for l in text.lines() {
            if let Ok(g) = serde_json::from_str::<Grant>(l) {
                if !out.contains(&g) {
                    out.push(g);
                }
            }
        }
        out
    }

    /// Put a grant on record (once), synced.
    pub fn add(&self, g: &Grant) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if self.all().contains(g) {
            return Ok(());
        }
        let mut line = b"\n".to_vec();
        serde_json::to_writer(&mut line, g).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)?;
        f.write_all(&line)?;
        f.sync_data()
    }

    /// Keep only the grants for which `keep` is true.
    pub fn retain(&self, keep: &dyn Fn(&Grant) -> bool) -> anyhow::Result<()> {
        let left: Vec<Grant> = self.all().into_iter().filter(|g| keep(g)).collect();
        if left.is_empty() {
            match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
                _ => return Ok(()),
            }
        }
        let mut text = String::new();
        for g in left {
            text.push_str(&serde_json::to_string(&g)?);
            text.push('\n');
        }
        super::super::service::atomic_write(&self.path, text.as_bytes(), 0o600)
    }
}
