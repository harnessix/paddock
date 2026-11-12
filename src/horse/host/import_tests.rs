use super::fixtures::*;
use super::import::{choose, import, Select};
use super::*;

struct Host {
    t: tempfile::TempDir,
    store: Store,
    home: PathBuf,
}

fn host() -> Host {
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    Host { t, store, home }
}

impl Host {
    fn staging(&self) -> PathBuf {
        self.t.path().join("state/horse/staging")
    }

    fn import(&self, harness: &str, select: Select, horse: Option<&str>) -> import::Report {
        let (found, missing) = choose(&self.home, harness, &select).unwrap();
        let mut r = import(
            &self.store,
            &self.staging(),
            harness,
            &self.home,
            &found,
            horse,
            "import",
        )
        .unwrap();
        r.missing = missing;
        r
    }
}

fn raw(st: &Store, id: &str) -> Vec<Vec<u8>> {
    st.session_nodes(&st.session(id).unwrap())
        .unwrap()
        .into_iter()
        .map(|n| n.raw)
        .collect()
}

#[test]
fn an_import_reads_a_copy_and_the_host_files_do_not_change() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 3));
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 2));
    let before = tree(&h.home);

    let r = h.import(claude::HARNESS, Select::Ids(vec![CLAUDE_A.into()]), None);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.imported.len(), 1);
    let a = &r.imported[0];
    assert!(a.new);
    assert_eq!((a.native_id.as_str(), a.rows), (CLAUDE_A, 3));
    assert_eq!(a.rel, claude_rel("p", CLAUDE_A));
    assert_eq!(tree(&h.home), before);
    // No copy stays behind.
    assert_eq!(std::fs::read_dir(h.staging()).unwrap().count(), 0);

    let s = h.store.session(&a.session).unwrap();
    let origin = s.host.clone().unwrap();
    assert_eq!(origin.via, "import");
    assert_eq!(origin.home, h.home);
    assert_eq!(
        raw(&h.store, &s.id),
        claude_rows(CLAUDE_A, 3)
            .iter()
            .map(|r| r.as_bytes().to_vec())
            .collect::<Vec<_>>()
    );
    // Run code never finds a host session by its native id.
    assert!(h
        .store
        .session_by_native(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .is_none());
    let ev = crate::horse::run::events(&h.store, &a.horse).unwrap();
    assert!(ev.iter().any(|e| e["type"] == "host.import"));

    // The host appends: the next import adds the new rows to the same session.
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 5));
    let before = tree(&h.home);
    let r2 = h.import(claude::HARNESS, Select::Ids(vec![CLAUDE_A.into()]), None);
    assert_eq!(r2.imported.len(), 1);
    assert!(!r2.imported[0].new);
    assert_eq!(r2.imported[0].session, a.session);
    assert_eq!(r2.imported[0].rows, 5);
    assert_eq!(tree(&h.home), before);

    // --all: the known session stays in its horse, the new one comes in.
    let r3 = h.import(claude::HARNESS, Select::All, None);
    assert_eq!(r3.imported.len(), 2);
    let b = r3
        .imported
        .iter()
        .find(|i| i.native_id == CLAUDE_B)
        .unwrap();
    assert!(b.new);
    assert_eq!(
        r3.imported
            .iter()
            .find(|i| i.native_id == CLAUDE_A)
            .unwrap()
            .horse,
        a.horse
    );
}

#[test]
fn an_import_keeps_the_rows_that_the_host_removed() {
    let h = host();
    let rows = claude_rows(CLAUDE_A, 4);
    write_claude(&h.home, "p", CLAUDE_A, &rows);
    let r = h.import(claude::HARNESS, Select::All, None);
    let id = r.imported[0].session.clone();
    // Claude Code rewrites the file shorter, with a new row.
    let mut rewritten = rows[..2].to_vec();
    rewritten.push(claude_row(CLAUDE_A, 9, "after the rewrite"));
    write_claude(&h.home, "p", CLAUDE_A, &rewritten);
    h.import(claude::HARNESS, Select::All, None);
    let have = raw(&h.store, &id);
    for r in rows.iter().chain(rewritten.iter()) {
        assert!(have.contains(&r.as_bytes().to_vec()), "{r}");
    }
}

