use super::*;
use sha2::Sha256;
use std::cell::RefCell;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

fn uid() -> u32 {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

struct World {
    _t: tempfile::TempDir,
    root: PathBuf,
    records: PathBuf,
    /// The pids that count as live.
    live: RefCell<Vec<u32>>,
    procs: Fake,
}

impl World {
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("tmp");
        std::fs::create_dir(&root).unwrap();
        let records = t.path().join("state/tmp");
        World {
            root,
            records,
            live: RefCell::new(vec![]),
            procs: Fake::default(),
            _t: t,
        }
    }

    fn claim(&self, run: &str, pid: u32, wants: &[Resolved]) -> Claim {
        self.live.borrow_mut().push(pid);
        let live = |p: u32| self.live.borrow().contains(&p);
        let place = Place {
            root: &self.root,
            records: &self.records,
            uid: uid(),
            live: &live,
            procs: &self.procs,
        };
        claim(&place, run, pid, wants).unwrap()
    }

    fn release(&self, run: &str) -> Vec<String> {
        let live = |p: u32| self.live.borrow().contains(&p);
        let place = Place {
            root: &self.root,
            records: &self.records,
            uid: uid(),
            live: &live,
            procs: &self.procs,
        };
        release(&place, run)
    }

    fn release_dead(&self) -> Vec<String> {
        let live = |p: u32| self.live.borrow().contains(&p);
        let place = Place {
            root: &self.root,
            records: &self.records,
            uid: uid(),
            live: &live,
            procs: &self.procs,
        };
        release_dead(&place)
    }

    fn stop_stale(&self, home: &str, keep: &[PathBuf]) -> Vec<u32> {
        let live = |p: u32| self.live.borrow().contains(&p);
        let place = Place {
            root: &self.root,
            records: &self.records,
            uid: uid(),
            live: &live,
            procs: &self.procs,
        };
        stop_stale(&place, Path::new(home), keep).0
    }

    /// Give the record of `run` the probe paths that the start writes.
    fn started(&self, run: &str, home: &str) {
        let mut r = load_all(&self.records)
            .into_iter()
            .find(|r| r.run == run)
            .unwrap();
        r.probe_read = make_probe(&self.records, run).unwrap();
        r.probe_write = PathBuf::from(format!("{home}/.paddock-tmp/{run}"));
        r.home = PathBuf::from(home);
        save(&self.records, &r).unwrap();
    }

    fn end(&self, pid: u32) {
        self.live.borrow_mut().retain(|p| *p != pid);
    }
}

/// A process of the fake host: the sandbox it runs in (its run and its
/// home, or none for a host process) and how it takes a signal.
#[derive(Clone, Default)]
struct P {
    run: String,
    home: String,
    /// It ignores SIGTERM.
    stubborn: bool,
    /// It survives SIGKILL too (a process in the kernel).
    stuck: bool,
}

#[derive(Default)]
struct Fake {
    procs: RefCell<std::collections::BTreeMap<u32, P>>,
    signals: RefCell<Vec<(u32, bool)>>,
}

impl Fake {
    fn add(&self, pid: u32, run: &str, home: &str) {
        self.procs.borrow_mut().insert(
            pid,
            P {
                run: run.into(),
                home: home.into(),
                ..P::default()
            },
        );
    }
    fn set(&self, pid: u32, f: impl FnOnce(&mut P)) {
        f(self.procs.borrow_mut().get_mut(&pid).unwrap());
    }
    fn runs(&self, pid: u32) -> bool {
        self.procs.borrow().contains_key(&pid)
    }
}

