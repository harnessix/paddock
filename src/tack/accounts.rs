//! The helper accounts of `protect = "uid"`: the passwd lookups (every
//! system), and creating and deleting them (`sudo paddock tack setup` and
//! `remove`, Linux).
//!
//! A helper account is a system account (`useradd --system`) with its own
//! group, no password (locked), no login shell, and a home of its own in
//! tackd's state folder (`/var/lib/paddock-tackd/home/NAME`, 0700, the
//! helper's). Its comment says whose it is (`protect::comment`): setup
//! adopts an account of that name only when the comment is paddock's, and
//! remove deletes only such an account.

use super::protect::{self, Helper};
use super::service::Runner;
use anyhow::{bail, Context as _, Result};
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// One passwd entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pw {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub gecos: String,
    pub shell: String,
}

fn from_raw(pw: &libc::passwd) -> Option<Pw> {
    let s = |p: *const libc::c_char| -> String {
        if p.is_null() {
            String::new()
        } else {
            // SAFETY: a non-null field of a passwd entry is a C string.
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    if pw.pw_name.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    // SAFETY: checked non-null above.
    let dir = unsafe { CStr::from_ptr(pw.pw_dir) };
    Some(Pw {
        name: s(pw.pw_name),
        uid: pw.pw_uid,
        gid: pw.pw_gid,
        home: PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())),
        gecos: s(pw.pw_gecos),
        shell: s(pw.pw_shell),
    })
}

/// The passwd entry of a uid (thread-safe).
pub fn by_uid(uid: u32) -> Option<Pw> {
    let mut buf = vec![0u8; 16 * 1024];
    // SAFETY: a zeroed passwd is a valid out-parameter.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: pw, buf and out are valid for the call.
    let r = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pw,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut out,
        )
    };
    if r != 0 || out.is_null() {
        return None;
    }
    from_raw(&pw)
}

/// The passwd entry of a name (thread-safe).
pub fn by_name(name: &str) -> Option<Pw> {
    let c = CString::new(name).ok()?;
    let mut buf = vec![0u8; 16 * 1024];
    // SAFETY: a zeroed passwd is a valid out-parameter.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: c, pw, buf and out are valid for the call.
    let r = unsafe {
        libc::getpwnam_r(
            c.as_ptr(),
            &mut pw,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut out,
        )
    };
    if r != 0 || out.is_null() {
        return None;
    }
    from_raw(&pw)
}

/// Whether the account behind a registry entry is still that helper: the
/// same name and uid, paddock's comment for that owner. tackd checks it at
/// each start, so a deleted and reused uid is not taken for a helper.
pub fn check(h: &Helper) -> Result<(), String> {
    match by_uid(h.uid) {
        None => Err(format!(
            "the helper account {} (uid {}) is gone: run sudo paddock tack setup",
            h.name, h.uid
        )),
        Some(pw) if pw.name != h.name || !protect::our_comment(&pw.gecos, h.owner) => Err(format!(
            "uid {} is now {} and not the helper {} of uid {}: run sudo paddock tack setup",
            h.uid, pw.name, h.name, h.owner
        )),
        Some(_) => Ok(()),
    }
}

/// The folder of the helpers' homes in tackd's state folder.
pub fn homes(state: &Path) -> PathBuf {
    state.join("home")
}

/// The shell of a helper: one that refuses a login.
fn nologin() -> &'static str {
    ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("/bin/false")
}

/// The users of this host that may use tack: uid 1000 to 59999 in
/// `/etc/passwd`, with a shell that allows a login.
pub fn people(passwd: &str) -> Vec<(String, u32)> {
    passwd
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 || l.starts_with('#') {
                return None;
            }
            let uid: u32 = f[2].parse().ok()?;
            let shell = f[6];
            let login =
                !shell.ends_with("nologin") && !shell.ends_with("/false") && !shell.is_empty();
            ((1000..60000).contains(&uid) && login).then(|| (f[0].to_string(), uid))
        })
        .collect()
}

/// The helper accounts in `/etc/passwd` by their comment: what remove
/// deletes even when the registry is gone.
pub fn ours_in(passwd: &str) -> Vec<(String, u32, u32)> {
    passwd
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            let uid: u32 = f[2].parse().ok()?;
            let owner: u32 = f[4]
                .strip_prefix(protect::MARK)?
                .strip_prefix(" for uid ")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()?;
            Some((f[0].to_string(), uid, owner))
        })
        .collect()
}

/// Make (or adopt) the helper of one user, and its home. `root` is the
/// file system root (`/` on a host), `owner` the uid that owns tackd's
/// folders (root).
pub fn ensure(state: &Path, user: &str, uid: u32, runner: &dyn Runner) -> Result<(Helper, bool)> {
    let name = protect::helper_name(user, uid);
    let home = homes(state).join(&name);
    let made = match by_name(&name) {
        Some(pw) if protect::our_comment(&pw.gecos, uid) => false,
        Some(pw) => bail!(
            "an account named {name} exists (uid {}) that paddock did not make for uid {uid}: rename or delete it, then run setup again",
            pw.uid
        ),
        None => {
            let comment = protect::comment(user, uid);
            let home_s = home.display().to_string();
            runner
                .run(
                    "useradd",
                    &[
                        "--system",
                        "--user-group",
                        "--no-create-home",
                        "--home-dir",
                        &home_s,
                        "--shell",
                        nologin(),
                        "--comment",
                        &comment,
                        &name,
                    ],
                )
                .with_context(|| format!("cannot make the helper account {name} for {user}"))?;
            true
        }
    };
    let pw = by_name(&name)
        .with_context(|| format!("{name} is not in the passwd database after useradd"))?;
    if pw.uid == 0 || pw.uid == uid {
        bail!("{name} has uid {}: delete it and run setup again", pw.uid);
    }
    make_home(&homes(state), &home, pw.uid, pw.gid)?;
    Ok((
        Helper {
            owner: uid,
            name,
            uid: pw.uid,
            gid: pw.gid,
            home,
        },
        made,
    ))
}