#[test]
fn an_import_into_a_named_horse_and_of_codex() {
    let h = host();
    let horse = h.store.new_default_horse().unwrap();
    write_codex(&h.home, CODEX_A, &codex_rows(CODEX_A, 2));
    let before = tree(&h.home);
    let r = h.import(
        codex::HARNESS,
        Select::Under(vec![".codex/sessions/2026".into()]),
        Some(&horse.id),
    );
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.imported.len(), 1);
    assert_eq!(r.imported[0].horse, horse.id);
    assert_eq!(r.imported[0].rows, 3);
    assert_eq!(tree(&h.home), before);
    assert!(h
        .store
        .host_session(codex::HARNESS, CODEX_A)
        .unwrap()
        .is_some());
}

#[test]
fn an_import_reads_only_what_is_really_in_the_transcript_folder() {
    let h = host();
    // A session outside, and a link to it inside the transcript folder.
    let outside = write_claude(&h.home, "../elsewhere", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    let dir = h.home.join(".claude/projects/p");
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join(format!("{CLAUDE_B}.jsonl"))).unwrap();
    std::os::unix::fs::symlink(outside.parent().unwrap(), h.home.join(".claude/projects/l"))
        .unwrap();
    assert!(list(&h.home, claude::HARNESS, None).unwrap().is_empty());
    // A folder link is refused when it is chosen.
    assert!(choose(
        &h.home,
        claude::HARNESS,
        &Select::Under(vec![".claude/projects/l".into()])
    )
    .unwrap()
    .0
    .is_empty());
    assert!(list(
        &h.home,
        claude::HARNESS,
        Some(Path::new(".claude/projects/l"))
    )
    .is_err());
    for bad in [
        PathBuf::from(".codex/sessions"),
        PathBuf::from("../x"),
        PathBuf::from("/etc"),
        PathBuf::from(".claude/projects/../../x"),
    ] {
        assert!(
            choose(&h.home, claude::HARNESS, &Select::Under(vec![bad.clone()])).is_err(),
            "{}",
            bad.display()
        );
    }
    // An id that is not there.
    let r = h.import(claude::HARNESS, Select::Ids(vec![CLAUDE_A.into()]), None);
    assert_eq!(r.missing, vec![CLAUDE_A.to_string()]);
    assert!(r.imported.is_empty());
    // A harness without a codec.
    assert!(import(
        &h.store,
        &h.staging(),
        "opencode",
        &h.home,
        &[],
        None,
        "import"
    )
    .is_err());
}

#[test]
fn a_session_of_another_version_is_reported_and_the_rest_comes_in() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let old = claude_rows(CLAUDE_B, 2)
        .iter()
        .map(|r| r.replace(claude::VERSION, "1.0.0"))
        .collect::<Vec<_>>();
    write_claude(&h.home, "p", CLAUDE_B, &old);
    let r = h.import(claude::HARNESS, Select::All, None);
    assert_eq!(r.imported.len(), 1);
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].0, claude_rel("p", CLAUDE_B));
    assert!(r.failed[0].1.contains("1.0.0"), "{}", r.failed[0].1);
    assert_eq!(std::fs::read_dir(h.staging()).unwrap().count(), 0);
}

