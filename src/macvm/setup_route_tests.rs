//! The route of the guided state and the offline steps through the
//! driver: seed offline, first boot, stop, TCC patch, boot, configure.
//! The disk is a real APFS layout (`apfs_fixture`) behind a fake disk
//! tool, and the TCC database a real SQLite file.

use super::*;
use crate::macvm::offline_fake::FakeDisk;
use crate::macvm::tcc;

fn macos27(w: &World) -> Fake {
    let mut f = Fake::new(&w.layout.golden);
    f.host_version = "27.0.1".into();
    f.image = ("27.2".into(), "26B123".into());
    f.support = Ok(());
    f
}

/// A fake disk with a TCC database whose agent row is denied, as the
/// first boot leaves it, and the home of the account with its fresh
/// Downloads folder.
fn with_disk(f: &mut Fake, w: &World, row: bool) -> PathBuf {
    let dir = w.layout.root.join("test-tcc");
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("TCC.db");
    let c = rusqlite::Connection::open(&db).unwrap();
    c.execute_batch(
        "CREATE TABLE access (service TEXT NOT NULL, client TEXT NOT NULL, client_type INTEGER NOT NULL, \
         auth_value INTEGER NOT NULL, auth_reason INTEGER NOT NULL, auth_version INTEGER NOT NULL, csreq BLOB, \
         flags INTEGER, last_modified INTEGER NOT NULL DEFAULT 0, \
         indirect_object_identifier TEXT NOT NULL DEFAULT 'UNUSED', \
         PRIMARY KEY (service, client, client_type, indirect_object_identifier))",
    )
    .unwrap();
    if row {
        c.execute(
            "INSERT INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq) VALUES (?1, ?2, 1, 0, 4, 1, x'fade0c00')",
            rusqlite::params![tcc::ACCESSIBILITY, tcc::CLIENT],
        )
        .unwrap();
    }
    c.close().unwrap();
    let scratch = w.layout.root.join("test-scratch.img");
    std::fs::create_dir_all(&w.layout.root).unwrap();
    let mut d = FakeDisk::new(&scratch);
    d.model.push(crate::macvm::apfs_fixture::Entry {
        path: tcc::DB.into(),
        dir: false,
        uid: 0,
        gid: 0,
    });
    for (path, dir) in [
        ("Users/paddock", true),
        ("Users/paddock/Downloads", true),
        ("Users/paddock/Downloads/.localized", false),
    ] {
        d.model.push(crate::macvm::apfs_fixture::Entry {
            path: path.into(),
            dir,
            uid: 501,
            gid: 20,
        });
    }
    d.tcc = Some(db.clone());
    f.disk = Some(Rc::new(RefCell::new(d)));
    db
}

fn lines(w: &World, kind: &str) -> Vec<String> {
    setup_events::read(&w.layout.golden.events)
        .into_iter()
        .filter(|l| l.kind == kind)
        .map(|l| l.text)
        .collect()
}

fn person_lines(w: &World) -> usize {
    setup_events::read(&w.layout.golden.events)
        .iter()
        .filter(|l| l.person)
        .count()
}

#[test]
fn a_27_host_and_guest_with_a_disk_tool_need_nobody() {
    let w = world();
    let mut fake = macos27(&w);
    let db = with_disk(&mut fake, &w, true);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(*fake.routes.borrow(), ["unattended"]);
    let seed = lines(&w, setup_events::SEED_OFFLINE);
    assert_eq!(seed.len(), 1);
    // The test layout has no agent binary: the stage, its script, the
    // install script, the plist, the agent folder and launch, the daemon.
    assert_eq!(seed[0], "done: 7 files, fsck clean");
    assert_eq!(lines(&w, setup_events::TRUST_OFFLINE), ["updated"]);
    assert_eq!(lines(&w, setup_events::TRUSTED), ["offline"]);
    // The link of ~/Downloads in the same stop of the guest.
    assert_eq!(lines(&w, setup_events::DOWNLOADS_OFFLINE), ["made"]);
    assert!(
        out.contains("~/Downloads of the guest is now a link"),
        "{out}"
    );
    assert_eq!(
        FakeDisk::owner(&w.layout.golden.disk, "Users/paddock/Downloads"),
        (501, 20)
    );
    assert_eq!(person_lines(&w), 0, "no wait on the person");
    assert!(!out.contains("sudo /bin/sh"), "no typed command: {out}");
    assert!(out.contains("nothing to type in the guest"), "{out}");
    assert!(
        out.contains("reports the Accessibility permission"),
        "{out}"
    );
    // The order: the guided boot, the stop, the patch, the boot, the
    // configure, the stop of the seal.
    let calls: Vec<String> = fake
        .calls
        .borrow()
        .iter()
        .filter(|c| ["guided", "shutdown", "trusted", "configure"].contains(&c.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        calls,
        ["guided", "shutdown", "trusted", "configure", "shutdown"]
    );
    // The disk holds the daemon, root-owned, and the row is allowed.
    assert_eq!(
        FakeDisk::owner(&w.layout.golden.disk, crate::macvm::offline::DAEMON),
        (0, 0)
    );
    assert_eq!(
        tcc::row(&db).unwrap(),
        Some((tcc::ALLOWED, tcc::REASON_USER_SET))
    );
    // The manifest named Trust while the patch ran.
    let seen = fake.seen.borrow();
    let trusted_at = seen.iter().find(|(w, _)| w == "trusted").map(|(_, s)| *s);
    assert_eq!(trusted_at, Some(State::Trust));
}

#[test]
fn an_older_guest_leaves_only_the_account_to_the_person() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    with_disk(&mut fake, &w, true);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(*fake.routes.borrow(), ["offline GuestOld(\"15.2\")"]);
    assert!(out.contains("guest is macOS 15.2"), "{out}");
    assert!(!out.contains("sudo /bin/sh"), "{out}");
    assert_eq!(lines(&w, setup_events::TRUST_OFFLINE), ["updated"]);
}

