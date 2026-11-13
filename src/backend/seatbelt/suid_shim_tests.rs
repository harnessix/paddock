use super::*;
use std::fs;

/// A test folder with a fake `ps` (prints the PID after `-p`), a fake `cp`
/// and a fake `codesign`. Each fake command appends a line to `calls`.
struct Fakes {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    calls: PathBuf,
    tools: Tools,
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `codesign_body` is the body of the fake codesign: it gets `-s - -f FILE`.
fn fakes(codesign_body: &str) -> Fakes {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let calls = tmp.path().join("calls");
    let source = bin.join("ps");
    script(&source, "echo \"$4\"");
    // The source is setuid on the Mac: a copy never keeps that bit.
    let cp = bin.join("cp");
    script(
        &cp,
        &format!(
            "echo \"cp $*\" >> '{c}'\ncat \"$1\" > \"$2\"",
            c = calls.display()
        ),
    );
    let codesign = bin.join("codesign");
    script(
        &codesign,
        &format!(
            "echo \"codesign $*\" >> '{c}'\n[ \"$1 $2 $3\" = '-s - -f' ] || exit 2\n{codesign_body}",
            c = calls.display()
        ),
    );
    Fakes {
        root: tmp.path().join("state/paddock/seatbelt-shims"),
        source,
        calls,
        tools: Tools { cp, codesign },
        _tmp: tmp,
    }
}

fn calls(f: &Fakes) -> Vec<String> {
    fs::read_to_string(&f.calls)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn entries(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn the_key_names_the_inode_size_and_mtime() {
    let f = fakes("exit 0");
    let m = fs::metadata(&f.source).unwrap();
    let k = key(&m);
    assert_eq!(
        k,
        format!(
            "ps-{:x}-{:x}-{:x}.{:x}",
            m.ino(),
            m.size(),
            m.mtime(),
            m.mtime_nsec()
        )
    );
    // The same file gives the same key.
    assert_eq!(key(&fs::metadata(&f.source).unwrap()), k);
    // A new build (new content, new file) gives a new key.
    let other = f.source.with_file_name("ps2");
    fs::write(&other, "#!/bin/sh\necho \"$4\" # a newer ps\n").unwrap();
    fs::rename(&other, &f.source).unwrap();
    assert_ne!(key(&fs::metadata(&f.source).unwrap()), k);
}

#[test]
fn ensure_copies_chmods_and_signs_once() {
    let f = fakes("exit 0");
    assert_eq!(current(&f.root, &f.source), None);
    let dir = ensure(&f.root, &f.source, &f.tools).unwrap();
    let key = key(&fs::metadata(&f.source).unwrap());
    assert_eq!(dir, f.root.join(&key));
    let copy = dir.join(NAME);
    let mode = fs::metadata(&copy).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o755);
    assert_eq!(fs::read(&copy).unwrap(), fs::read(&f.source).unwrap());
    let c = calls(&f);
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(
        c[0].starts_with(&format!("cp {} ", f.source.display())),
        "{c:?}"
    );
    // The copy is signed in the temporary folder, before the rename.
    assert!(c[1].starts_with("codesign -s - -f "), "{c:?}");
    assert!(!c[1].contains(&format!("/{key}/")), "{c:?}");
    // No temporary folder is left, and only the key folder is there.
    assert_eq!(entries(&f.root), vec![key.clone()]);
    assert_eq!(current(&f.root, &f.source), Some(dir.clone()));
    // A second start uses the copy and runs nothing.
    assert_eq!(ensure(&f.root, &f.source, &f.tools).unwrap(), dir);
    assert_eq!(calls(&f).len(), 2);
}

#[test]
fn a_new_build_of_ps_makes_a_new_copy_and_drops_the_old() {
    let f = fakes("exit 0");
    let old = ensure(&f.root, &f.source, &f.tools).unwrap();
    let next = f.source.with_file_name("ps.new");
    script(&next, "echo \"$4\" # after a macOS update");
    fs::rename(&next, &f.source).unwrap();
    assert_eq!(current(&f.root, &f.source), None);
    let new = ensure(&f.root, &f.source, &f.tools).unwrap();
    assert_ne!(new, old);
    assert!(!old.exists());
    assert_eq!(
        entries(&f.root),
        vec![new.file_name().unwrap().to_string_lossy().into_owned()]
    );
}

#[test]
fn a_failed_codesign_leaves_nothing_and_says_why() {
    let f = fakes("echo 'no identity' >&2; exit 1");
    let e = ensure(&f.root, &f.source, &f.tools).unwrap_err();
    let msg = format!("{e:#}");
    assert!(msg.contains("codesign failed"), "{msg}");
    assert!(msg.contains("no identity"), "{msg}");
    assert!(entries(&f.root).is_empty(), "{:?}", entries(&f.root));
    assert_eq!(current(&f.root, &f.source), None);
    let line = status_line(&Err(e));
    assert!(
        line.starts_with("  no       ps for the sandbox: "),
        "{line}"
    );
    assert!(line.contains("codex stops"), "{line}");
}

#[test]
fn a_copy_that_does_not_run_is_not_used() {
    // The fake codesign breaks the copy, as a broken signature does.
    let f = fakes("printf '#!/bin/sh\\nkill -9 $$\\n' > \"$4\"");
    let e = ensure(&f.root, &f.source, &f.tools).unwrap_err();
    assert!(format!("{e:#}").contains("does not work"), "{e:#}");
    assert!(entries(&f.root).is_empty());
}

#[test]
fn a_missing_codesign_is_an_error_not_a_panic() {
    let mut f = fakes("exit 0");
    f.tools.codesign = PathBuf::from("/nonexistent/codesign");
    let e = ensure(&f.root, &f.source, &f.tools).unwrap_err();
    assert!(format!("{e:#}").contains("cannot start codesign"), "{e:#}");
    assert!(entries(&f.root).is_empty());
}

#[test]
fn two_starts_at_once_get_the_same_copy() {
    let f = fakes("sleep 0.2");
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| ensure(&f.root, &f.source, &f.tools));
        let b = s.spawn(|| ensure(&f.root, &f.source, &f.tools));
        (a.join().unwrap().unwrap(), b.join().unwrap().unwrap())
    });
    assert_eq!(a, b);
    let key = key(&fs::metadata(&f.source).unwrap());
    assert_eq!(entries(&f.root), vec![key]);
    assert!(crate::env::is_executable(&a.join(NAME)));
}