#[test]
fn an_id_names_a_session_and_never_a_folder_above_it() {
    let h = host();
    write_claude(
        &h.home,
        "-home-me-src-app",
        CLAUDE_A,
        &claude_rows(CLAUDE_A, 1),
    );
    write_claude(
        &h.home,
        "-home-me-src-app",
        CLAUDE_B,
        &claude_rows(CLAUDE_B, 1),
    );
    // The project folder of Claude Code is no session id.
    let (found, missing) = choose(
        &h.home,
        claude::HARNESS,
        &Select::Ids(vec!["-home-me-src-app".into()]),
    )
    .unwrap();
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(missing, vec!["-home-me-src-app".to_string()]);
    let r = h.import(claude::HARNESS, Select::Ids(vec![CLAUDE_A.into()]), None);
    assert_eq!(r.imported.len(), 1);
    assert_eq!(r.imported[0].native_id, CLAUDE_A);
    // A file whose name is not the id that it holds is refused: a Codex
    // rollout named for one id with the metadata of another.
    let named = "01934b20-1111-7777-8888-0000000000d4";
    let wrong = h.home.join(codex_rel(named));
    std::fs::create_dir_all(wrong.parent().unwrap()).unwrap();
    let mut b = codex_rows(CODEX_A, 1).join("\n");
    b.push('\n');
    std::fs::write(&wrong, b).unwrap();
    let r = h.import(codex::HARNESS, Select::Ids(vec![named.into()]), None);
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(
        r.failed[0]
            .1
            .contains(&format!("holds the session {CODEX_A}, not {named}")),
        "{r:?}"
    );
    assert!(r.imported.is_empty(), "{r:?}");
    assert!(h
        .store
        .host_session(codex::HARNESS, CODEX_A)
        .unwrap()
        .is_none());
}

#[test]
fn a_copy_of_the_home_is_another_host() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let copy = h.t.path().join("backup-home");
    write_claude(&copy, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 3));
    let a = h.import(claude::HARNESS, Select::All, None);
    let (found, _) = choose(&copy, claude::HARNESS, &Select::All).unwrap();
    let b = import(
        &h.store,
        &h.staging(),
        claude::HARNESS,
        &copy,
        &found,
        None,
        "import",
    )
    .unwrap();
    assert!(a.imported[0].new && b.imported[0].new);
    assert_ne!(a.imported[0].session, b.imported[0].session);
    assert_eq!(raw(&h.store, &a.imported[0].session).len(), 2);
    assert_eq!(raw(&h.store, &b.imported[0].session).len(), 3);
    let at = |home: &Path| {
        h.store
            .host_session_at(claude::HARNESS, CLAUDE_A, &canon_home(home))
            .unwrap()
            .unwrap()
            .id
    };
    assert_eq!(at(&h.home), a.imported[0].session);
    assert_eq!(at(&copy), b.imported[0].session);
    // The same home by another path is the same host.
    let other = h.home.join("..").join("home");
    let (found, _) = choose(&other, claude::HARNESS, &Select::All).unwrap();
    let c = import(
        &h.store,
        &h.staging(),
        claude::HARNESS,
        &other,
        &found,
        None,
        "import",
    )
    .unwrap();
    assert!(!c.imported[0].new);
    assert_eq!(c.imported[0].session, a.imported[0].session);
}

#[test]
fn the_lookup_of_a_host_session_reads_the_index_not_every_session() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    let r = h.import(claude::HARNESS, Select::All, None);
    let id = r.imported[0].session.clone();
    // Another session file of the store cannot be read now.
    std::fs::write(h.store.root().join("sessions/s-damaged.json"), "not json").unwrap();
    assert!(h.store.sessions().is_err());
    let home = canon_home(&h.home);
    let s = h
        .store
        .host_session_at(claude::HARNESS, CLAUDE_A, &home)
        .unwrap()
        .unwrap();
    assert_eq!(s.id, id);
    assert_eq!(
        h.store
            .host_session(claude::HARNESS, CLAUDE_A)
            .unwrap()
            .unwrap()
            .id,
        id
    );
    // A new row of the host session comes in without a scan either.
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 3));
    let r = h.import(claude::HARNESS, Select::Ids(vec![CLAUDE_A.into()]), None);
    assert_eq!(r.imported[0].session, id);
    assert_eq!(r.imported[0].rows, 3);
}