#[test]
fn without_a_disk_tool_the_person_types_the_command_and_turns_the_switch_on() {
    let w = world();
    let mut fake = macos27(&w);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(*fake.routes.borrow(), ["provision"]);
    assert!(out.contains("sudo /bin/sh"), "{out}");
    assert!(lines(&w, setup_events::SEED_OFFLINE)[0].starts_with("skipped: "));
    assert!(
        lines(&w, setup_events::TRUST_OFFLINE).is_empty(),
        "the window trusted it"
    );
    assert!(!fake.calls.borrow().contains(&"trusted".to_string()));
}

#[test]
fn a_failed_offline_seed_keeps_the_disk_and_shows_the_typed_command() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    fake.disk.as_ref().unwrap().borrow_mut().fail_detach = true;
    // The offline trust fails on the same detach: the window again.
    fake.guided = vec![Ok(End::Done), Ok(End::Done)];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    let seed = lines(&w, setup_events::SEED_OFFLINE);
    assert!(
        seed[0].starts_with("failed: ") && seed[0].contains("Resource busy"),
        "{seed:?}"
    );
    assert!(out.contains("the disk is as it was"), "{out}");
    assert!(out.contains("sudo /bin/sh"), "{out}");
    assert_eq!(*fake.routes.borrow(), ["provision", "guided Forced"]);
    assert!(!crate::macvm::offline::backup_of(&w.layout.golden.disk).exists());
}

#[test]
fn a_failed_trust_patch_shows_the_window_for_the_switch() {
    let w = world();
    let mut fake = macos27(&w);
    let db = with_disk(&mut fake, &w, true);
    // The database goes away before the patch: an error, not a hang.
    fake.disk.as_ref().unwrap().borrow_mut().tcc = None;
    fake.guided = vec![Ok(End::Done), Ok(End::Done)];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    let t = lines(&w, setup_events::TRUST_OFFLINE);
    assert!(
        t[0].starts_with("failed: ") && t[0].contains("TCC"),
        "{t:?}"
    );
    assert!(out.contains("Turn on paddock-guest"), "{out}");
    assert_eq!(fake.routes.borrow().len(), 2, "the window again");
    assert_eq!(tcc::row(&db).unwrap(), Some((0, 4)), "untouched");
}

#[test]
fn an_agent_without_the_permission_after_the_patch_shows_the_window() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    fake.trusted = vec![Ok(Some(false))];
    fake.guided = vec![Ok(End::Done), Ok(End::Done)];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("still reports no Accessibility permission"),
        "{out}"
    );
    assert_eq!(fake.routes.borrow().len(), 2);
}

#[test]
fn a_missing_row_is_inserted() {
    let w = world();
    let mut fake = macos27(&w);
    let db = with_disk(&mut fake, &w, false);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(lines(&w, setup_events::TRUST_OFFLINE), ["inserted"]);
    assert_eq!(
        tcc::row(&db).unwrap(),
        Some((tcc::ALLOWED, tcc::REASON_USER_SET))
    );
}

#[test]
fn a_stop_in_the_trust_check_continues_there() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    fake.trusted = vec![Ok(None)];
    let (code, _) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN);
    assert_eq!(state_of(&w), State::Trust);
    let mut again = macos27(&w);
    again.disk = fake.disk.clone();
    let (code, out) = run_with(&w, &mut again, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("step 5 of 7, trust"), "{out}");
    // The row is allowed already: the second patch says so.
    assert_eq!(
        lines(&w, setup_events::TRUST_OFFLINE),
        ["updated", "already"]
    );
    assert!(again.routes.borrow().is_empty(), "no new guided boot");
}

