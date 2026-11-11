//! The exec wrapper of `protect = "uid"`: the programs that a protected
//! program starts get the references, not the values.
//!
//! tackd puts a folder first on the PATH of a program at `uid`. The folder
//! has one link for each program name on the caller's PATH, all to tackd's
//! own binary. When the protected program runs `git`, it finds that link;
//! paddock sees from the name that the kernel ran (`AT_EXECFN`) that it is
//! the wrapper, and:
//!
//! 1. sets every secret of the user to its reference (`tack://NAME`);
//! 2. finds the real `git` on the PATH from before the folder;
//! 3. runs it, or, when the allowlist names it, asks tackd to start it
//!    with its own secrets, as a stand-in does (`standin`).
//!
//! The folder lives in tackd's state folder, root's, one for each set of
//! PATH folders and their contents (`ensure`). Only folders that every
//! user can list count, so the names show nothing that the helper could
//! not see.
//!
//! Not covered: a program that runs another one by its absolute path
//! (`/usr/bin/git`), or with its own copy of the environment from before,
//! does not pass the wrapper, and that child gets the values. A child runs
//! as the same helper account, so it could read the program's memory
//! anyway: the wrapper keeps values out of the environment of children,
//! such as a pager or a hook that prints it, not out of a child that
//! means harm.

#[cfg(target_os = "linux")]
use super::protect::{WRAP_DIR, WRAP_LISTED, WRAP_PATH, WRAP_SECRETS};
use sha2::{Digest, Sha256};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The most names in one wrapper folder.
pub const MAX_NAMES: usize = 50_000;
/// A wrapper folder that no start has used for this long is deleted.
const KEEP_DAYS: u64 = 30;
const DONE: &str = ".complete";

/// The PATH folders that the wrapper covers: absolute, there, listable and
/// enterable by every user up to `/`, without repeats and without `skip`.
pub fn dirs_for(path: &str, skip: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in path.split(':').map(Path::new) {
        if !d.is_absolute() || out.iter().any(|o| o == d) || skip.iter().any(|s| s == d) {
            continue;
        }
        let Ok(m) = std::fs::metadata(d) else {
            continue;
        };
        if !m.is_dir() || m.mode() & 0o005 != 0o005 {
            continue;
        }
        let open = d
            .ancestors()
            .skip(1)
            .all(|a| std::fs::metadata(a).is_ok_and(|m| m.mode() & 0o001 != 0));
        if open {
            out.push(d.to_path_buf());
        }
    }
    out
}

/// The name of a wrapper folder: the folders, and a stamp of each one's
/// content (a new program changes the folder's change time).
pub fn key(dirs: &[PathBuf]) -> String {
    let mut h = Sha256::new();
    for d in dirs {
        h.update(d.as_os_str().as_encoded_bytes());
        h.update([0]);
        if let Ok(m) = std::fs::metadata(d) {
            h.update(format!(
                "{}:{}:{}.{}",
                m.dev(),
                m.ino(),
                m.mtime(),
                m.mtime_nsec()
            ));
        }
        h.update([0]);
    }
    crate::manifest::hex(&h.finalize())[..24].to_string()
}

/// Whether a file name can be a link in the folder.
fn usable(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && !n.contains('/') && n.len() <= 255
}

/// The names in `dirs`, in order, without repeats.
pub fn names(dirs: &[PathBuf]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            continue;
        };
        for e in rd.flatten() {
            let ok = e.file_type().is_ok_and(|t| t.is_file() || t.is_symlink());
            if let (true, Some(n)) = (ok, e.file_name().to_str()) {
                if usable(n) && seen.len() < MAX_NAMES {
                    seen.insert(n.to_string());
                }
            }
        }
    }
    seen.into_iter().collect()
}

/// The wrapper folder for a PATH under `root` (tackd's `wrap` folder,
/// which `owner` owns), made when it is not there: links to `target` (the
/// binary of tackd). Made under a temporary name and renamed into place,
/// so a crash or a second start at the same time never leaves half a
/// folder in use.
pub fn ensure(
    root: &Path,
    owner: u32,
    path: &str,
    target: &Path,
    skip: &[PathBuf],
) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root)?;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755))?;
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let m = std::fs::symlink_metadata(root)?;
    if !m.is_dir() || m.uid() != owner || m.mode() & 0o022 != 0 {
        return Err(io::Error::other(format!(
            "{} is not a folder that only uid {owner} can write",
            root.display()
        )));
    }
    let mut skip = skip.to_vec();
    skip.push(root.to_path_buf());
    let dirs = dirs_for(path, &skip);
    let k = key(&dirs);
    let dir = root.join(&k);
    if dir.join(DONE).exists() {
        // In use: the sweep keeps it.
        if let Ok(f) = std::fs::File::open(&dir) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        return Ok(dir);
    }
    sweep(root, &k);
    let tmp = root.join(format!(
        ".new-{k}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let built = (|| -> io::Result<()> {
        std::fs::create_dir(&tmp)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        for n in names(&dirs) {
            std::os::unix::fs::symlink(target, tmp.join(n))?;
        }
        std::fs::write(
            tmp.join(DONE),
            dirs.iter()
                .map(|d| format!("{}\n", d.display()))
                .collect::<String>(),
        )?;
        std::fs::File::open(&tmp)?.sync_all()?;
        Ok(())
    })();
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &dir) {
        let _ = std::fs::remove_dir_all(&tmp);
        // Another start made it first.
        if !dir.join(DONE).exists() {
            return Err(e);
        }
    }
    Ok(dir)
}

