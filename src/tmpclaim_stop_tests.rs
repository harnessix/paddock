//! The end of a run and the processes of its server: the end step stops
//! every process of the run and no other, SIGKILL follows a SIGTERM that a
//! process ignores, the next command retries what still runs, a start stops
//! a stale server of its home, and the probe finds the server after the run
//! folder is gone. The fake processes and the world are in tmpclaim_tests.rs.

use super::*;

#[test]
fn the_end_stops_every_process_of_the_run_and_no_other() {
    let w = World::new();
    w.claim("r1", 10, &[shared("h1")]);
    w.started("r1", HOME);
    // The server left the process group; it and its helper under node have
    // no pid file. A host codex and a process of another run look the same
    // by name, but they do not run in this sandbox.
    w.procs.add(100, "r1", HOME);
    w.procs.add(101, "r1", HOME);
    w.procs.add(200, "", "");
    w.procs.add(300, "r2", "/homes/codex/q");
    std::fs::write(w.root.join("codex-daemon-501/h1"), "").unwrap();
    assert!(w.release("r1").is_empty());
    assert!(!w.procs.runs(100) && !w.procs.runs(101));
    assert!(w.procs.runs(200), "a host process is never signaled");
    assert!(
        w.procs.runs(300),
        "a process of another run is never signaled"
    );
    assert!(w
        .procs
        .signals
        .borrow()
        .iter()
        .all(|(p, _)| *p == 100 || *p == 101));
    assert!(!w.root.join("codex-daemon-501/h1").exists());
    assert!(load_all(&w.records).is_empty());
}

#[test]
fn a_process_that_ignores_sigterm_gets_sigkill() {
    let w = World::new();
    w.claim("r1", 10, &[shared("h1")]);
    w.started("r1", HOME);
    w.procs.add(100, "r1", HOME);
    w.procs.set(100, |p| p.stubborn = true);
    assert!(w.release("r1").is_empty());
    assert!(!w.procs.runs(100));
    assert_eq!(*w.procs.signals.borrow(), vec![(100, false), (100, true)]);
}

#[test]
fn names_of_a_server_that_still_runs_stay_and_the_next_command_retries() {
    let w = World::new();
    w.claim("r1", 10, &[shared("h1")]);
    w.started("r1", HOME);
    std::fs::write(w.root.join("codex-daemon-501/h1"), "").unwrap();
    w.procs.add(100, "r1", HOME);
    w.procs.set(100, |p| {
        p.stubborn = true;
        p.stuck = true;
    });
    let problems = w.release("r1");
    assert!(problems[0].contains("still run"), "{problems:?}");
    // The socket of a live server stays: a new start must not find a server
    // without its socket.
    assert!(w.root.join("codex-daemon-501/h1").exists());
    assert_eq!(load_all(&w.records).len(), 1, "the record stays");
    // paddock ended. The next command, once the process is gone, cleans up.
    w.end(10);
    w.procs.set(100, |p| p.stuck = false);
    assert!(w.release_dead().is_empty());
    assert!(!w.procs.runs(100));
    assert!(!w.root.join("codex-daemon-501/h1").exists());
    assert!(load_all(&w.records).is_empty());
}

#[test]
fn the_next_command_releases_only_the_records_of_dead_runs() {
    let w = World::new();
    w.claim("dead", 10, &[shared("h1")]);
    w.started("dead", HOME);
    w.claim("live", 11, &[shared("h2")]);
    w.started("live", "/homes/codex/q");
    w.procs.add(100, "dead", HOME);
    w.procs.add(101, "live", "/homes/codex/q");
    // Nothing is dead: nothing happens, not even a lock.
    assert!(w.release_dead().is_empty());
    assert!(w.procs.runs(100));
    // paddock of run "dead" got kill -9.
    w.end(10);
    assert!(w.release_dead().is_empty());
    assert!(!w.procs.runs(100), "the server of the dead run stops");
    assert!(w.procs.runs(101), "the live run keeps its server");
    let runs: Vec<String> = load_all(&w.records).into_iter().map(|r| r.run).collect();
    assert_eq!(runs, vec!["live".to_string()]);
}

