use super::*;
use crate::macvm::diskimage::Fsck;

/// The `access` table of macOS 14 and later, with the two columns that
/// macOS 27 has at the end.
const SCHEMA: &str = "
CREATE TABLE policies (id INTEGER NOT NULL PRIMARY KEY, bundle_id TEXT NOT NULL, uuid TEXT NOT NULL, display TEXT NOT NULL, UNIQUE (bundle_id, uuid));
CREATE TABLE access (
    service TEXT NOT NULL,
    client TEXT NOT NULL,
    client_type INTEGER NOT NULL,
    auth_value INTEGER NOT NULL,
    auth_reason INTEGER NOT NULL,
    auth_version INTEGER NOT NULL,
    csreq BLOB,
    policy_id INTEGER,
    indirect_object_identifier_type INTEGER,
    indirect_object_identifier TEXT NOT NULL DEFAULT 'UNUSED',
    indirect_object_code_identity BLOB,
    flags INTEGER,
    last_modified INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
    pid INTEGER,
    pid_version INTEGER,
    boot_uuid TEXT NOT NULL DEFAULT 'UNUSED',
    last_reminded INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
    one_time_reprompt_eligible INTEGER NOT NULL DEFAULT 0,
    reminder_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (service, client, client_type, indirect_object_identifier),
    FOREIGN KEY (policy_id) REFERENCES policies(id) ON DELETE CASCADE ON UPDATE CASCADE);
";

const CDHASH_REQ: &[u8] = b"\xfa\xde\x0c\x00\x00\x00\x00\x28\x00\x00\x00\x01\x00\x00\x00\x08\x00\x00\x00\x14cdhash-of-one-build!";

fn db(dir: &Path, wal: bool, row: Option<(i64, Option<&[u8]>)>) -> PathBuf {
    let p = dir.join("TCC.db");
    let c = Connection::open(&p).unwrap();
    if wal {
        let _: String = c
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
    }
    c.execute_batch(SCHEMA).unwrap();
    c.execute(
        "INSERT INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq) VALUES ('kTCCServiceSystemPolicyAllFiles', '/usr/libexec/sshd-keygen-wrapper', 1, 2, 4, 1, NULL)",
        [],
    )
    .unwrap();
    if let Some((value, csreq)) = row {
        c.execute(
            "INSERT INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, flags) VALUES (?1, ?2, 1, ?3, 4, 1, ?4, 0)",
            params![ACCESSIBILITY, CLIENT, value, csreq],
        )
        .unwrap();
    }
    c.close().unwrap();
    p
}

fn full_row(p: &Path) -> (i64, i64, Option<Vec<u8>>, String) {
    let c = Connection::open(p).unwrap();
    c.query_row(
        "SELECT auth_value, auth_reason, csreq, indirect_object_identifier FROM access WHERE service = ?1 AND client = ?2",
        params![ACCESSIBILITY, CLIENT],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

fn other_row(p: &Path) -> (i64, i64) {
    let c = Connection::open(p).unwrap();
    c.query_row(
        "SELECT auth_value, auth_reason FROM access WHERE client = '/usr/libexec/sshd-keygen-wrapper'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn no_side_files(p: &Path) {
    for s in side_files(p) {
        assert!(!s.exists(), "{} stayed", s.display());
    }
}

fn never() -> Result<Vec<u8>> {
    panic!("the row has its requirement")
}

#[test]
fn the_row_of_the_agent_becomes_allowed_and_keeps_its_requirement() {
    for wal in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let p = db(t.path(), wal, Some((0, Some(CDHASH_REQ))));
        assert_eq!(row(&p).unwrap(), Some((0, 4)));
        assert_eq!(
            allow(&p, never).unwrap(),
            Patched::Updated { csreq_added: false }
        );
        let (v, r, csreq, ind) = full_row(&p);
        assert_eq!((v, r), (ALLOWED, REASON_USER_SET));
        assert_eq!(csreq.as_deref(), Some(CDHASH_REQ));
        assert_eq!(ind, "UNUSED");
        assert_eq!(other_row(&p), (2, 4), "other rows stay");
        no_side_files(&p);
        // A second run sees it allowed.
        assert_eq!(allow(&p, never).unwrap(), Patched::Already);
        no_side_files(&p);
        let c = Connection::open(&p).unwrap();
        let check: String = c
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
    }
}

#[test]
fn a_row_without_a_requirement_gets_the_agents() {
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), false, Some((0, None)));
    let req = identifier_requirement(IDENTIFIER);
    let got = allow(&p, || Ok(req.clone())).unwrap();
    assert_eq!(got, Patched::Updated { csreq_added: true });
    assert_eq!(full_row(&p).2, Some(req));
}

