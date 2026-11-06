use super::*;
use std::collections::HashSet;

#[test]
fn transitions_table() {
    use Outcome::*;
    use State::*;
    let failed = || Failed("x".into());
    let rejected = || ImageRejected("x".into());
    let table: &[(State, Outcome, State)] = &[
        (Fetch, Done, Install),
        (Fetch, failed(), Fetch),
        (Fetch, rejected(), Fetch),
        (Fetch, UserStopped, Fetch),
        (Fetch, Crashed, Fetch),
        (Install, Done, Seed),
        (Install, failed(), Install),
        (Install, rejected(), Fetch),
        (Install, UserStopped, Install),
        (Install, Crashed, Install),
        (Seed, Done, Guided),
        (Seed, failed(), Seed),
        (Seed, rejected(), Seed),
        (Seed, UserStopped, Seed),
        (Seed, Crashed, Seed),
        (Guided, Done, Trust),
        (Guided, failed(), Guided),
        (Guided, rejected(), Guided),
        (Guided, UserStopped, Guided),
        (Guided, Crashed, Guided),
        (Trust, Done, Configure),
        (Trust, failed(), Trust),
        (Trust, rejected(), Trust),
        (Trust, UserStopped, Trust),
        (Trust, Crashed, Trust),
        (Configure, Done, Seal),
        (Configure, failed(), Configure),
        (Configure, rejected(), Configure),
        (Configure, UserStopped, Configure),
        (Configure, Crashed, Configure),
        (Seal, Done, Sealed),
        (Seal, failed(), Seal),
        (Seal, rejected(), Seal),
        (Seal, UserStopped, Seal),
        (Seal, Crashed, Seal),
        (Sealed, Done, Sealed),
        (Sealed, failed(), Sealed),
        (Sealed, rejected(), Sealed),
        (Sealed, UserStopped, Sealed),
        (Sealed, Crashed, Sealed),
    ];
    // Every state with every outcome kind, once.
    let mut seen = HashSet::new();
    for (s, o, want) in table {
        assert_eq!(next(*s, o), *want, "{s:?} + {o:?}");
        assert!(seen.insert((*s, std::mem::discriminant(o))));
    }
    assert_eq!(seen.len(), State::ALL.len() * 5);
    assert!(State::Install.writes() && State::Seed.writes() && State::Seal.writes());
    assert!(!State::Fetch.writes() && !State::Guided.writes() && !State::Configure.writes());
    assert_eq!(State::Fetch.number(), 1);
    assert_eq!(State::Seal.number(), 7);
    assert_eq!(State::STEPS, 7);
    assert!(!State::Trust.writes());
}

#[test]
fn crashed_state_reruns_writer() {
    let partial = vec![PathBuf::from("/g/disk.img.partial")];
    let mut m = Manifest::new("15.2", "24C101", "0.2.1");
    // No manifest: the setup starts at Fetch.
    assert_eq!(resume_point(None, &partial), (State::Fetch, None));
    for s in State::ALL {
        m.at(s);
        let (state, crashed) = resume_point(Some(&m), &partial);
        assert_eq!(state, s);
        assert_eq!(crashed.is_some(), s.writes(), "{s:?}");
        if let Some(c) = &crashed {
            assert_eq!(*c, Outcome::Crashed);
            assert_eq!(next(state, c), s, "a crashed writer runs again");
        }
        // Without a partial, nothing crashed.
        assert_eq!(resume_point(Some(&m), &[]), (s, None));
    }
    m.at(State::Seed);
    assert_eq!(
        resume_point(Some(&m), &partial),
        (State::Seed, Some(Outcome::Crashed))
    );
}

#[test]
fn manifest_round_trip() {
    let mut m = Manifest::new("15.2", "24C101", "0.2.1");
    m.version = "15.2".into();
    m.build = "24C101".into();
    m.hardware_model_b64 = "YQ==".into();
    m.ipsw = Some(PathBuf::from("/x/15.2.ipsw"));
    m.ipsw_size = Some(15);
    m.at(State::Guided);
    let text = m.to_toml().unwrap();
    assert!(text.contains("state = \"guided\""));
    assert!(text.contains("schema = 1"));
    let back = Manifest::parse(&text, Path::new("/g/manifest.toml")).unwrap();
    assert_eq!(back, m);
    assert_eq!(back.version_text(), "15.2 (24C101)");
    assert_eq!(Manifest::new("", "", "").version_text(), "unknown");
    // An unknown field stays harmless.
    let more = format!("{text}\nfuture = 1\n");
    assert!(Manifest::parse(&more, Path::new("/g/manifest.toml")).is_ok());
}