#[test]
fn a_record_without_probe_paths_stops_nothing() {
    let w = World::new();
    w.claim("r1", 10, &[shared("h1")]);
    // The start did not get to write the probe paths.
    w.procs.add(100, "r1", HOME);
    assert!(w.release("r1").is_empty());
    assert!(w.procs.runs(100));
    assert!(w.procs.signals.borrow().is_empty());
}

#[test]
fn a_start_stops_a_stale_server_of_its_home_and_nothing_else() {
    let w = World::new();
    // An earlier run of HOME lost its record; its server and helper still run.
    w.procs.add(100, "old", HOME);
    w.procs.add(101, "old", HOME);
    // A live run of the same home without a record, a run of another home,
    // and a host process.
    w.procs.add(200, "busy", HOME);
    w.procs.add(300, "other", "/homes/codex/q");
    w.procs.add(400, "", HOME);
    // A live run with a record in the same home.
    w.claim("rec", 12, &[want("d", WantKind::Dir, &[])]);
    w.started("rec", HOME);
    w.procs.add(500, "rec", HOME);
    let stopped = w.stop_stale(HOME, &[PathBuf::from("/run/busy")]);
    assert_eq!(stopped, vec![100, 101]);
    for pid in [200, 300, 400, 500] {
        assert!(w.procs.runs(pid), "{pid} keeps running");
    }
}

/// TMP1-F5, the path of the Mac: the terminal of the run closes and its
/// paddock process dies without the end step. Another paddock process lists
/// the runs and removes the run folder. The next command still finds the
/// server by the probe, which lives until the release, and the probe goes
/// with the record.
#[test]
fn the_probe_lives_until_the_release_and_finds_the_server_without_the_run_folder() {
    let w = World::new();
    w.claim("r1", 10, &[shared("h1")]);
    w.started("r1", HOME);
    let rec = load_all(&w.records).pop().unwrap();
    assert_eq!(
        rec.probe_read,
        std::fs::canonicalize(probe_file(&w.records, "r1")).unwrap()
    );
    assert!(
        !rec.probe_read.starts_with("/run"),
        "never the run folder, which other code removes"
    );
    std::fs::write(w.root.join("codex-daemon-501/h1"), "").unwrap();
    w.procs.add(100, "r1", HOME);
    w.procs.add(101, "r1", HOME);
    w.procs.set(100, |p| {
        p.stubborn = true;
        p.stuck = true;
    });
    w.end(10);
    // The server still runs: the record and the probe stay.
    assert!(!w.release_dead().is_empty());
    assert!(
        rec.probe_read.exists(),
        "the probe stays while the record stays"
    );
    assert_eq!(load_all(&w.records).len(), 1);
    w.procs.set(100, |p| p.stuck = false);
    assert!(w.release_dead().is_empty());
    assert!(
        !w.procs.runs(100) && !w.procs.runs(101),
        "the server and its helper stop"
    );
    assert!(!w.root.join("codex-daemon-501/h1").exists());
    assert!(load_all(&w.records).is_empty());
    assert!(!rec.probe_read.exists(), "the probe goes with the record");
}

#[test]
fn the_profile_lets_the_run_read_its_probe_only() {
    let t = tempfile::tempdir().unwrap();
    let probe = make_probe(t.path(), "r").unwrap();
    let rec = Record {
        run: "r".into(),
        pid: 1,
        root: PathBuf::from("/private/tmp"),
        items: vec![],
        stop: vec![],
        probe_read: probe.clone(),
        probe_write: PathBuf::from("/x/tmp"),
        home: PathBuf::new(),
    };
    let mut p = crate::seatbelt::Policy::terminal(crate::seatbelt::Net::None, vec![]);
    host::policy(&mut p, &rec);
    let text = crate::seatbelt::render(&p).unwrap();
    let lit = format!("(literal \"{}\")", probe.display());
    let line = text
        .lines()
        .find(|l| l.contains(&lit))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        line.contains("file-read") && !line.contains("file-write"),
        "{line}"
    );
    assert!(
        !text.contains(&format!("(subpath \"{}\")", t.path().display())),
        "never the records folder: {text}"
    );
}
