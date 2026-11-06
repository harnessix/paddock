//! The app in the guest: the relaunch counter, the crash report lookup
//! and the whole of `app.copy` are pure and tested here. The launch, the
//! quit and the exit watch with AppKit are in `apps_macos.rs`.
//!
//! `app.copy` writes `<Name>.app.partial` and renames it, so a killed session
//! never leaves half an app that Launch Services can start, and the old copy
//! stays until the new one is complete. The commands (`ditto`, `xattr`,
//! `lsregister`) are a closure, so the whole order runs in a Linux test with
//! a fake runner.

use super::configure::Plan;
use super::platform::Fail;
use crate::macvm::appcopy::{COPY_HEADROOM, GUEST_APPS};
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Three exits within this window stop the relaunch.
pub const RELAUNCH_WINDOW: Duration = Duration::from_secs(60);
/// The exits that the window allows.
pub const RELAUNCH_LIMIT: usize = 3;
/// A crash report that appears within this time after the exit belongs
/// to it.
pub const CRASH_WAIT: Duration = Duration::from_secs(5);

/// The relaunch counter of one app.
#[derive(Debug, Clone, Default)]
pub struct Relaunch {
    exits: VecDeque<Instant>,
}

impl Relaunch {
    /// Record an exit. True when the host may relaunch: fewer than
    /// [`RELAUNCH_LIMIT`] exits within [`RELAUNCH_WINDOW`], this one
    /// included.
    pub fn record(&mut self, now: Instant) -> bool {
        while self
            .exits
            .front()
            .is_some_and(|t| now.duration_since(*t) > RELAUNCH_WINDOW)
        {
            self.exits.pop_front();
        }
        self.exits.push_back(now);
        self.exits.len() < RELAUNCH_LIMIT
    }

    /// Forget the exits: a quit that the host asked for is not a crash.
    pub fn reset(&mut self) {
        self.exits.clear();
    }
}

/// The crash report folder of the guest user.
pub fn crash_reports_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library").join("Logs").join("DiagnosticReports")
}

/// The crash report of `pid` written at or after `since`: an `.ips` file
/// of the folder whose head names the pid. The newest when several do.
pub fn find_crash_report(dir: &Path, pid: u32, since: SystemTime) -> Option<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "ips") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < since {
            continue;
        }
        if names_pid(&path, pid) {
            found.push((modified, path));
        }
    }
    found.sort();
    found.pop().map(|(_, p)| p)
}

/// True when the head of a report (`.ips` is JSON lines: a summary line,
/// then the report) carries `"pid":PID`.
fn names_pid(path: &Path, pid: u32) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = vec![0u8; 16 * 1024];
    let n = f.read(&mut head).unwrap_or(0);
    let text = String::from_utf8_lossy(&head[..n]);
    let needle = format!("\"pid\":{pid}");
    text.contains(&needle) || text.replace(' ', "").contains(&needle)
}

// ---- `app.copy` -------------------------------------------------------

/// The Applications folder of the guest home, where every copy lives.
pub fn apps_dir() -> PathBuf {
    Path::new(super::data::HOME).join(GUEST_APPS)
}

/// `ditto`, which copies a bundle with its resource forks and its
/// signature intact.
pub const DITTO: &str = "/usr/bin/ditto";
/// `codesign`, which checks the signature of the copy.
pub const CODESIGN: &str = "/usr/bin/codesign";
/// `xattr`, which removes the quarantine attribute.
pub const XATTR: &str = "/usr/bin/xattr";
/// Launch Services' own tool, which registers the copy so that the Dock
/// and `NSWorkspace.openApplication` find it.
pub const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// One copy, with its paths checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    /// The clone of the host bundle, under the read-only `apps` share.
    pub source: PathBuf,
    pub target: PathBuf,
    /// `<target>.partial`: the name the copy carries until it is whole.
    pub partial: PathBuf,
    /// The guest may remove `com.apple.quarantine` from the copy.
    pub unquarantine: bool,
}

