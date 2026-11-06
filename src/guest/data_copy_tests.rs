use super::*;
use std::sync::Mutex;

fn dir(name: &str) -> Entry {
    Entry {
        name: name.into(),
        kind: Kind::Dir,
        mode: 0o40700,
        target: None,
    }
}

fn file(name: &str) -> Entry {
    Entry {
        name: name.into(),
        kind: Kind::File,
        mode: 0o100644,
        target: None,
    }
}

fn link(name: &str, to: &str) -> Entry {
    Entry {
        name: name.into(),
        kind: Kind::Link,
        mode: 0o120755,
        target: Some(to.into()),
    }
}

/// A fresh home: the TCC folders, a Downloads link to the share, Library.
fn fresh_home() -> (Vec<Entry>, Vec<Entry>) {
    (
        vec![
            file(".zshrc"),
            dir("Desktop"),
            dir("Documents"),
            link("Downloads", "/Volumes/My Shared Files/downloads"),
            dir("Library"),
            dir("Music"),
            link("notes", "Documents/notes.txt"),
        ],
        vec![
            dir("Application Support"),
            dir("Containers"),
            dir("Group Containers"),
            dir("Mail"),
            dir("Preferences"),
        ],
    )
}

fn the_plan() -> (Vec<Op>, Vec<PathBuf>) {
    let (home, lib) = fresh_home();
    plan(
        Path::new("/Users/paddock"),
        Path::new("/Volumes/paddock/home.partial"),
        0o40750,
        &home,
        Some((0o40700, &lib)),
    )
}

fn copy_of(ops: &[Op], name: &str) -> Option<(bool, PathBuf)> {
    ops.iter().find_map(|o| match o {
        Op::Copy {
            src, dst, guarded, ..
        } if src.ends_with(name) => Some((*guarded, dst.clone())),
        _ => None,
    })
}

#[test]
fn the_plan_splits_the_library_guards_the_tcc_folders_and_keeps_links() {
    let (ops, measure) = the_plan();
    // The two folders first: the copy and its Library, with their modes.
    assert_eq!(
        ops[0],
        Op::Dir {
            dst: "/Volumes/paddock/home.partial".into(),
            mode: 0o40750
        }
    );
    assert!(ops.contains(&Op::Dir {
        dst: "/Volumes/paddock/home.partial/Library".into(),
        mode: 0o40700
    }));
    // No ditto of the whole home or of the whole Library.
    assert!(copy_of(&ops, "paddock").is_none());
    assert!(copy_of(&ops, "Library").is_none());
    // The TCC folders and the app data are guarded; the rest is not.
    for g in [
        "Desktop",
        "Documents",
        "Containers",
        "Group Containers",
        "Mail",
    ] {
        assert_eq!(copy_of(&ops, g).map(|c| c.0), Some(true), "{g}");
    }
    for u in [".zshrc", "Music", "Application Support", "Preferences"] {
        assert_eq!(copy_of(&ops, u).map(|c| c.0), Some(false), "{u}");
    }
    assert_eq!(
        copy_of(&ops, "Preferences").unwrap().1,
        PathBuf::from("/Volumes/paddock/home.partial/Library/Preferences")
    );
    // The Downloads link to the share is skipped; another link is made
    // again, never followed.
    assert!(!ops.iter().any(|o| match o {
        Op::Copy { src, .. } => src.ends_with("Downloads"),
        Op::Link { dst, .. } => dst.ends_with("Downloads"),
        _ => false,
    }));
    assert!(ops.contains(&Op::Link {
        dst: "/Volumes/paddock/home.partial/notes".into(),
        target: "Documents/notes.txt".into()
    }));
    // du measures only what is not guarded: a du of a TCC folder could
    // wait on the prompt too.
    assert_eq!(measure.len(), 4);
    assert!(measure
        .iter()
        .all(|m| !m.ends_with("Desktop") && !m.ends_with("Mail")));
}

#[test]
fn a_home_without_a_readable_library_copies_it_whole() {
    let (home, _) = fresh_home();
    let (ops, _) = plan(
        Path::new("/Users/paddock"),
        Path::new("/x"),
        0o40750,
        &home,
        None,
    );
    assert_eq!(copy_of(&ops, "Library").map(|c| c.0), Some(false));
}

#[test]
fn du_sizes_reads_the_size_lines_and_skips_the_errors() {
    let out = "du: /Users/paddock/Library/Mail: Operation not permitted\n\
               1024\t/Users/paddock/Music\n\
               52\t/Users/paddock/Library/Application Support\n";
    let s = du_sizes(out);
    assert_eq!(s.len(), 2);
    assert_eq!(
        s[Path::new("/Users/paddock/Library/Application Support")],
        52
    );
}