#[test]
fn a_missing_row_is_inserted_with_the_defaults_of_the_schema() {
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), true, None);
    assert_eq!(row(&p).unwrap(), None);
    let req = identifier_requirement(IDENTIFIER);
    assert_eq!(allow(&p, || Ok(req.clone())).unwrap(), Patched::Inserted);
    let (v, r, csreq, ind) = full_row(&p);
    assert_eq!((v, r, ind.as_str()), (ALLOWED, REASON_USER_SET, "UNUSED"));
    assert_eq!(csreq, Some(req));
    no_side_files(&p);
}

#[test]
fn a_failed_requirement_or_an_unknown_schema_changes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), false, Some((0, None)));
    let before = std::fs::read(&p).unwrap();
    let e = allow(&p, || Err(anyhow!("codesign: no signature"))).unwrap_err();
    assert!(format!("{e:#}").contains("no signature"));
    assert_eq!(full_row(&p).0, 0);
    assert_eq!(std::fs::read(&p).unwrap(), before);
    no_side_files(&p);

    // An old schema without auth_reason.
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("TCC.db");
    let c = Connection::open(&p).unwrap();
    c.execute_batch("CREATE TABLE access (service TEXT, client TEXT, client_type INTEGER, allowed INTEGER, csreq BLOB)")
        .unwrap();
    c.close().unwrap();
    let e = allow(&p, never).unwrap_err();
    assert!(format!("{e:#}").contains("auth_value"), "{e:#}");

    // A schema with a required column that paddock does not know.
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("TCC.db");
    let c = Connection::open(&p).unwrap();
    c.execute_batch(&SCHEMA.replace(
        "reminder_count INTEGER NOT NULL DEFAULT 0,",
        "reminder_count INTEGER NOT NULL DEFAULT 0, new_thing TEXT NOT NULL,",
    ))
    .unwrap();
    c.close().unwrap();
    let e = allow(&p, || Ok(identifier_requirement(IDENTIFIER))).unwrap_err();
    assert!(format!("{e:#}").contains("new_thing"), "{e:#}");
    assert_eq!(row(&p).unwrap(), None);

    // No table, no file.
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("TCC.db");
    Connection::open(&p).unwrap().close().unwrap();
    assert!(allow(&p, never).is_err());
    assert!(allow(&t.path().join("none.db"), never).is_err());
}

#[test]
fn hostile_text_is_data_not_sql() {
    // The values are constants, but a requirement with quotes and a
    // semicolon must land as bytes.
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), false, Some((0, None)));
    let evil = b"'); DROP TABLE access; --".to_vec();
    allow(&p, || Ok(evil.clone())).unwrap();
    assert_eq!(full_row(&p).2, Some(evil));
    assert_eq!(other_row(&p), (2, 4));
}

#[test]
fn the_identifier_requirement_is_the_compiled_form() {
    let b = identifier_requirement("dev.paddock.guest");
    let mut want = vec![
        0xfa, 0xde, 0x0c, 0x00, 0, 0, 0, 40, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 17,
    ];
    want.extend_from_slice(b"dev.paddock.guest");
    want.extend_from_slice(&[0, 0, 0]);
    assert_eq!(b, want);
    // A length that is a multiple of four gets no padding.
    assert_eq!(identifier_requirement("abcd").len(), 12 + 4 + 4 + 4);
    // The published blob of `identifier "com.apple.Terminal" and anchor
    // apple` is 0x30 bytes: this one plus opAnd and opAppleAnchor, four
    // bytes each.
    let t = identifier_requirement("com.apple.Terminal");
    assert_eq!(t.len() + 8, 0x30);
    assert_eq!(&t[..8], &[0xfa, 0xde, 0x0c, 0x00, 0, 0, 0, 40]);
}

#[test]
fn the_designated_requirement_is_read_from_codesign() {
    assert_eq!(
        designated("Executable=/x/paddock-guest\ndesignated => identifier \"dev.paddock.guest\"\n")
            .as_deref(),
        Some("identifier \"dev.paddock.guest\"")
    );
    assert_eq!(
        designated("# designated => cdhash H\"0123\"\n").as_deref(),
        Some("cdhash H\"0123\"")
    );
    assert_eq!(designated("Executable=/x\n"), None);
    assert_eq!(designated("designated =>\n"), None);
    assert!(is_agent_identifier("identifier \"dev.paddock.guest\""));
    assert!(!is_agent_identifier("cdhash H\"0123\""));
}