/// Check the paths of an `app.copy`. The target has to be an `.app` folder
/// directly in the Applications folder of the guest home, so a host that
/// asks for anything else writes nothing.
pub fn plan_copy(from: &Path, to: &Path, unquarantine: bool) -> Result<Copy, Fail> {
    let bad = || (ErrorCode::BadRequest, text::copy_target_refused(to));
    let name = to.file_name().and_then(|n| n.to_str()).ok_or_else(bad)?;
    if !name.ends_with(".app") || name.len() <= 4 || to.parent() != Some(&apps_dir()) {
        return Err(bad());
    }
    if from.file_name().is_none() || !from.is_absolute() {
        return Err((ErrorCode::BadRequest, text::not_app_bundle(from)));
    }
    Ok(Copy {
        partial: crate::macvm::layout::partial(to),
        source: from.to_path_buf(),
        target: to.to_path_buf(),
        unquarantine,
    })
}

impl Copy {
    /// `ditto -x <archive> <partial>`: the whole bundle, in one command,
    /// from the archive that the host wrote with `ditto -c`. A source that
    /// is a folder (an older host) is `ditto <source> <partial>`.
    pub fn ditto(&self) -> Plan {
        let mut argv = vec![DITTO.to_string()];
        if self.source.extension().and_then(|e| e.to_str())
            == Some(crate::macvm::appcopy::ARCHIVE_EXT)
        {
            argv.push("-x".into());
        }
        argv.push(self.source.display().to_string());
        argv.push(self.partial.display().to_string());
        Plan {
            argv,
            password_on_stdin: false,
        }
    }

    /// `codesign --verify --deep --strict <partial>`: the copy carries an
    /// intact signature, before it takes the final name. A broken one
    /// would make macOS call the app damaged.
    pub fn verify(&self) -> Plan {
        Plan {
            argv: vec![
                CODESIGN.into(),
                "--verify".into(),
                "--deep".into(),
                "--strict".into(),
                self.partial.display().to_string(),
            ],
            password_on_stdin: false,
        }
    }

    /// `xattr -d -r com.apple.quarantine <target>`, when the host copy
    /// carried the user's approval.
    pub fn xattr(&self) -> Option<Plan> {
        self.unquarantine.then(|| Plan {
            argv: vec![
                XATTR.into(),
                "-d".into(),
                "-r".into(),
                "com.apple.quarantine".into(),
                self.target.display().to_string(),
            ],
            password_on_stdin: false,
        })
    }

    /// `lsregister -f <target>`: Launch Services learns the new copy.
    pub fn lsregister(&self) -> Plan {
        Plan {
            argv: vec![
                LSREGISTER.into(),
                "-f".into(),
                self.target.display().to_string(),
            ],
            password_on_stdin: false,
        }
    }

    /// The free space the copy needs on the data disk: the bundle plus
    /// `COPY_HEADROOM`.
    pub fn needs(&self, bundle_bytes: u64) -> u64 {
        bundle_bytes.saturating_add(COPY_HEADROOM)
    }
}