/// The sessions of the store with the native id `native`.
fn with_native(st: &Store, native: &str) -> Vec<crate::horse::Session> {
    st.sessions()
        .unwrap()
        .into_iter()
        .filter(|s| s.binding.native_id.as_deref() == Some(native))
        .collect()
}

#[test]
fn a_failed_write_of_the_index_makes_no_second_session() {
    let h = host();
    // The index is there: another host session came in before.
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 1));
    assert_eq!(
        h.import(claude::HARNESS, Select::All, None).imported.len(),
        1
    );
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let a = Select::Ids(vec![CLAUDE_A.into()]);
    crate::horse::session::FAIL_INDEX_WRITE.with(|c| c.set(true));
    let r = h.import(claude::HARNESS, a.clone(), None);
    crate::horse::session::FAIL_INDEX_WRITE.with(|c| c.set(false));
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(with_native(&h.store, CLAUDE_A).is_empty());
    // The next import makes the one session, and the one after finds it.
    let r = h.import(claude::HARNESS, a.clone(), None);
    assert!(r.imported[0].new);
    let r2 = h.import(claude::HARNESS, a, None);
    assert!(!r2.imported[0].new);
    assert_eq!(r2.imported[0].session, r.imported[0].session);
    assert_eq!(with_native(&h.store, CLAUDE_A).len(), 1);
}

#[test]
fn a_crash_between_the_index_and_the_session_makes_no_second_session() {
    let h = host();
    write_claude(&h.home, "q", CLAUDE_B, &claude_rows(CLAUDE_B, 1));
    h.import(claude::HARNESS, Select::All, None);
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let a = Select::Ids(vec![CLAUDE_A.into()]);
    crate::horse::session::CRASH_AFTER_INDEX.with(|c| c.set(true));
    let r = h.import(claude::HARNESS, a.clone(), None);
    crate::horse::session::CRASH_AFTER_INDEX.with(|c| c.set(false));
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(with_native(&h.store, CLAUDE_A).is_empty());
    // The entry names no session: the lookup finds none, and no error.
    let home = canon_home(&h.home);
    assert!(h
        .store
        .host_session_at(claude::HARNESS, CLAUDE_A, &home)
        .unwrap()
        .is_none());
    assert!(h
        .store
        .host_session(claude::HARNESS, CLAUDE_A)
        .unwrap()
        .is_none());
    let r = h.import(claude::HARNESS, a.clone(), None);
    assert!(r.imported[0].new);
    assert_eq!(
        h.store
            .host_session_at(claude::HARNESS, CLAUDE_A, &home)
            .unwrap()
            .unwrap()
            .id,
        r.imported[0].session
    );
    let r2 = h.import(claude::HARNESS, a, None);
    assert_eq!(r2.imported[0].session, r.imported[0].session);
    assert_eq!(with_native(&h.store, CLAUDE_A).len(), 1);
}

#[test]
fn a_host_session_that_cannot_be_read_gets_no_second_one() {
    let h = host();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 1));
    let r = h.import(claude::HARNESS, Select::All, None);
    let id = r.imported[0].session.clone();
    let sessions = h.store.root().join("sessions");
    std::fs::write(sessions.join(format!("{id}.json")), "not json").unwrap();
    let count = || std::fs::read_dir(&sessions).unwrap().count();
    let before = count();
    write_claude(&h.home, "p", CLAUDE_A, &claude_rows(CLAUDE_A, 2));
    let r = h.import(claude::HARNESS, Select::Ids(vec![CLAUDE_A.into()]), None);
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(r.imported.is_empty());
    assert_eq!(count(), before);
    // Both lookups say that the session cannot be read; neither finds none.
    let home = canon_home(&h.home);
    assert!(h
        .store
        .host_session_at(claude::HARNESS, CLAUDE_A, &home)
        .is_err());
    assert!(h.store.host_session(claude::HARNESS, CLAUDE_A).is_err());
}