#[test]
fn manifest_unknown_schema_names_remove() {
    let text = "schema = 2\nstate = \"fetch\"\nversion = \"\"\nbuild = \"\"\nhost_version = \"\"\nhost_build = \"\"\nagent_version = \"\"\ntime = \"\"\n";
    let e = Manifest::parse(text, Path::new("/g/manifest.toml"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("/g/manifest.toml"), "{e}");
    assert!(e.contains("schema 2"), "{e}");
    assert!(e.contains("paddock macvm remove"), "{e}");
    let none = "state = \"fetch\"\n";
    let e = Manifest::parse(none, Path::new("/g/manifest.toml"))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("schema none") && e.contains("paddock macvm remove"),
        "{e}"
    );
}

#[test]
fn manifest_unknown_state_names_remove() {
    let text = "schema = 1\nstate = \"polish\"\nversion = \"\"\nbuild = \"\"\nhost_version = \"\"\nhost_build = \"\"\nagent_version = \"\"\ntime = \"\"\n";
    let e = Manifest::parse(text, Path::new("/g/manifest.toml"))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("/g/manifest.toml") && e.contains("\"polish\""),
        "{e}"
    );
    assert!(e.contains("paddock macvm remove"), "{e}");
    let bad = "schema = 1\nstate = [1]\n";
    assert!(Manifest::parse(bad, Path::new("/g/m")).is_err());
    assert!(Manifest::parse("not toml = = ", Path::new("/g/m")).is_err());
}

#[test]
fn password_has_20_letters_that_no_layout_moves_and_differs() {
    assert_eq!(PASSWORD_LEN, 20);
    assert_eq!(PASSWORD_SET, b"bcdefghijklnoprstuvx");
    // At least 80 bits: 20 * log2(20) is 86.4.
    let bits = PASSWORD_LEN as f64 * (PASSWORD_SET.len() as f64).log2();
    assert!(bits >= 80.0, "{bits}");
    let a = make_password().unwrap();
    let b = make_password().unwrap();
    assert_eq!(a.len(), PASSWORD_LEN);
    assert!(a.bytes().all(|c| PASSWORD_SET.contains(&c)), "{a}");
    for moved in "aqwzym0123456789ABCXYZ-_.,;/".chars() {
        assert!(!a.contains(moved), "{a} has {moved}");
    }
    assert!(is_password(&a));
    assert_ne!(a, b);
    assert!(!is_password("short"));
    assert!(!is_password("bcdefghijklnoprstuva"), "a moves on AZERTY");
    assert!(!is_password("Abc123Abc123Abc1"), "the older shape");
    // Every letter of the set comes out, so the sampling covers the set.
    let many: String = (0..20).map(|_| make_password().unwrap()).collect();
    for c in PASSWORD_SET {
        assert!(many.as_bytes().contains(c), "{} never drawn", *c as char);
    }
    // Every letter of the set types on the US keys without shift.
    for c in a.chars() {
        let s = crate::macvm::guided_ui::key_strokes(&c.to_string()).unwrap();
        assert!(!s[0].shift, "{c}");
    }
}

#[test]
fn keyring_takes_the_password_the_person_set_and_older_ones() {
    assert!(is_keyring_password("Abc123Abc123Abc1"), "an older paddock");
    assert!(
        is_keyring_password(" my Pass wörd! "),
        "spaces and non-ASCII"
    );
    assert!(!is_keyring_password(""));
    assert!(!is_keyring_password("two\nlines"));
    assert!(!is_keyring_password("tab\tin"));
    assert!(!is_keyring_password(&"x".repeat(PASSWORD_MAX + 1)));
    assert!(is_keyring_password(&"x".repeat(PASSWORD_MAX)));
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let k = Keyring::open(&env).unwrap();
    let made = k.password().unwrap();
    k.replace(" my Pass wörd! ").unwrap();
    assert_eq!(k.password().unwrap(), " my Pass wörd! ", "kept as typed");
    assert_ne!(k.password().unwrap(), made);
    let e = k.replace("two\nlines").unwrap_err().to_string();
    assert!(e.contains("password"), "{e}");
    assert_eq!(
        k.password().unwrap(),
        " my Pass wörd! ",
        "a refused one changes nothing"
    );
    // A file of an older paddock, with a line end, still reads.
    std::fs::write(&k.guest_password, "Abc123Abc123Abc1\n").unwrap();
    assert_eq!(k.password().unwrap(), "Abc123Abc123Abc1");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&k.guest_password)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "{mode:o}");
}