/// Run one copy. `app` names the app in the texts, `free` is the free
/// space of the data disk when the agent knows it, and `run` is
/// `configure::spawn` on macOS and a fake in the tests.
///
/// The order never leaves a half app and never loses the old one: the
/// space check, an old partial away, `ditto` into the partial, the old
/// copy away, the rename, then the quarantine and Launch Services. A
/// failure before the rename keeps the old copy exactly as it was.
pub fn run_copy(
    c: &Copy,
    app: &str,
    free: Option<u64>,
    run: &dyn Fn(&Plan) -> std::io::Result<(bool, String)>,
) -> Result<(), Fail> {
    let needs = c.needs(crate::macvm::appcopy::size(&c.source));
    if let Some(free) = free {
        if free < needs {
            return Err((ErrorCode::Io, text::copy_no_space(app, needs, free)));
        }
    }
    remove_tree(&c.partial).map_err(|e| io_fail("the removal of the old partial copy", &e))?;
    if let Some(parent) = c.target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_fail("the Applications folder", &e))?;
    }
    let (ok, out) = run(&c.ditto()).map_err(|e| io_fail("ditto", &e))?;
    if !ok {
        let _ = remove_tree(&c.partial);
        return Err((
            ErrorCode::Io,
            text::copy_command_failed("ditto", out.trim()),
        ));
    }
    let (ok, out) = run(&c.verify()).map_err(|e| io_fail("codesign", &e))?;
    if !ok {
        let _ = remove_tree(&c.partial);
        return Err((ErrorCode::Io, text::copy_signature_broken(app, out.trim())));
    }
    remove_tree(&c.target).map_err(|e| io_fail("the removal of the old copy", &e))?;
    std::fs::rename(&c.partial, &c.target).map_err(|e| io_fail("rename", &e))?;
    // From here the copy is in place. The quarantine and Launch Services
    // are best effort: a failure of either does not undo a good copy.
    if let Some(plan) = c.xattr() {
        match run(&plan) {
            // A bundle without the attribute makes xattr fail; that is
            // the outcome paddock wants anyway.
            Ok((ok, out)) if !ok => log::info!("xattr of the copy: {}", out.trim()),
            Ok(_) => {}
            Err(e) => log::warn!("xattr did not run: {e}"),
        }
    }
    match run(&c.lsregister()) {
        Ok((false, out)) => log::warn!("lsregister: {}", out.trim()),
        Ok(_) => {}
        Err(e) => log::warn!("lsregister did not run: {e}"),
    }
    Ok(())
}

fn io_fail(what: &str, e: &dyn std::fmt::Display) -> Fail {
    (
        ErrorCode::Io,
        text::copy_command_failed(what, &e.to_string()),
    )
}

/// Remove a file or a folder tree. A path that is not there is no error.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_stops_after_three_in_a_minute() {
        let mut r = Relaunch::default();
        let t0 = Instant::now();
        assert!(r.record(t0));
        assert!(r.record(t0 + Duration::from_secs(10)));
        assert!(
            !r.record(t0 + Duration::from_secs(20)),
            "the third exit stops it"
        );
        assert!(!r.record(t0 + Duration::from_secs(30)));
        // After the window the count starts again.
        assert!(r.record(t0 + Duration::from_secs(100)));
        assert!(r.record(t0 + Duration::from_secs(101)));
        r.reset();
        assert!(r.record(t0 + Duration::from_secs(102)));
    }

    #[test]
    fn crash_report_by_pid_and_time() {
        let d = tempfile::tempdir().unwrap();
        let since = SystemTime::now() - Duration::from_secs(1);
        let old = d.path().join("ChatGPT-2026-11-03-100000.ips");
        std::fs::write(&old, "{\"pid\":4242}\n{...}").unwrap();
        let past = std::fs::FileTimes::new().set_modified(since - Duration::from_secs(3600));
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(past)
            .unwrap();
        let other = d.path().join("Safari-2026-11-04-100000.ips");
        std::fs::write(&other, "{\"pid\" : 99}\n").unwrap();
        let txt = d.path().join("notes.txt");
        std::fs::write(&txt, "\"pid\":4242").unwrap();
        assert_eq!(
            find_crash_report(d.path(), 4242, since),
            None,
            "old and wrong files do not count"
        );
        let new = d.path().join("ChatGPT-2026-11-04-100000.ips");
        std::fs::write(&new, "{\"app_name\":\"ChatGPT\",\"pid\" : 4242}\n").unwrap();
        assert_eq!(find_crash_report(d.path(), 4242, since), Some(new));
        assert_eq!(find_crash_report(d.path(), 99, since), Some(other));
        assert_eq!(find_crash_report(&d.path().join("none"), 1, since), None);
    }
}

#[cfg(test)]
mod copy_tests {
    use super::*;
    use std::cell::RefCell;