/// The helper's home: 0700, the helper's, in a root folder (0755).
fn make_home(parent: &Path, home: &Path, uid: u32, gid: u32) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    std::fs::create_dir_all(parent).with_context(|| format!("cannot make {}", parent.display()))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755))?;
    match std::fs::symlink_metadata(home) {
        Ok(m) if !m.is_dir() => bail!("{} is not a folder", home.display()),
        Ok(_) => {}
        Err(_) => {
            std::fs::create_dir(home).with_context(|| format!("cannot make {}", home.display()))?
        }
    }
    std::os::unix::fs::lchown(home, Some(uid), Some(gid))
        .with_context(|| format!("cannot give {} to uid {uid}", home.display()))?;
    std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
    let m = std::fs::symlink_metadata(home)?;
    if m.uid() != uid {
        bail!("{} does not belong to uid {uid}", home.display());
    }
    Ok(())
}

/// The processes of a uid, from `/proc`.
pub fn processes(proc: &Path, uid: u32) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir(proc) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            (e.metadata().ok()?.uid() == uid).then_some(pid)
        })
        .collect()
}

/// Delete one helper account: its processes end (SIGTERM, then SIGKILL
/// after 3 seconds), then the account, its group and its home.
pub fn delete(
    name: &str,
    uid: u32,
    home: Option<&Path>,
    runner: &dyn Runner,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let proc = Path::new("/proc");
    let running = processes(proc, uid);
    if !running.is_empty() {
        for &p in &running {
            // SAFETY: kill takes plain integers.
            unsafe { libc::kill(p, libc::SIGTERM) };
        }
        let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !processes(proc, uid).is_empty() && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        for p in processes(proc, uid) {
            // SAFETY: kill takes plain integers.
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        out.push(format!(
            "ended     {} programs that ran as {name}",
            running.len()
        ));
    }
    if by_name(name).is_some() {
        runner
            .run("userdel", &[name])
            .with_context(|| format!("cannot delete the account {name}"))?;
        out.push(format!("deleted   account {name} (uid {uid})"));
    }
    // useradd --user-group made a group of that name; userdel deletes it
    // on most systems. One that is left goes too.
    if group_exists(name) {
        let _ = runner.run("groupdel", &[name]);
    }
    if let Some(h) = home.filter(|h| h.exists()) {
        std::fs::remove_dir_all(h).with_context(|| format!("cannot delete {}", h.display()))?;
        out.push(format!("deleted   {}", h.display()));
    }
    Ok(out)
}

fn group_exists(name: &str) -> bool {
    let Ok(c) = CString::new(name) else {
        return false;
    };
    let mut buf = vec![0u8; 16 * 1024];
    // SAFETY: a zeroed group is a valid out-parameter.
    let mut gr: libc::group = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::group = std::ptr::null_mut();
    // SAFETY: c, gr, buf and out are valid for the call.
    let r = unsafe {
        libc::getgrnam_r(
            c.as_ptr(),
            &mut gr,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut out,
        )
    };
    r == 0 && !out.is_null()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_are_login_users_in_the_normal_uid_range() {
        let p = "root:x:0:0:root:/root:/bin/bash\n\
                 alice:x:1000:1000:Alice:/home/alice:/bin/zsh\n\
                 svc:x:1001:1001::/var/svc:/usr/sbin/nologin\n\
                 bob:x:1002:1002::/home/bob:/bin/false\n\
                 nobody:x:65534:65534::/:/bin/sh\n\
                 # a comment\n\
                 carol:x:2000:2000::/home/carol:/usr/bin/fish\n";
        assert_eq!(people(p), [("alice".into(), 1000), ("carol".into(), 2000)]);
    }

    #[test]
    fn our_accounts_are_found_by_their_comment() {
        let p = format!(
            "alice:x:1000:1000::/home/alice:/bin/zsh\n_tack_alice:x:981:981:{}:/var/lib/paddock-tackd/home/_tack_alice:/usr/sbin/nologin\n_tack_x:x:982:982:someone else:/:/bin/false\n",
            protect::comment("alice", 1000)
        );
        assert_eq!(ours_in(&p), [("_tack_alice".to_string(), 981, 1000)]);
    }

    #[test]
    fn the_running_user_is_found_by_uid_and_name() {
        // SAFETY: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        let Some(pw) = by_uid(me) else {
            eprintln!("SKIP: uid {me} has no passwd entry in this sandbox");
            return;
        };
        assert_eq!(pw.uid, me);
        assert_eq!(by_name(&pw.name).map(|p| p.uid), Some(me));
        assert!(by_name("no-such-user-paddock-test").is_none());
    }

    #[test]
    fn a_registry_entry_for_an_account_that_is_not_a_helper_is_refused() {
        // SAFETY: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        let Some(pw) = by_uid(me) else {
            eprintln!("SKIP: uid {me} has no passwd entry in this sandbox");
            return;
        };
        let h = Helper {
            owner: 4242,
            name: pw.name.clone(),
            uid: me,
            gid: pw.gid,
            home: "/x".into(),
        };
        let e = check(&h).unwrap_err();
        assert!(
            e.contains("not the helper") && e.contains("sudo paddock tack setup"),
            "{e}"
        );
        let gone = Helper {
            uid: 3_999_999_999,
            ..h
        };
        assert!(check(&gone).unwrap_err().contains("is gone"));
    }
}