/// Delete wrapper folders that no start used for `KEEP_DAYS`, and
/// temporary ones that a crash left.
fn sweep(root: &Path, keep: &str) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n == keep {
            continue;
        }
        let age = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let stale = if n.starts_with(".new-") {
            age > 3600
        } else {
            n.len() == 24 && n.bytes().all(|c| c.is_ascii_hexdigit()) && age > KEEP_DAYS * 86400
        };
        if stale {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The real program of `name`: the first executable file of that name on
/// `path` that is not in the wrapper folder, not a link to tackd's binary
/// (`me`) and not a tack stand-in.
pub fn real_program(name: &str, path: &str, wrap: &Path, me: Option<&Path>) -> Option<PathBuf> {
    let wrap_real = std::fs::canonicalize(wrap).ok();
    for d in path.split(':').map(Path::new) {
        if !d.is_absolute()
            || d == wrap
            || std::fs::canonicalize(d).ok() == wrap_real && wrap_real.is_some()
        {
            continue;
        }
        let p = d.join(name);
        let Ok(m) = std::fs::metadata(&p) else {
            continue;
        };
        if !m.is_file() || m.mode() & 0o111 == 0 || !crate::env::is_executable(&p) {
            continue;
        }
        let real = std::fs::canonicalize(&p).ok();
        if me.is_some_and(|me| real.as_deref() == Some(me)) || super::tackshims::is_shim(&p) {
            continue;
        }
        return Some(p);
    }
    None
}

/// The environment entries that the wrapper changes: each secret of the
/// list that is set becomes its reference.
pub fn references(names: &str, has: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
    names
        .split(',')
        .filter(|n| has(n))
        .filter_map(|n| super::Reference::new(n).map(|r| (n.to_string(), r.to_string())))
        .collect()
}

/// Whether the wrapper sends this program's start to tackd.
pub fn listed(listed: &str, program: &Path) -> bool {
    let real = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    listed
        .split(':')
        .filter(|l| !l.is_empty())
        .any(|l| Path::new(l) == real)
}

/// paddock started as the wrapper: the code of the program that it ran,
/// or None when this is not the wrapper (then paddock goes on as usual).
#[cfg(target_os = "linux")]
pub fn intercept() -> Option<i32> {
    use std::os::unix::ffi::OsStrExt;
    let wrap = PathBuf::from(std::env::var_os(WRAP_DIR)?);
    // SAFETY: getauxval has no preconditions; AT_EXECFN is a pointer to
    // the NUL-terminated path that execve got, or 0.
    let execfn = unsafe { libc::getauxval(libc::AT_EXECFN) } as *const libc::c_char;
    if execfn.is_null() {
        return None;
    }
    // SAFETY: the kernel put a NUL-terminated string there.
    let ran = unsafe { std::ffi::CStr::from_ptr(execfn) };
    let ran = Path::new(std::ffi::OsStr::from_bytes(ran.to_bytes()));
    if ran.parent() != Some(wrap.as_path()) {
        return None;
    }
    let name = ran.file_name()?.to_str()?.to_string();
    Some(run_wrapper(&wrap, &name))
}

#[cfg(target_os = "linux")]
fn run_wrapper(wrap: &Path, name: &str) -> i32 {
    use std::os::unix::process::CommandExt;
    let var = |n: &str| std::env::var(n).unwrap_or_default();
    let path = var(WRAP_PATH);
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok());
    let Some(real) = real_program(name, &path, wrap, me.as_deref()) else {
        eprintln!("{name}: command not found");
        return 127;
    };
    for (k, v) in references(&var(WRAP_SECRETS), &|n| std::env::var_os(n).is_some()) {
        std::env::set_var(k, v);
    }
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let argv0 = argv.first().cloned().unwrap_or_else(|| name.into());
    if listed(&var(WRAP_LISTED), &real) {
        return through_tackd(&real, &argv);
    }
    let e = std::process::Command::new(&real)
        .arg0(argv0)
        .args(&argv[1.min(argv.len())..])
        .exec();
    eprintln!("tack: cannot run {}: {e}", real.display());
    if e.kind() == io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

/// Start an allowlisted program through tackd, as the stand-in does.
#[cfg(target_os = "linux")]
fn through_tackd(real: &Path, argv: &[std::ffi::OsString]) -> i32 {
    use super::standin;
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let command: Option<Vec<String>> = argv.iter().map(|a| a.clone().into_string().ok()).collect();
    let Some(command) = command.filter(|c| !c.is_empty()) else {
        eprintln!("tack: {name}: an argument is not UTF-8, which tackd cannot pass");
        return standin::UNAVAILABLE;
    };
    let paths = super::service::Paths::system();
    let req = match standin::request(real, &command) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tack: {name} was not started: {e:#}");
            return standin::UNAVAILABLE;
        }
    };
    let socket = match std::os::unix::net::UnixStream::connect(&paths.socket) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "tack: {name} was not started: tackd does not answer on {} ({e})",
                paths.socket.display()
            );
            return standin::UNAVAILABLE;
        }
    };
    let fds: Vec<i32> = (0..3).filter(|i| req.stdio[*i as usize]).collect();
    let sigs = match standin::catch_signals() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tack: {name} was not started: {e}");
            return standin::UNAVAILABLE;
        }
    };
    match standin::session(&socket, &req, &fds, Some(sigs), &mut standin::stop_self) {
        Ok(o) => {
            if let Some(m) = standin::message(&command[0], &o) {
                eprintln!("{m}");
            }
            standin::end_like(&o)
        }
        Err(e) => {
            eprintln!("tack: {name} was not started: {e}");
            standin::UNAVAILABLE
        }
    }
}

#[cfg(test)]
#[path = "wrap_tests.rs"]
mod tests;