impl Procs for Fake {
    fn list(&self) -> Vec<u32> {
        self.procs.borrow().keys().copied().collect()
    }
    /// As `sandbox_check` on the Mac: a probe path that does not exist is
    /// not allowed, so a run whose probe is gone has no processes.
    fn of_run(&self, pid: u32, rec: &Record) -> bool {
        self.procs.borrow().get(&pid).is_some_and(|p| {
            !p.run.is_empty() && reads(&p.run, &rec.probe_read) && rec.probe_read.exists()
        })
    }
    fn of_home(&self, pid: u32, home: &Path, keep: &[PathBuf]) -> bool {
        self.procs.borrow().get(&pid).is_some_and(|p| {
            !p.run.is_empty()
                && Path::new(&p.home) == home
                && !keep.iter().any(|k| reads(&p.run, k))
        })
    }
    fn alive(&self, pid: u32) -> bool {
        self.runs(pid)
    }
    fn signal(&self, pid: u32, kill: bool) {
        self.signals.borrow_mut().push((pid, kill));
        let mut m = self.procs.borrow_mut();
        let Some(p) = m.get(&pid) else { return };
        if (kill && !p.stuck) || (!kill && !p.stubborn) {
            m.remove(&pid);
        }
    }
    fn grace(&self) -> std::time::Duration {
        std::time::Duration::from_millis(20)
    }
}

/// The profile of `run` allows to read `path`: its run folder or its probe.
fn reads(run: &str, path: &Path) -> bool {
    path == Path::new(&format!("/run/{run}"))
        || path
            .file_name()
            .is_some_and(|n| n.to_string_lossy() == format!("{run}{PROBE}"))
}

fn want(name: &str, kind: WantKind, entries: &[&str]) -> Resolved {
    Resolved {
        name: name.into(),
        kind,
        entries: entries.iter().map(|s| s.to_string()).collect(),
        replaces_args: vec![],
        stop: vec![],
        why: String::new(),
        home_bound: false,
    }
}

fn shared(entry: &str) -> Resolved {
    let mut w = want("codex-daemon-501", WantKind::Shared, &[entry]);
    w.replaces_args = vec!["--no-daemon".into()];
    w
}

#[test]
fn templates_expand_and_a_value_is_never_a_template() {
    let home = Path::new("/Users/a/{uid}");
    let v = Vars {
        uid: 501,
        home,
        run: "r1",
    };
    assert_eq!(
        expand("codex-daemon-{uid}", &v).unwrap(),
        "codex-daemon-501"
    );
    assert_eq!(expand("x-{run}", &v).unwrap(), "x-r1");
    let sock = "/Users/a/{uid}/.codex/app-server-control/app-server-control.sock";
    let hash = crate::manifest::hex(&Sha256::digest(sock.as_bytes()));
    assert_eq!(
        expand(
            "{sha256:{home}/.codex/app-server-control/app-server-control.sock}.lock",
            &v
        )
        .unwrap(),
        format!("{hash}.lock")
    );
    for bad in ["{nope}", "a{uid", "a}", "{sha256:abc"] {
        assert!(expand(bad, &v).is_err(), "{bad}");
    }
}

#[test]
fn a_name_is_one_plain_part() {
    for bad in ["", ".", "..", "a/b", "a\nb"] {
        assert!(check_name(bad).is_err(), "{bad:?}");
    }
    check_name("codex-daemon-501").unwrap();
    let v = Vars {
        uid: 1,
        home: Path::new("/h"),
        run: "r",
    };
    let w = Want {
        name: "{home}".into(),
        ..Want::default()
    };
    assert!(resolve(&w, &v).is_err(), "a home path is not one name");
    let w = Want {
        name: "x".into(),
        kind: WantKind::Shared,
        ..Want::default()
    };
    assert!(resolve(&w, &v).is_err(), "shared needs entries");
    let w = Want {
        name: "x".into(),
        stop_pid_files: vec!["../p".into()],
        ..Want::default()
    };
    assert!(resolve(&w, &v).is_err(), "a pid file stays in the home");
}

#[test]
fn rule1_a_host_entry_is_never_given() {
    let w = World::new();
    std::fs::write(w.root.join("host.lock"), "host").unwrap();
    let c = w.claim("r1", 10, &[want("host.lock", WantKind::File, &[])]);
    assert!(c.record.is_none());
    assert!(
        c.refused[0].contains("the host or another program"),
        "{c:?}"
    );
    // The release of the run does not touch it.
    w.release("r1");
    assert_eq!(
        std::fs::read_to_string(w.root.join("host.lock")).unwrap(),
        "host"
    );
}