#[test]
fn the_copy_limit_grows_with_the_size() {
    assert_eq!(copy_limit(Some(0)), Duration::from_secs(60));
    // 1 s per 50 MB: 5 GB gives 100 s more.
    assert_eq!(copy_limit(Some(5_000_000_000)), Duration::from_secs(160));
    assert_eq!(copy_limit(Some(u64::MAX)), MAX_LIMIT);
    assert_eq!(copy_limit(None), UNKNOWN_LIMIT);
    assert!(UNKNOWN_LIMIT > crate::guest::configure::STEP_TIMEOUT);
}

#[test]
fn ditto_errors_counts_the_unreadable_files() {
    let out = "ditto: /Users/paddock/Library/Mail/V10: Operation not permitted\n\
               ditto: /Users/paddock/Documents/a: Permission denied\n\
               \n";
    assert_eq!(ditto_errors(out), (2, vec![]));
    let out = "ditto: /Volumes/paddock/home.partial/x: No space left on device";
    assert_eq!(ditto_errors(out).0, 0);
    assert_eq!(ditto_errors(out).1.len(), 1);
}

/// A runner that answers each ditto from `answer` by the source name and
/// records every argv without the sudo prefix.
struct Runner {
    seen: Vec<(Vec<String>, Duration)>,
    answer: Box<dyn Fn(&str) -> Ran>,
}

impl Runner {
    fn new(answer: impl Fn(&str) -> Ran + 'static) -> Self {
        Runner {
            seen: Vec::new(),
            answer: Box::new(answer),
        }
    }

    fn run(&mut self, p: &Plan, limit: Duration) -> Result<Ran, Fail> {
        assert_eq!(p.argv[..5], ["/usr/bin/sudo", "-S", "-p", "", "--"]);
        assert!(p.password_on_stdin);
        let argv = p.argv[5..].to_vec();
        self.seen.push((argv.clone(), limit));
        if argv[0] == "/usr/bin/ditto" {
            return Ok((self.answer)(&argv[1]));
        }
        Ok(ok())
    }
}

fn ok() -> Ran {
    Ran {
        code: Some(0),
        timed_out: false,
        output: String::new(),
    }
}

fn hung() -> Ran {
    Ran {
        code: None,
        timed_out: true,
        output: String::new(),
    }
}

/// The progress lines that a watch got.
type Lines = Arc<Mutex<Vec<(String, Duration)>>>;

fn progress_watch() -> (Watch, Lines) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l = lines.clone();
    let w = Watch {
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Box::new(move |t, d| l.lock().unwrap().push((t, d))),
    };
    (w, lines)
}

#[test]
fn a_guarded_folder_that_hangs_is_left_empty_and_the_copy_goes_on() {
    let (ops, _) = the_plan();
    let mut r = Runner::new(|src| {
        if src.ends_with("Containers") && !src.ends_with("Group Containers") {
            hung()
        } else if src.ends_with("Mail") {
            Ran {
                code: Some(1),
                timed_out: false,
                output: "ditto: /Users/paddock/Library/Mail/V10: Operation not permitted\n\
                         ditto: /Users/paddock/Library/Mail/x: Operation not permitted"
                    .into(),
            }
        } else {
            ok()
        }
    });
    let sizes: BTreeMap<PathBuf, u64> = [(PathBuf::from("/Users/paddock/Music"), 3 << 30)].into();
    let (watch, lines) = progress_watch();
    let report = execute(&ops, &sizes, Duration::from_secs(600), &watch, |p, l| {
        r.run(p, l)
    })
    .unwrap();
    assert_eq!(report.denied, 2);
    assert_eq!(
        report.skipped,
        vec![PathBuf::from("/Users/paddock/Library/Containers")]
    );
    assert_eq!(report.parts, 9);
    // The guarded parts got the short limit, the others the rest of the
    // whole limit.
    let limit_of = |name: &str| {
        r.seen
            .iter()
            .find(|(a, _)| a[0] == "/usr/bin/ditto" && a[1].ends_with(name))
            .map(|(_, l)| *l)
            .unwrap()
    };
    assert_eq!(limit_of("Desktop"), GUARD_LIMIT);
    assert!(limit_of("Music") > GUARD_LIMIT);
    // The half copy went, and an empty folder with the mode took its
    // place.
    let after: Vec<&Vec<String>> = r
        .seen
        .iter()
        .map(|(a, _)| a)
        .skip_while(|a| !(a[0] == "/usr/bin/ditto" && a[1].ends_with("Library/Containers")))
        .collect();
    let dst = "/Volumes/paddock/home.partial/Library/Containers";
    assert_eq!(*after[1], ["/bin/rm", "-rf", dst]);
    assert_eq!(*after[2], ["/bin/mkdir", "-p", dst]);
    assert_eq!(*after[3], ["/bin/chmod", "700", dst]);
    // The links: one made again, the share link not at all.
    assert!(r.seen.iter().any(|(a, _)| a[0] == "/bin/ln"
        && a[2] == "Documents/notes.txt"
        && a[3].ends_with("home.partial/notes")));
    // The progress came at least at the start and at the end, with the
    // bytes of the measured parts.
    let lines = lines.lock().unwrap();
    assert!(!lines.is_empty());
    let last = &lines.last().unwrap().0;
    assert!(last.contains("9 of 9"), "{last}");
    assert!(last.contains("3 GiB of 3 GiB"), "{last}");
}