    /// A source bundle on disk and a target under a fake guest home. The
    /// Applications folder of the real guest is an absolute path, so the
    /// test builds a `Copy` by hand after `plan_copy` checked the rules.
    fn fixture(t: &Path) -> (Copy, PathBuf) {
        let source = t.join("share/apps/ChatGPT.app");
        std::fs::create_dir_all(source.join("Contents")).unwrap();
        std::fs::write(source.join("Contents/Info.plist"), vec![0u8; 1000]).unwrap();
        let target = t.join("home/Applications/ChatGPT.app");
        let c = Copy {
            partial: crate::macvm::layout::partial(&target),
            source,
            target: target.clone(),
            unquarantine: true,
        };
        (c, target)
    }

    /// A runner that records the commands and copies the tree for ditto.
    fn runner(
        log: &RefCell<Vec<String>>,
        ditto_ok: bool,
    ) -> impl Fn(&Plan) -> std::io::Result<(bool, String)> + '_ {
        move |plan: &Plan| {
            log.borrow_mut().push(plan.argv.join(" "));
            if plan.argv[0] == DITTO {
                if !ditto_ok {
                    return Ok((false, "ditto: No space left on device\n".into()));
                }
                let (from, to) = (Path::new(&plan.argv[1]), Path::new(&plan.argv[2]));
                crate::macvm::drop::clone_into(from, to)?;
            }
            Ok((true, String::new()))
        }
    }

    #[test]
    fn plan_copy_refuses_a_target_outside_the_guest_applications() {
        let apps = apps_dir();
        // The Applications folder of the home on the data disk.
        assert_eq!(apps, Path::new(super::super::data::HOME).join(GUEST_APPS));
        assert!(apps.ends_with("Applications"));
        assert!(apps.starts_with(super::super::data::MOUNT));
        let from = Path::new("/Volumes/My Shared Files/apps/ChatGPT.app");
        let c = plan_copy(from, &apps.join("ChatGPT.app"), true).unwrap();
        assert_eq!(c.target, apps.join("ChatGPT.app"));
        assert_eq!(
            c.partial,
            apps.join("ChatGPT.app.partial"),
            "the copy carries the partial name until it is whole"
        );
        for bad in [
            PathBuf::from("/etc/ChatGPT.app"),
            apps.join("sub/ChatGPT.app"),
            apps.join("ChatGPT"),
            apps.join(".app"),
            PathBuf::from("/"),
        ] {
            let e = plan_copy(from, &bad, true).unwrap_err();
            assert_eq!(e.0, ErrorCode::BadRequest, "{}", bad.display());
            assert!(
                e.1.contains("Applications") || e.1.contains("not an app"),
                "{}",
                e.1
            );
        }
        // A relative source is refused too.
        let e = plan_copy(Path::new("apps/X.app"), &apps.join("X.app"), false).unwrap_err();
        assert_eq!(e.0, ErrorCode::BadRequest);
    }

    #[test]
    fn a_copy_runs_ditto_then_the_rename_then_launch_services() {
        let t = tempfile::tempdir().unwrap();
        let (c, target) = fixture(t.path());
        let log = RefCell::new(Vec::new());
        run_copy(&c, "ChatGPT", Some(8 << 30), &runner(&log, true)).unwrap();
        assert!(target.join("Contents/Info.plist").is_file());
        assert!(!c.partial.exists(), "no partial is left");
        let calls = log.borrow().clone();
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert!(calls[0].starts_with(DITTO) && calls[0].ends_with(".app.partial"));
        // The signature is checked on the partial, before the rename.
        assert_eq!(calls[1], c.verify().argv.join(" "));
        assert!(calls[1].starts_with(CODESIGN) && calls[1].contains("--strict"));
        assert!(calls[2].starts_with(XATTR) && calls[2].contains("com.apple.quarantine"));
        assert!(calls[2].contains("-r"), "the whole tree: {}", calls[2]);
        assert!(calls[3].starts_with(LSREGISTER) && calls[3].contains("-f"));

        // Without the approval the quarantine stays: no xattr call.
        let mut keep = c.clone();
        keep.unquarantine = false;
        assert!(keep.xattr().is_none());
        let log = RefCell::new(Vec::new());
        run_copy(&keep, "ChatGPT", None, &runner(&log, true)).unwrap();
        assert_eq!(log.borrow().len(), 3);
    }

    #[test]
    fn an_archive_is_unpacked_and_a_broken_signature_keeps_the_old_copy() {
        let t = tempfile::tempdir().unwrap();
        let (mut c, target) = fixture(t.path());
        // The host's archive: `ditto -x ARCHIVE PARTIAL`.
        c.source = t.path().join("share/apps/ChatGPT.app.cpio");
        assert_eq!(
            c.ditto().argv,
            [
                DITTO.to_string(),
                "-x".into(),
                c.source.display().to_string(),
                c.partial.display().to_string()
            ]
        );
        // A copy whose signature does not verify never takes the name.
        std::fs::create_dir_all(target.join("Contents")).unwrap();
        std::fs::write(target.join("Contents/old"), b"v1").unwrap();
        let (c, _) = fixture(t.path());
        let log = RefCell::new(Vec::new());
        let e = run_copy(&c, "ChatGPT", None, &|plan: &Plan| {
            log.borrow_mut().push(plan.argv[0].clone());
            if plan.argv[0] == DITTO {
                crate::macvm::drop::clone_into(Path::new(&plan.argv[1]), Path::new(&plan.argv[2]))?;
                return Ok((true, String::new()));
            }
            Ok((
                plan.argv[0] != CODESIGN,
                "a sealed resource is missing".into(),
            ))
        })
        .unwrap_err();
        assert!(
            e.1.contains("damaged") && e.1.contains("sealed resource"),
            "{}",
            e.1
        );
        assert_eq!(std::fs::read(target.join("Contents/old")).unwrap(), b"v1");
        assert!(!c.partial.exists(), "no partial is left");
        assert_eq!(*log.borrow(), [DITTO, CODESIGN]);
    }

    #[test]
    fn a_failed_ditto_keeps_the_old_copy_and_leaves_no_partial() {
        let t = tempfile::tempdir().unwrap();
        let (c, target) = fixture(t.path());
        // An old copy of the app that must survive the failure.
        std::fs::create_dir_all(target.join("Contents")).unwrap();
        std::fs::write(target.join("Contents/old"), b"v1").unwrap();
        // A partial of a killed earlier session, which goes first.
        std::fs::create_dir_all(c.partial.join("Contents")).unwrap();

        let log = RefCell::new(Vec::new());
        let e = run_copy(&c, "ChatGPT", Some(8 << 30), &runner(&log, false)).unwrap_err();
        assert_eq!(e.0, ErrorCode::Io);
        assert!(e.1.contains("No space left on device"), "{}", e.1);
        assert!(e.1.contains("ditto"), "{}", e.1);
        assert_eq!(
            std::fs::read(target.join("Contents/old")).unwrap(),
            b"v1",
            "the old copy is untouched"
        );
        assert!(!c.partial.exists(), "no partial is left");
        assert_eq!(log.borrow().len(), 1, "nothing ran after the failed ditto");

        // A second try with a working ditto replaces the old copy.
        let log = RefCell::new(Vec::new());
        run_copy(&c, "ChatGPT", Some(8 << 30), &runner(&log, true)).unwrap();
        assert!(!target.join("Contents/old").exists());
        assert!(target.join("Contents/Info.plist").is_file());
    }

    #[test]
    fn too_little_space_refuses_before_anything_runs() {
        let t = tempfile::tempdir().unwrap();
        let (c, target) = fixture(t.path());
        assert_eq!(c.needs(1000), 1000 + COPY_HEADROOM);
        assert_eq!(c.needs(u64::MAX), u64::MAX, "no overflow");
        let log = RefCell::new(Vec::new());
        let e = run_copy(&c, "ChatGPT", Some(1 << 20), &runner(&log, true)).unwrap_err();
        assert!(e.1.contains("no room in the guest for ChatGPT"), "{}", e.1);
        assert!(e.1.contains("The old copy stays"), "{}", e.1);
        assert!(e.1.contains("paddock app reset ChatGPT --data"), "{}", e.1);
        assert!(log.borrow().is_empty() && !target.exists());
    }
}