#[test]
fn rule1_a_host_entry_in_a_shared_folder_is_never_given_nor_removed() {
    let w = World::new();
    let dir = w.root.join("codex-daemon-501");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(dir.join("hosthash"), "").unwrap();
    // The run's own entry is free: granted; the folder is not paddock's.
    let c = w.claim("r1", 10, &[shared("runhash")]);
    let rec = c.record.unwrap();
    assert!(rec
        .items
        .iter()
        .any(|i| i.kind == ItemKind::Folder && !i.made));
    assert_eq!(c.drop_args, vec!["--no-daemon".to_string()]);
    std::fs::write(dir.join("runhash"), "").unwrap();
    assert!(w.release("r1").is_empty());
    assert!(!dir.join("runhash").exists(), "the run's entry goes");
    assert!(dir.join("hosthash").exists(), "the host's entry stays");
    assert!(dir.exists(), "the host's folder stays");
    // The host's own name is refused.
    let c = w.claim("r2", 11, &[shared("hosthash")]);
    assert!(c.record.is_none() && c.drop_args.is_empty(), "{c:?}");
}

#[test]
fn a_shared_folder_of_a_wrong_mode_or_a_symlink_is_refused() {
    let w = World::new();
    let dir = w.root.join("codex-daemon-501");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let c = w.claim("r1", 10, &[shared("h")]);
    assert!(c.refused[0].contains("not 700"), "{c:?}");
    assert_eq!(dir.metadata().unwrap().mode() & 0o777, 0o755, "not changed");
    std::fs::remove_dir(&dir).unwrap();
    std::os::unix::fs::symlink(w.root.join("elsewhere"), &dir).unwrap();
    let c = w.claim("r2", 11, &[shared("h")]);
    assert!(c.refused[0].contains("not a folder"), "{c:?}");
}

#[test]
fn rule2_one_fixed_name_goes_to_one_run_and_the_second_is_told() {
    let w = World::new();
    let a = w.claim("ra", 10, &[want("fixed", WantKind::Dir, &[])]);
    assert!(a.record.is_some());
    assert_eq!(
        w.root.join("fixed").metadata().unwrap().mode() & 0o777,
        0o700
    );
    let b = w.claim("rb", 11, &[want("fixed", WantKind::Dir, &[])]);
    assert!(b.record.is_none());
    assert!(b.refused[0].contains("run ra holds it"), "{b:?}");
    // After the end of run a, run b gets it.
    w.end(10);
    let b = w.claim("rb", 11, &[want("fixed", WantKind::Dir, &[])]);
    assert!(b.record.is_some(), "{b:?}");
}

#[test]
fn rule2_two_runs_in_one_shared_folder_get_their_own_entries() {
    let w = World::new();
    let a = w.claim("ra", 10, &[shared("hash-a")]).record.unwrap();
    let b = w.claim("rb", 11, &[shared("hash-b")]).record.unwrap();
    let names = |r: &Record| -> Vec<String> {
        r.items
            .iter()
            .filter(|i| i.kind == ItemKind::Entry)
            .map(|i| i.path.clone())
            .collect()
    };
    assert_eq!(names(&a), vec!["codex-daemon-501/hash-a".to_string()]);
    assert_eq!(names(&b), vec!["codex-daemon-501/hash-b".to_string()]);
    // The rules of each run name its own entry, never the other's.
    let mut pa = crate::seatbelt::Policy::terminal(crate::seatbelt::Net::None, vec![]);
    host::policy(&mut pa, &a);
    let text = crate::seatbelt::render(&pa).unwrap();
    assert!(text.contains("codex-daemon-501/hash-a\""), "{text}");
    assert!(!text.contains("hash-b"), "{text}");
    assert!(
        !text.contains(&format!(
            "(subpath \"{}\")",
            w.root.join("codex-daemon-501").display()
        )),
        "never the whole folder: {text}"
    );
    // The same entry (the same sandbox home) is not shared: the second run is told.
    let c = w.claim("rc", 12, &[shared("hash-a")]);
    assert!(c.refused[0].contains("run ra holds hash-a"), "{c:?}");
    assert!(c.drop_args.is_empty(), "rc keeps --no-daemon");
    // Run a ends: its entry goes, the folder stays for run b.
    std::fs::write(w.root.join("codex-daemon-501/hash-a"), "").unwrap();
    w.end(10);
    w.release("ra");
    assert!(!w.root.join("codex-daemon-501/hash-a").exists());
    assert!(
        w.root.join("codex-daemon-501").is_dir(),
        "run b still uses it"
    );
    // Run b ends: the folder that paddock made goes too.
    w.end(11);
    w.release("rb");
    assert!(!w.root.join("codex-daemon-501").exists());
}