#[test]
fn the_flag_leaves_every_step_to_the_person() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    let opts = Opts {
        guided: true,
        ..roomy()
    };
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(*fake.routes.borrow(), ["guided Forced"]);
    assert!(out.contains("PADDOCK_MACVM_GUIDED=1"), "{out}");
    assert!(out.contains("sudo /bin/sh"), "{out}");
    assert!(lines(&w, setup_events::SEED_OFFLINE)[0].starts_with("skipped: "));
    assert!(lines(&w, setup_events::TRUST_OFFLINE).is_empty());
    let l = lines(&w, setup_events::DOWNLOADS_OFFLINE);
    assert!(l.len() == 1 && l[0].starts_with("skipped: "), "{l:?}");
    assert_eq!(
        FakeDisk::owner(&w.layout.golden.disk, "Library/LaunchDaemons"),
        (0, 0)
    );
    assert!(apfs_has_no(
        &w.layout.golden.disk,
        crate::macvm::offline::DAEMON
    ));
}

/// A golden trusted before this version made the link (in the window, or
/// by an older paddock): `Configure` stops the guest and makes it first.
#[test]
fn a_golden_trusted_without_the_link_gets_it_before_configure() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    std::fs::create_dir_all(w.layout.golden.events.parent().unwrap()).unwrap();
    setup_events::record_once(&w.layout.golden.events, setup_events::TRUSTED, "window");
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(lines(&w, setup_events::TRUST_OFFLINE).is_empty());
    assert_eq!(lines(&w, setup_events::DOWNLOADS_OFFLINE), ["made"]);
    let calls = fake.calls.borrow().clone();
    let at = calls.iter().position(|c| c == "configure").unwrap();
    assert_eq!(calls[at - 1], "shutdown", "{calls:?}");
    assert_eq!(
        FakeDisk::owner(&w.layout.golden.disk, "Users/paddock/Downloads"),
        (501, 20)
    );
}

/// The link fails in the trust (no home on the disk): the trust still
/// passes, `Configure` tries once more, and the configure request runs.
#[test]
fn a_failed_link_is_an_event_and_never_stops_the_setup() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    fake.disk
        .as_ref()
        .unwrap()
        .borrow_mut()
        .model
        .retain(|e| !e.path.starts_with("Users/paddock"));
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(lines(&w, setup_events::TRUST_OFFLINE), ["updated"]);
    let l = lines(&w, setup_events::DOWNLOADS_OFFLINE);
    assert_eq!(l.len(), 2, "{l:?}");
    assert!(
        l.iter()
            .all(|t| t.starts_with("failed: ") && t.contains("Users/paddock")),
        "{l:?}"
    );
    assert!(out.contains("downloads-link fails"), "{out}");
    assert!(fake.calls.borrow().contains(&"configure".to_string()));
    // The status names the failed link.
    let (kind, why) = crate::macvm::manage::offline_failure(&w.layout.golden).unwrap();
    assert_eq!(kind, setup_events::DOWNLOADS_OFFLINE);
    assert!(why.contains("Users/paddock"), "{why}");
}

fn apfs_has_no(image: &Path, path: &str) -> bool {
    crate::macvm::apfs::owners(image, crate::macvm::apfs::ROLE_DATA, &[path]).is_err()
}

/// A stop in the guided state after a provisioned first boot: the next
/// setup finds the account made, without the options.
#[test]
fn a_second_setup_after_a_provisioned_boot_has_its_account() {
    let w = world();
    let mut fake = macos27(&w);
    with_disk(&mut fake, &w, true);
    fake.guided = vec![Ok(End::Stopped)];
    let (code, _) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN);
    // The backend records the first boot after the start.
    provision::record_first_boot(&w.layout.golden.events, true);
    let mut again = macos27(&w);
    again.disk = fake.disk.clone();
    let (code, out) = run_with(&w, &mut again, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(*again.routes.borrow(), ["unattended"]);

    // A plain first boot: the guest ignores the options from now on.
    let w = world();
    let mut fake = macos27(&w);
    fake.guided = vec![Ok(End::Stopped)];
    run_with(&w, &mut fake, &roomy(), true);
    provision::record_first_boot(&w.layout.golden.events, false);
    let mut again = macos27(&w);
    run_with(&w, &mut again, &roomy(), true);
    assert_eq!(*again.routes.borrow(), ["guided FirstBootUsed"]);
}

#[test]
fn the_route_line_names_the_route_and_the_reason() {
    let w = world();
    let mut fake = macos27(&w);
    fake.support = Err("VZMacGuestProvisioningOptions".into());
    with_disk(&mut fake, &w, true);
    run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(
        *fake.routes.borrow(),
        ["offline Missing(\"VZMacGuestProvisioningOptions\")"]
    );
    let r = lines(&w, provision::ROUTE);
    assert_eq!(r.len(), 1);
    assert!(
        r[0].starts_with("offline seeded true trust offline true ")
            && r[0].contains("VZMacGuestProvisioningOptions"),
        "{r:?}"
    );
}