#[test]
fn keyring_keeps_one_password_and_forgets_it() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let k = Keyring::open(&env).unwrap();
    assert_eq!(
        k.guest_password,
        env.paddock_state().join("keyring/macvm/guest-password")
    );
    let a = k.password().unwrap();
    assert_eq!(a, k.password().unwrap(), "stable across calls");
    assert!(is_password(&a));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&k.guest_password)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    k.forget().unwrap();
    assert_ne!(a, k.password().unwrap(), "a new one after forget");
    // A damaged file is replaced.
    std::fs::write(&k.guest_password, "\u{0}bad\n\n").unwrap();
    assert!(is_password(&k.password().unwrap()));
    std::fs::write(&k.guest_password, "").unwrap();
    assert!(is_password(&k.password().unwrap()));
}

#[test]
fn keyring_that_is_not_available_names_itself() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    // The state folder is a file: no keyring folder can be made.
    std::fs::create_dir_all(env.paddock_state().parent().unwrap()).unwrap();
    std::fs::write(env.paddock_state(), "").unwrap();
    let e = Keyring::open(&env).unwrap_err().to_string();
    assert!(e.contains("keyring") && e.contains("not available"), "{e}");
    assert!(
        e.contains(
            &env.paddock_state()
                .join("keyring/macvm")
                .display()
                .to_string()
        ),
        "{e}"
    );
}

#[test]
fn space_needed_per_state() {
    assert_eq!(
        space_needed(State::Fetch, None),
        IPSW_GUESS_BYTES + INSTALL_REAL_BYTES
    );
    assert_eq!(space_needed(State::Fetch, Some(17 << 30)), 47 << 30);
    assert_eq!(space_needed(State::Install, None), INSTALL_REAL_BYTES);
    assert_eq!(space_needed(State::Install, Some(1)), 30 << 30);
    assert_eq!(DISK_BYTES, 64 << 30, "the disk is 64 GiB sparse");
    for s in [
        State::Seed,
        State::Guided,
        State::Trust,
        State::Configure,
        State::Seal,
        State::Sealed,
    ] {
        assert_eq!(space_needed(s, Some(1 << 40)), 0);
    }
}

#[test]
fn space_refusal_names_sizes() {
    let opts = super::super::setup::Opts {
        fake_free: Some("1G".into()),
        ..Default::default()
    };
    let needed = space_needed(State::Fetch, None);
    let line = super::super::setup::space_refusal(Path::new("/"), needed, "the download", &opts)
        .unwrap()
        .unwrap();
    assert!(line.contains("47 GB"), "{line}");
    assert!(line.contains("1 GB"), "{line}");
    assert!(
        line.contains("the download") && line.contains("paddock macvm setup"),
        "{line}"
    );
    let ok = super::super::setup::Opts {
        fake_free: Some("100G".into()),
        ..Default::default()
    };
    assert!(
        super::super::setup::space_refusal(Path::new("/"), needed, "x", &ok)
            .unwrap()
            .is_none()
    );
    let bad = super::super::setup::Opts {
        fake_free: Some("lots".into()),
        ..Default::default()
    };
    assert!(super::super::setup::space_refusal(Path::new("/"), needed, "x", &bad).is_err());
    // The real free space of a folder is a number, also of a missing one.
    assert!(free_bytes(Path::new("/nonexistent/deeper"), None).unwrap() > 0);
}

#[test]
fn used_bytes_counts_blocks_not_length() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("sparse");
    let file = std::fs::File::create(&f).unwrap();
    file.set_len(1 << 30).unwrap();
    drop(file);
    assert_eq!(std::fs::metadata(&f).unwrap().len(), 1 << 30);
    assert!(used_bytes(&f) < 1 << 20, "a sparse file uses no blocks");
    std::fs::write(t.path().join("full"), vec![1u8; 1 << 20]).unwrap();
    assert!(used_bytes_in(t.path()) >= 1 << 20);
    assert_eq!(used_bytes(Path::new("/nonexistent")), 0);
}

// ---- The swap of `macvm update` and `rollback` -------------------------

/// A golden folder with a sealed manifest of `version` and a disk that
/// holds `version` as its text.
fn sealed_golden(g: &super::super::layout::Golden, version: &str) {
    std::fs::create_dir_all(&g.dir).unwrap();
    std::fs::write(&g.disk, version).unwrap();
    std::fs::write(&g.aux, "aux").unwrap();
    let mut m = Manifest::new("15.2", "24C101", "0.9.0");
    m.version = version.into();
    m.build = "B1".into();
    m.sealed = Some(now_text());
    m.at(State::Sealed);
    g.write_manifest(&m).unwrap();
    for f in g.sealed_files() {
        super::super::layout::seal_file(f).unwrap();
    }
}