#[test]
fn rule3_the_end_removes_what_the_run_left_and_never_follows_a_symlink() {
    let w = World::new();
    let outside = w.root.parent().unwrap().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), "x").unwrap();
    w.claim(
        "r1",
        10,
        &[
            want("d", WantKind::Dir, &[]),
            want("s", WantKind::Socket, &[]),
        ],
    );
    // The sandbox fills its folder and puts a symlink to the host in it.
    std::fs::write(w.root.join("d/file"), "").unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join("d/link")).unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join("s")).unwrap();
    assert!(w.release("r1").is_empty());
    assert!(!w.root.join("d").exists());
    assert!(std::fs::symlink_metadata(w.root.join("s")).is_err());
    assert!(outside.join("keep").exists(), "nothing through a symlink");
    assert!(load_all(&w.records).is_empty(), "the record goes");
}

#[test]
fn rule3_a_record_of_an_ended_run_is_released_by_the_next_claim() {
    let w = World::new();
    w.claim("old", 10, &[want("d", WantKind::Dir, &[])]);
    w.end(10);
    // No end step ran: the name and the record are left.
    assert!(w.root.join("d").exists());
    assert_eq!(load_all(&w.records).len(), 1);
    let c = w.claim("new", 11, &[want("d", WantKind::Dir, &[])]);
    assert!(c.record.is_some(), "{c:?}");
    let runs: Vec<String> = load_all(&w.records).into_iter().map(|r| r.run).collect();
    assert_eq!(runs, vec!["new".to_string()]);
}

#[test]
fn a_refused_want_keeps_its_flag_and_a_granted_one_drops_it() {
    let w = World::new();
    std::fs::write(w.root.join("busy"), "").unwrap();
    let mut a = want("free", WantKind::Socket, &[]);
    a.replaces_args = vec!["--x".into(), "--y".into()];
    let mut b = want("busy", WantKind::Socket, &[]);
    b.replaces_args = vec!["--y".into()];
    let c = w.claim("r1", 10, &[a, b]);
    assert_eq!(c.drop_args, vec!["--x".to_string()]);
    assert_eq!(c.granted.len(), 1);
    assert_eq!(c.refused.len(), 1);
}