#[test]
fn a_part_that_is_not_guarded_and_hangs_fails_the_copy_with_the_limit() {
    let (ops, _) = the_plan();
    let mut r = Runner::new(|src| if src.ends_with("Music") { hung() } else { ok() });
    let e = execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(75),
        &Watch::quiet(),
        |p, l| r.run(p, l),
    )
    .unwrap_err();
    assert_eq!(e.0, ErrorCode::Timeout);
    assert!(e.1.contains("75 s"), "{}", e.1);
}

#[test]
fn another_ditto_error_fails_the_copy_and_names_the_part() {
    let (ops, _) = the_plan();
    let mut r = Runner::new(|src| {
        if src.ends_with("Preferences") {
            Ran {
                code: Some(1),
                timed_out: false,
                output: "ditto: /Volumes/paddock/home.partial/Library/Preferences/a: No space left on device".into(),
            }
        } else {
            ok()
        }
    });
    let e = execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(600),
        &Watch::quiet(),
        |p, l| r.run(p, l),
    )
    .unwrap_err();
    assert_eq!(e.0, ErrorCode::Io);
    assert!(
        e.1.contains("Library/Preferences") && e.1.contains("No space"),
        "{}",
        e.1
    );
    // A failed exit without any line fails too.
    let mut r = Runner::new(|_| Ran {
        code: Some(1),
        timed_out: false,
        output: String::new(),
    });
    assert!(execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(600),
        &Watch::quiet(),
        |p, l| r.run(p, l)
    )
    .is_err());
}

#[test]
fn a_cancel_stops_before_the_next_part() {
    let (ops, _) = the_plan();
    let watch = Watch::quiet();
    let cancel = watch.cancel.clone();
    let mut copies = 0;
    let e = execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(600),
        &watch,
        |p, _| {
            if p.argv[5] == "/usr/bin/ditto" {
                copies += 1;
                if copies == 2 {
                    cancel.store(true, Ordering::SeqCst);
                    return Ok(hung());
                }
            }
            Ok(ok())
        },
    )
    .unwrap_err();
    assert_eq!(copies, 2);
    assert!(e.1.contains("quit"), "{}", e.1);
}

#[test]
fn read_entries_never_follows_a_link() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("Library")).unwrap();
    std::fs::set_permissions(
        t.path().join("Library"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(t.path().join(".zshrc"), "x").unwrap();
    std::os::unix::fs::symlink("/nowhere/downloads", t.path().join("Downloads")).unwrap();
    let e = read_entries(t.path()).unwrap();
    assert_eq!(
        e.iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect::<Vec<_>>(),
        [
            (".zshrc", Kind::File),
            ("Downloads", Kind::Link),
            ("Library", Kind::Dir)
        ]
    );
    assert_eq!(
        e[1].target.as_deref(),
        Some(Path::new("/nowhere/downloads"))
    );
    assert_eq!(e[2].mode & 0o777, 0o700);
}

fn interrupted() -> Ran {
    Ran {
        code: Some(1),
        timed_out: false,
        output: "ditto: /Users/paddock/Library/Containers/./x/Data: Interrupted system call".into(),
    }
}

#[test]
fn an_interrupted_part_is_copied_again() {
    assert!(transient(&interrupted().output));
    assert!(!transient("ditto: /a: No space left on device"));
    assert!(!transient(
        "ditto: /a: Interrupted system call\nditto: /b: No space left on device"
    ));
    // Interrupted twice, then whole: the copy goes on.
    let (ops, _) = the_plan();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut r = Runner::new(move |src| {
        if src.ends_with("Music") && calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
            interrupted()
        } else {
            ok()
        }
    });
    let report = execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(600),
        &Watch::quiet(),
        |p, l| r.run(p, l),
    )
    .unwrap();
    assert_eq!(report.parts, 9);
    let music = r
        .seen
        .iter()
        .filter(|(a, _)| a[0] == "/usr/bin/ditto" && a[1].ends_with("Music"))
        .count();
    assert_eq!(music, 3);
    // Interrupted every time: after COPY_TRIES the copy fails.
    let (ops, _) = the_plan();
    let mut r = Runner::new(|src| {
        if src.ends_with("Music") {
            interrupted()
        } else {
            ok()
        }
    });
    let e = execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(600),
        &Watch::quiet(),
        |p, l| r.run(p, l),
    )
    .unwrap_err();
    assert!(e.1.contains("Interrupted system call"), "{}", e.1);
}