/// A disk tool whose attach gives a folder with the database.
struct Folder {
    data: PathBuf,
    detached: bool,
    fail_detach: bool,
}

impl DiskTool for Folder {
    fn attach(&mut self, _image: &Path, _root: &Path) -> Result<Attached> {
        Ok(Attached {
            disk: "/dev/disk9".into(),
            data: self.data.clone(),
        })
    }
    fn detach(&mut self, _a: &Attached) -> Result<()> {
        self.detached = true;
        if self.fail_detach {
            anyhow::bail!("busy");
        }
        Ok(())
    }
    fn fsck(&mut self, _image: &Path) -> Result<Fsck> {
        Ok(Fsck::Clean)
    }
    fn clone_file(&mut self, _f: &Path, _t: &Path) -> Result<()> {
        Ok(())
    }
}

#[test]
fn trust_disk_patches_through_the_mount_and_always_detaches() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("Data");
    let dbdir = data.join("Library/Application Support/com.apple.TCC");
    std::fs::create_dir_all(&dbdir).unwrap();
    let p = db(&dbdir, false, Some((0, Some(CDHASH_REQ))));
    let mut f = Folder {
        data: data.clone(),
        detached: false,
        fail_detach: false,
    };
    let mnt = t.path().join("mnt");
    let got = trust_disk(&mut f, Path::new("/g/disk.img"), &mnt, never).unwrap();
    assert_eq!(got, Patched::Updated { csreq_added: false });
    assert!(f.detached);
    assert_eq!(row(&p).unwrap(), Some((ALLOWED, REASON_USER_SET)));

    // No database on the volume: an error, and still a detach.
    let mut f = Folder {
        data: t.path().join("empty"),
        detached: false,
        fail_detach: false,
    };
    let e = trust_disk(&mut f, Path::new("/g/disk.img"), &mnt, never).unwrap_err();
    assert!(format!("{e:#}").contains("TCC.db"), "{e:#}");
    assert!(f.detached);

    // A failed detach after a good patch is an error.
    let mut f = Folder {
        data,
        detached: false,
        fail_detach: true,
    };
    assert!(trust_disk(&mut f, Path::new("/g/disk.img"), &mnt, never).is_err());
}

#[test]
fn without_codesign_the_requirement_is_the_identifier_of_the_signature() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("paddock-guest");
    std::fs::write(&f, "x").unwrap();
    let want = identifier_requirement(IDENTIFIER);
    assert_eq!(agent_requirement(None), want);
    if !Path::new("/usr/bin/codesign").exists() {
        assert_eq!(agent_requirement(Some(&f)), want);
    }
}

fn service_row(p: &Path, service: &str) -> Option<(i64, Option<Vec<u8>>)> {
    let c = Connection::open(p).unwrap();
    c.query_row(
        "SELECT auth_value, csreq FROM access WHERE service = ?1 AND client = ?2",
        params![service, CLIENT],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .unwrap()
}

#[test]
fn the_agent_also_gets_full_disk_access_with_the_same_requirement() {
    // The requirement that macOS recorded is reused, none is made.
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), false, Some((0, Some(CDHASH_REQ))));
    allow(&p, never).unwrap();
    assert_eq!(
        service_row(&p, FULL_DISK),
        Some((ALLOWED, Some(CDHASH_REQ.to_vec())))
    );
    // No row at all: both get the agent's requirement, made once.
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), true, None);
    let req = identifier_requirement(IDENTIFIER);
    let mut made = 0;
    allow(&p, || {
        made += 1;
        Ok(req.clone())
    })
    .unwrap();
    assert_eq!(made, 1);
    assert_eq!(
        service_row(&p, FULL_DISK),
        Some((ALLOWED, Some(req.clone())))
    );
    // No Screen Recording: a window capture would ask in the guest.
    assert_eq!(service_row(&p, SCREEN_CAPTURE), None);
    // Photos is a service of the user: never in the system database.
    assert_eq!(service_row(&p, PHOTOS), None);
    assert_eq!(service_row(&p, ACCESSIBILITY), Some((ALLOWED, Some(req))));
    no_side_files(&p);
}

#[test]
fn photos_goes_into_the_users_database() {
    let t = tempfile::tempdir().unwrap();
    let p = db(t.path(), false, None);
    let req = identifier_requirement(IDENTIFIER);
    assert_eq!(
        allow_in(&p, USER_SERVICES, || Ok(req.clone())).unwrap(),
        Patched::Inserted
    );
    assert_eq!(service_row(&p, PHOTOS), Some((ALLOWED, Some(req))));
    assert_eq!(service_row(&p, ACCESSIBILITY), None);
    no_side_files(&p);
}