#[test]
fn pid_files_parse_a_number_or_codex_json() {
    assert_eq!(parse_pid("1234\n"), Some(1234));
    assert_eq!(
        parse_pid(r#"{"pid": 77, "processStartTime": "x"}"#),
        Some(77)
    );
    assert_eq!(parse_pid("1"), None, "never launchd");
    assert_eq!(parse_pid("0"), None);
    assert_eq!(parse_pid("nope"), None);
}

#[test]
fn the_codex_profile_claims_its_daemon_folder_on_macos() {
    let p = crate::profile::builtin("codex").unwrap();
    let home = Path::new("/Users/a/.local/share/paddock/homes/codex/x");
    let v = Vars {
        uid: 501,
        home,
        run: "r",
    };
    let [w] = p.macos.tmp.as_slice() else {
        panic!("one entry: {:?}", p.macos.tmp)
    };
    let r = resolve(w, &v).unwrap();
    assert_eq!(r.name, "codex-daemon-501");
    assert_eq!(r.kind, WantKind::Shared);
    // codex: sha256 of the canonical rendezvous path, and `.lock` next to it.
    let sock = format!(
        "{}/.codex/app-server-control/app-server-control.sock",
        home.display()
    );
    let hash = crate::manifest::hex(&Sha256::digest(sock.as_bytes()));
    assert_eq!(r.entries, vec![hash.clone(), format!("{hash}.lock")]);
    assert!(
        "/private/tmp/codex-daemon-501/".len() + hash.len() < 104,
        "fits sun_path"
    );
    assert_eq!(r.replaces_args, vec!["--no-daemon".to_string()]);
    assert_eq!(r.stop[0], home.join(".codex/app-server-daemon/daemon.pid"));
    // Linux keeps the flag: it is still in args_if_supported.
    assert!(p
        .launch
        .args_if_supported
        .contains(&"--no-daemon".to_string()));
}

#[test]
fn the_profile_rules_of_each_kind() {
    let rec = Record {
        run: "r".into(),
        pid: 1,
        root: PathBuf::from("/private/tmp"),
        items: vec![
            Item {
                path: "d".into(),
                kind: ItemKind::Dir,
                made: true,
            },
            Item {
                path: "f".into(),
                kind: ItemKind::File,
                made: true,
            },
            Item {
                path: "c".into(),
                kind: ItemKind::Folder,
                made: true,
            },
            Item {
                path: "c/e".into(),
                kind: ItemKind::Entry,
                made: false,
            },
        ],
        stop: vec![],
        probe_read: PathBuf::new(),
        probe_write: PathBuf::new(),
        home: PathBuf::new(),
    };
    let mut p = crate::seatbelt::Policy::terminal(crate::seatbelt::Net::None, vec![]);
    host::policy(&mut p, &rec);
    let t = crate::seatbelt::render(&p).unwrap();
    for want in [
        "(allow file-read* file-write* (subpath \"/private/tmp/d\"))",
        "(allow file-read* file-write* (literal \"/private/tmp/f\"))",
        "(allow file-write-create (literal \"/private/tmp/c\"))",
        "(allow file-read* file-write* (literal \"/private/tmp/c/e\"))",
        "(allow network-bind (local unix-socket (path-literal \"/private/tmp/c/e\")))",
        "(allow network-outbound (remote unix-socket (path-literal \"/private/tmp/c/e\")))",
    ] {
        assert!(t.contains(want), "{want} in\n{t}");
    }
    assert!(
        !t.contains("(subpath \"/private/tmp\")"),
        "never all of /tmp"
    );
    assert!(
        !t.contains("file-read* file-write* (literal \"/private/tmp/c\")"),
        "the shared folder is never read or changed"
    );
}

const HOME: &str = "/homes/codex/p";

#[test]
fn a_leftover_name_of_the_same_home_is_removed_at_the_next_claim() {
    let w = World::new();
    let dir = w.root.join("codex-daemon-501");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(dir.join("homehash"), "").unwrap();
    std::fs::write(dir.join("homehash.lock"), "").unwrap();
    let mut s = want(
        "codex-daemon-501",
        WantKind::Shared,
        &["homehash", "homehash.lock"],
    );
    s.home_bound = true;
    let c = w.claim("r1", 10, &[s.clone()]);
    assert!(c.record.is_some() && c.refused.is_empty(), "{c:?}");
    assert!(!dir.join("homehash").exists() && !dir.join("homehash.lock").exists());
    // A live run holds it: the second run is refused, nothing is removed.
    std::fs::write(dir.join("homehash"), "live").unwrap();
    let c = w.claim("r2", 11, &[s.clone()]);
    assert!(
        c.record.is_none() && c.refused[0].contains("run r1 holds"),
        "{c:?}"
    );
    assert!(dir.join("homehash").exists());
    // A folder at that name is never removed.
    w.release("r1");
    std::fs::create_dir(dir.join("homehash")).unwrap();
    let c = w.claim("r3", 12, &[s]);
    assert!(c.record.is_none(), "{c:?}");
    assert!(dir.join("homehash").is_dir());
}

#[test]
fn only_names_built_from_the_home_are_home_bound() {
    let p = crate::profile::builtin("codex").unwrap();
    let v = Vars {
        uid: 501,
        home: Path::new(HOME),
        run: "r",
    };
    assert!(resolve(&p.macos.tmp[0], &v).unwrap().home_bound);
    let w = Want {
        name: "f".into(),
        kind: WantKind::Shared,
        entries: vec!["{sha256:{home}}".into(), "fixed".into()],
        ..Want::default()
    };
    assert!(!resolve(&w, &v).unwrap().home_bound);
}

fn home_shared(entry: &str) -> Resolved {
    let mut s = shared(entry);
    s.home_bound = true;
    s
}

/// TMP1-F4, the order of the macOS end test: run a makes the shared folder,
/// its paddock process dies, and its record is lost. Run b of the same home
/// starts: its claim removes what run a left. Run b ends: the folder that
/// paddock made goes.
#[test]
fn a_folder_that_a_lost_run_made_goes_at_the_end_of_the_next_run() {
    let w = World::new();
    let dir = w.root.join("codex-daemon-501");
    w.claim("ra", 10, &[home_shared("homehash")])
        .record
        .unwrap();
    std::fs::write(dir.join("homehash"), "socket of a").unwrap();
    w.end(10);
    std::fs::remove_dir_all(&w.records).unwrap();
    let b = w.claim("rb", 11, &[home_shared("homehash")]);
    let rec = b.record.unwrap();
    assert!(
        rec.items
            .iter()
            .any(|i| i.kind == ItemKind::Folder && i.made),
        "{rec:?}"
    );
    assert!(!dir.join("homehash").exists(), "what run a left goes");
    std::fs::write(dir.join("homehash"), "socket of b").unwrap();
    w.end(11);
    assert!(w.release("rb").is_empty());
    assert!(!dir.exists(), "paddock made the folder, so it goes");
    assert!(
        std::fs::read_dir(made_dir(&w.records))
            .unwrap()
            .next()
            .is_none(),
        "the mark goes with the folder"
    );
}

/// TMP1-F4: run a makes the folder and dies while run b uses it. The
/// release of run a keeps the folder for run b and removes the record of
/// run a. A run c that starts after it still knows that paddock made the
/// folder, so the end of the last run removes it.
#[test]
fn the_made_fact_survives_the_release_of_the_run_that_made_the_folder() {
    let w = World::new();
    let dir = w.root.join("codex-daemon-501");
    w.claim("ra", 10, &[shared("hash-a")]);
    w.claim("rb", 11, &[shared("hash-b")]);
    w.end(10);
    assert!(w.release_dead().is_empty());
    assert!(dir.is_dir(), "run b still uses it");
    let c = w.claim("rc", 12, &[shared("hash-c")]).record.unwrap();
    assert!(c.items.iter().any(|i| i.kind == ItemKind::Folder && i.made));
    w.end(11);
    w.release("rb");
    assert!(dir.is_dir(), "run c still uses it");
    w.end(12);
    w.release("rc");
    assert!(!dir.exists(), "paddock made the folder, so it goes");
}

/// TMP1-F4: a mark of an earlier folder never makes a folder of the host
/// paddock's, though it has the same name.
#[test]
fn a_mark_of_an_earlier_folder_does_not_take_a_host_folder() {
    let w = World::new();
    let dir = w.root.join("codex-daemon-501");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let m = dir.metadata().unwrap();
    // The mark of an earlier folder of this name: another inode.
    mark_made(&w.records, "codex-daemon-501", (m.dev(), m.ino() + 1));
    let mark = made_file(&w.records, "codex-daemon-501");
    assert!(mark.exists());
    let b = w.claim("rb", 11, &[shared("hash-b")]).record.unwrap();
    assert!(
        b.items
            .iter()
            .any(|i| i.kind == ItemKind::Folder && !i.made),
        "{b:?}"
    );
    assert!(!mark.exists(), "the old mark goes");
    w.end(11);
    w.release("rb");
    assert!(dir.is_dir(), "the host's folder stays");
}

#[path = "tmpclaim_stop_tests.rs"]
mod stops;