#[test]
fn an_old_temporary_folder_of_a_crash_goes_away() {
    let f = fakes("exit 0");
    fs::create_dir_all(&f.root).unwrap();
    let crashed = f.root.join(".ps-1-2-3.0.99.1");
    fs::create_dir(&crashed).unwrap();
    let two_days = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600);
    fs::File::open(&crashed)
        .unwrap()
        .set_modified(two_days)
        .unwrap();
    // A fresh one may belong to a start that runs now: it stays.
    let running = f.root.join(".ps-1-2-3.0.98.1");
    fs::create_dir(&running).unwrap();
    ensure(&f.root, &f.source, &f.tools).unwrap();
    assert!(!crashed.exists());
    assert!(running.exists());
}

#[test]
fn the_copy_comes_first_on_path_once() {
    let dir = Path::new("/Users/u/.local/state/paddock/seatbelt-shims/ps-1-2-3.0");
    let d = dir.display().to_string();
    let path = format!("/run/paddock/id1:/usr/bin:/bin:{d}::/usr/sbin");
    assert_eq!(
        path_first(&path, dir),
        format!("{d}:/run/paddock/id1:/usr/bin:/bin:/usr/sbin")
    );
    assert_eq!(path_first("", dir), d);
    // Before /bin, where the setuid ps is.
    let out = path_first("/bin:/usr/bin", dir);
    let parts: Vec<&str> = out.split(':').collect();
    let shim = parts.iter().position(|p| *p == d).unwrap();
    let bin = parts.iter().position(|p| *p == "/bin").unwrap();
    assert!(shim < bin, "{out}");
}

#[test]
fn the_profile_allows_the_folder_read_only_and_nothing_else() {
    let dir = Path::new("/Users/u/.local/state/paddock/seatbelt-shims/ps-1-2-3.0");
    let mut empty = Policy::terminal(crate::seatbelt::Net::None, Vec::new());
    let before = crate::seatbelt::render(&empty).unwrap();
    allow(&mut empty, dir);
    let after = crate::seatbelt::render(&empty).unwrap();
    let q = format!("(subpath \"{}\")", dir.display());
    assert!(
        after.contains(&format!("(allow file-read* {q})")),
        "{after}"
    );
    assert!(
        after.contains(&format!("(deny file-write* {q})")),
        "{after}"
    );
    let added: Vec<&str> = after
        .lines()
        .filter(|l| !before.lines().any(|b| b == *l))
        .collect();
    assert_eq!(added.len(), 3, "a comment and two rules: {added:?}");
}

#[test]
fn the_ready_line_names_the_copy() {
    let dir = PathBuf::from("/s/seatbelt-shims/ps-1");
    assert_eq!(
        status_line(&Ok(dir)),
        "  ok       ps for the sandbox: /s/seatbelt-shims/ps-1/ps"
    );
}

#[test]
fn the_copy_is_in_paddocks_state_folder() {
    let home = PathBuf::from("/Users/u");
    let env = HostEnv::from_parts(home, "u".into(), 501, 20, Default::default());
    assert!(root(&env).starts_with(env.paddock_state()));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_a_run_gets_no_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().to_path_buf(),
        "u".into(),
        501,
        20,
        Default::default(),
    );
    assert_eq!(for_run(&env), None);
    prepare(&env);
    assert!(!root(&env).exists());
}