fn disk_text(g: &super::super::layout::Golden) -> String {
    std::fs::read_to_string(&g.disk).unwrap_or_else(|_| "(none)".into())
}

#[test]
fn swap_and_crash_between_renames() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    std::fs::create_dir_all(&l.root).unwrap();
    // Nothing to swap, nothing to recover.
    assert!(swap(&l).unwrap_err().to_string().contains("no new golden"));
    assert_eq!(recover(&l).unwrap(), None);
    // An unsealed build does not become the golden.
    sealed_golden(&l.golden, "15.2");
    std::fs::create_dir_all(&l.golden_new.dir).unwrap();
    let mut m = Manifest::new("15.2", "24C101", "0.9.0");
    m.at(State::Guided);
    l.golden_new.write_manifest(&m).unwrap();
    let e = swap(&l).unwrap_err().to_string();
    assert!(
        e.contains("not sealed") && e.contains("paddock macvm update"),
        "{e}"
    );
    assert_eq!(disk_text(&l.golden), "15.2");
    // The plain swap: new becomes golden, golden becomes prev.
    sealed_golden(&l.golden_new, "15.3");
    swap(&l).unwrap();
    assert_eq!(disk_text(&l.golden), "15.3");
    assert_eq!(disk_text(&l.golden_prev), "15.2");
    assert!(!l.golden_new.dir.exists());
    assert!(!l.root.join(SWAP_MARKER).exists(), "the marker went");
    assert_eq!(
        prev_state(&l),
        Prev::Kept {
            version: "15.2 (B1)".into()
        }
    );
    // A crash between the two renames: golden is missing, prev and new
    // are there, and the marker says `update`.
    sealed_golden(&l.golden_new, "15.4");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        swap_with(&l, || panic!("crash between the renames"))
    }));
    assert!(r.is_err());
    assert!(!l.golden.dir.exists());
    assert_eq!(
        disk_text(&l.golden_prev),
        "15.3",
        "the older prev went first"
    );
    assert_eq!(disk_text(&l.golden_new), "15.4");
    assert_eq!(
        std::fs::read_to_string(l.root.join(SWAP_MARKER))
            .unwrap()
            .trim(),
        "update"
    );
    // The next start finishes the second rename.
    assert_eq!(recover(&l).unwrap(), Some(Recovered::Update));
    assert_eq!(disk_text(&l.golden), "15.4");
    assert_eq!(disk_text(&l.golden_prev), "15.3");
    assert!(!l.golden_new.dir.exists());
    assert!(!l.root.join(SWAP_MARKER).exists());
    assert_eq!(recover(&l).unwrap(), None, "a second recover does nothing");
    assert!(Recovered::Update.line().contains("paddock macvm update"));
    // Without the marker (an older paddock, or a removed file): prev with
    // new is an interrupted update too.
    std::fs::rename(&l.golden.dir, &l.golden_new.dir).unwrap();
    assert_eq!(recover(&l).unwrap(), Some(Recovered::Update));
    assert_eq!(disk_text(&l.golden), "15.4");
    // A stale marker with the golden in place goes without a rename.
    std::fs::write(l.root.join(SWAP_MARKER), "rollback").unwrap();
    assert_eq!(recover(&l).unwrap(), None);
    assert!(!l.root.join(SWAP_MARKER).exists());
    assert_eq!(disk_text(&l.golden), "15.4");
    // The marker names a folder that is gone: the other one is the golden.
    std::fs::rename(&l.golden.dir, &l.golden_new.dir).unwrap();
    std::fs::write(l.root.join(SWAP_MARKER), "rollback").unwrap();
    super::super::layout::remove_golden_dir(&l.golden_prev).unwrap();
    assert_eq!(recover(&l).unwrap(), Some(Recovered::Update));
    assert_eq!(disk_text(&l.golden), "15.4");
    assert_eq!(Swap::Update.word(), "update");
    assert_eq!(Swap::Rollback.word(), "rollback");
}

