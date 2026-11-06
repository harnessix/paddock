use super::*;
use std::fs;

const T: u64 = 1_700_000_000;

/// Two files of one name, a text, an image, a drop over 2 GiB refused
/// with the badge, and a sweep that keeps the folder of an open file.
#[test]
fn collision_text_image_cap_sweep() {
    let t = tempfile::tempdir().unwrap();
    let p = Profile::new(t.path().join("profile"));
    let a = t.path().join("one");
    let b = t.path().join("two");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(b.join("sub")).unwrap();
    fs::write(a.join("notes.txt"), "a").unwrap();
    fs::write(b.join("notes.txt"), "bb").unwrap();
    fs::write(b.join("sub/deep.bin"), [0u8; 10]).unwrap();
    let items = [
        Item::File(a.join("notes.txt")),
        Item::File(b.join("notes.txt")),
        Item::Text("hello".into()),
        Item::Text("again".into()),
        Item::Image(vec![0x89, b'P', b'N', b'G']),
        Item::File(b.join("sub")),
    ];
    let s = stage(&p, &items, T).unwrap();
    assert_eq!(s.dir, p.drop_dir().join("1700000000-1"));
    assert_eq!(s.bytes, 1 + 2 + 5 + 5 + 4 + 10);
    let names: Vec<String> = s
        .guest_paths
        .iter()
        .map(|g| g.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "notes.txt",
            "notes (2).txt",
            "dropped.txt",
            "dropped (2).txt",
            "dropped.png",
            "sub"
        ]
    );
    for g in &s.guest_paths {
        assert!(
            g.starts_with("/Volumes/My Shared Files/drop/1700000000-1"),
            "{g:?}"
        );
    }
    assert_eq!(
        fs::read_to_string(s.dir.join("notes (2).txt")).unwrap(),
        "bb"
    );
    assert_eq!(
        fs::read_to_string(s.dir.join("dropped (2).txt")).unwrap(),
        "again"
    );
    assert_eq!(fs::read(s.dir.join("sub/deep.bin")).unwrap().len(), 10);
    // The originals are untouched, and the copies do not share a change.
    fs::write(s.dir.join("notes.txt"), "changed").unwrap();
    assert_eq!(fs::read_to_string(a.join("notes.txt")).unwrap(), "a");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&s.dir).unwrap().permissions().mode() & 0o777,
        0o700
    );

    // A second drop in the same second is `-2`.
    let s2 = stage(&p, &[Item::Text("x".into())], T).unwrap();
    assert_eq!(s2.dir, p.drop_dir().join("1700000000-2"));
    // A dotfile keeps its name whole.
    assert_eq!(unique(&s.dir, ".env"), ".env");
    fs::write(s.dir.join(".env"), "").unwrap();
    assert_eq!(unique(&s.dir, ".env"), ".env (2)");
    assert_eq!(unique(&s.dir, "notes.txt"), "notes (3).txt");

    // The cap: a sparse file of 2 GiB plus one byte is refused, nothing
    // is staged, and the badge names the size.
    let big = t.path().join("big.bin");
    let f = fs::File::create(&big).unwrap();
    f.set_len(DROP_MAX + 1).unwrap();
    drop(f);
    let e = stage(&p, &[Item::File(big.clone())], T + 1).unwrap_err();
    assert!(matches!(e, StageError::TooLarge { size } if size == DROP_MAX + 1));
    assert_eq!(e.badge(), text::drop_too_large(DROP_MAX + 1));
    assert!(e.badge().starts_with("drop too large"), "{}", e.badge());
    assert!(!p.drop_dir().join("1700000001-1").exists());
    // Exactly the cap passes the size check (the copy is a sparse 2 GiB).
    f_truncate(&big, DROP_MAX);
    assert!(stage(&p, &[Item::File(big)], T + 2).is_ok());
    assert!(matches!(stage(&p, &[], T).unwrap_err(), StageError::Empty));
    assert_eq!(StageError::Empty.badge(), "drop failed");
    let e = stage(&p, &[Item::File(t.path().join("none"))], T + 3).unwrap_err();
    assert!(matches!(e, StageError::Io(_)), "{e}");
    assert!(
        !p.drop_dir().join("1700000003-1").exists(),
        "a failed drop leaves nothing"
    );

    // The sweep keeps the folder of an open file and removes the rest.
    fs::create_dir_all(p.drop_dir().join(CLIPBOARD_DIR)).unwrap();
    let open = open_paths(&format!(
        "p123\nf4\nn{}\nn/other/file\n",
        s2.dir.join("dropped.txt").display()
    ));
    assert_eq!(
        open,
        [s2.dir.join("dropped.txt"), PathBuf::from("/other/file")]
    );
    let removed = sweep(&p, &open);
    assert!(removed.contains(&s.dir), "{removed:?}");
    assert!(removed.contains(&p.drop_dir().join(CLIPBOARD_DIR)));
    assert!(!removed.contains(&s2.dir));
    assert!(s2.dir.join("dropped.txt").exists());
    assert!(!s.dir.exists());
    assert_eq!(sweep(&p, &[]), vec![s2.dir.clone()]);
    assert!(fs::read_dir(p.drop_dir()).unwrap().next().is_none());
    assert!(sweep(&Profile::new(t.path().join("nowhere")), &[]).is_empty());
}

fn f_truncate(p: &Path, len: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(p)
        .unwrap()
        .set_len(len)
        .unwrap();
}