#[test]
fn rollback_cases() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    std::fs::create_dir_all(&l.root).unwrap();
    // No prev: the refusal names the folder.
    sealed_golden(&l.golden, "15.3");
    let e = rollback(&l).unwrap_err().to_string();
    assert!(e.contains("no previous golden"), "{e}");
    assert_eq!(disk_text(&l.golden), "15.3");
    assert_eq!(prev_state(&l), Prev::None);
    // Prev present: it comes back, the golden steps aside as golden.new,
    // and a golden.new from before goes.
    sealed_golden(&l.golden_prev, "15.2");
    sealed_golden(&l.golden_new, "15.9");
    rollback(&l).unwrap();
    assert_eq!(disk_text(&l.golden), "15.2");
    assert_eq!(disk_text(&l.golden_new), "15.3");
    assert!(!l.golden_prev.dir.exists());
    assert!(!l.root.join(SWAP_MARKER).exists());
    // An update after the rollback finds the set-aside golden sealed in
    // golden.new: swap would promote it again, which is what a redo wants.
    swap(&l).unwrap();
    assert_eq!(disk_text(&l.golden), "15.3");
    assert_eq!(disk_text(&l.golden_prev), "15.2");
    // A crash between the two renames of a rollback: the marker says so,
    // and the next start finishes with the previous golden in place.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rollback_with(&l, || panic!("crash"))
    }));
    assert!(r.is_err());
    assert!(!l.golden.dir.exists());
    assert_eq!(disk_text(&l.golden_new), "15.3");
    assert_eq!(disk_text(&l.golden_prev), "15.2");
    assert_eq!(
        std::fs::read_to_string(l.root.join(SWAP_MARKER))
            .unwrap()
            .trim(),
        "rollback"
    );
    assert_eq!(recover(&l).unwrap(), Some(Recovered::Rollback));
    assert_eq!(disk_text(&l.golden), "15.2");
    assert_eq!(disk_text(&l.golden_new), "15.3");
    assert!(!l.golden_prev.dir.exists());
    assert!(Recovered::Rollback
        .line()
        .contains("paddock macvm rollback"));
    // Without a marker, prev alone is an interrupted rollback.
    std::fs::rename(&l.golden.dir, &l.golden_prev.dir).unwrap();
    super::super::layout::remove_golden_dir(&l.golden_new).unwrap();
    assert_eq!(recover(&l).unwrap(), Some(Recovered::Rollback));
    assert_eq!(disk_text(&l.golden), "15.2");
}

#[test]
fn prev_removed_after_clean_session() {
    use super::super::proto::Version;
    use super::super::session::{Display, Now};
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    std::fs::create_dir_all(&l.root).unwrap();
    sealed_golden(&l.golden, "15.3");
    sealed_golden(&l.golden_prev, "15.2");
    let version = "15.2 (B1)".to_string();
    assert_eq!(
        prev_state(&l),
        Prev::Kept {
            version: version.clone()
        }
    );
    let now = |golden: &str| Now {
        golden: golden.into(),
        host_build: "24C101".into(),
        vz: "vz".into(),
        protocol: Version { major: 1, minor: 0 },
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    };
    // A profile whose last session ran on the old golden: kept.
    let a = l.profile("chatgpt", "default");
    std::fs::create_dir_all(&a.dir).unwrap();
    now("15.2 (B1)")
        .stamp("a-0".into())
        .write(&a.stamp())
        .unwrap();
    assert_eq!(
        prev_state(&l),
        Prev::Kept {
            version: version.clone()
        }
    );
    // A profile on the new golden that did not end in order: kept.
    let b = l.profile("vscode", "default");
    std::fs::create_dir_all(&b.dir).unwrap();
    let mut s = now("15.3 (B1)").stamp("b-0".into());
    s.clean = false;
    s.write(&b.stamp()).unwrap();
    assert_eq!(
        prev_state(&l),
        Prev::Kept {
            version: version.clone()
        }
    );
    // The first clean session on the new golden makes prev removable.
    s.clean = true;
    s.write(&b.stamp()).unwrap();
    assert_eq!(
        prev_state(&l),
        Prev::Removable {
            version: version.clone()
        }
    );
    assert_eq!(prev_state(&l).version(), Some(version.as_str()));
    // A stamp of an unknown schema does not count.
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(b.stamp()).unwrap()).unwrap();
    raw["schema"] = serde_json::json!(99);
    std::fs::write(b.stamp(), serde_json::to_vec(&raw).unwrap()).unwrap();
    assert_eq!(
        prev_state(&l),
        Prev::Kept {
            version: version.clone()
        }
    );
    s.write(&b.stamp()).unwrap();
    // The removal takes the sealed files with it.
    remove_prev(&l).unwrap();
    assert!(!l.golden_prev.dir.exists());
    assert_eq!(prev_state(&l), Prev::None);
    assert_eq!(disk_text(&l.golden), "15.3", "the golden is untouched");
    assert_eq!(version_text_of(&l.golden_new), "unknown");
}
