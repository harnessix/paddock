use super::*;
use crate::macvm::setup_events::{self, STATE};

fn inputs() -> Inputs<'static> {
    Inputs {
        forced_guided: false,
        host: "27.0.1",
        guest: "27.2",
        support: Ok(()),
        first_boot: FirstBoot::Unused,
        seeded: true,
        disk_tool: true,
    }
}

fn why(i: &Inputs) -> Why {
    match decide(i).account {
        Account::Person(w) => w,
        a => panic!("not the person: {a:?}"),
    }
}

#[test]
fn a_27_host_and_guest_with_the_classes_and_a_seeded_disk_need_nobody() {
    let r = decide(&inputs());
    assert_eq!(r.account, Account::Provision);
    assert!(r.seeded && r.trust_offline);
    assert_eq!(r.word(), "unattended");
    assert_eq!(
        decide(&Inputs {
            host: "28.1",
            guest: "27.0",
            ..inputs()
        })
        .word(),
        "unattended"
    );
    // A disk that the seed could not patch: the typed command.
    let r = decide(&Inputs {
        seeded: false,
        ..inputs()
    });
    assert_eq!(
        (r.account.clone(), r.word()),
        (Account::Provision, "provision")
    );
    // No disk tool: the person turns the switch on.
    assert!(
        !decide(&Inputs {
            disk_tool: false,
            ..inputs()
        })
        .trust_offline
    );
}

#[test]
fn the_class_missing_an_older_guest_or_host_leave_the_account_to_the_person() {
    let missing = Inputs {
        support: Err("VZMacGuestProvisioningOptions".into()),
        ..inputs()
    };
    assert_eq!(
        why(&missing),
        Why::Missing("VZMacGuestProvisioningOptions".into())
    );
    assert_eq!(
        decide(&missing).word(),
        "offline",
        "the agent and the permission still need nobody"
    );
    for (guest, host, want) in [
        ("26.6.2", "27.0.1", Why::GuestOld("26.6.2".into())),
        ("15.6", "27.0.1", Why::GuestOld("15.6".into())),
        ("", "27.0.1", Why::GuestOld("".into())),
        ("27.2", "26.4", Why::HostOld("26.4".into())),
        ("27.2", "", Why::HostOld("".into())),
    ] {
        assert_eq!(
            why(&Inputs {
                guest,
                host,
                ..inputs()
            }),
            want
        );
    }
    // A newer guest than any known: the options are the same.
    assert_eq!(
        decide(&Inputs {
            guest: "28.0",
            ..inputs()
        })
        .account,
        Account::Provision
    );
    let forced = decide(&Inputs {
        forced_guided: true,
        ..inputs()
    });
    assert_eq!(forced.why(), Some(&Why::Forced));
    assert!(
        !forced.trust_offline,
        "the flag leaves every step to the person"
    );
    assert_eq!(forced.word(), "guided");
    assert_eq!(Route::guided(Why::Forced).word(), "guided");
}

#[test]
fn a_used_first_boot_goes_to_the_person_and_a_provisioned_one_has_its_account() {
    assert_eq!(
        why(&Inputs {
            first_boot: FirstBoot::Plain,
            ..inputs()
        }),
        Why::FirstBootUsed
    );
    // The account is there: no options, even on a host that lost the class.
    let again = Inputs {
        first_boot: FirstBoot::Provisioned,
        support: Err("x".into()),
        host: "26.0",
        ..inputs()
    };
    assert_eq!(decide(&again).account, Account::Made);
    assert_eq!(decide(&again).word(), "unattended");
}

#[test]
fn the_seed_comes_from_the_last_seed_line_of_the_event_log() {
    let t = tempfile::tempdir().unwrap();
    let ev = t.path().join("events.jsonl");
    let s = || seeded(&setup_events::read(&ev));
    assert!(!s());
    setup_events::record(&ev, setup_events::SEED_OFFLINE, false, "failed: no APFS");
    assert!(!s());
    setup_events::record(
        &ev,
        setup_events::SEED_OFFLINE,
        false,
        "done: 9 files, fsck clean",
    );
    assert!(s());
    setup_events::record(
        &ev,
        setup_events::SEED_OFFLINE,
        false,
        "skipped: PADDOCK_MACVM_GUIDED=1",
    );
    assert!(!s(), "the last line counts");
}

#[test]
fn redact_hides_every_copy_and_debug_never_shows_it() {
    let s = Secret::new("hunter2");
    assert_eq!(redact("a hunter2 b hunter2", &s), "a ••• b •••");
    assert_eq!(format!("{s:?}"), "Secret(•••)");
    assert_eq!(redact("x", &Secret::new("")), "x");
}

#[test]
fn the_first_boot_comes_from_the_event_log() {
    let t = tempfile::tempdir().unwrap();
    let ev = t.path().join("events.jsonl");
    let fb = || first_boot(&setup_events::read(&ev));
    assert_eq!(fb(), FirstBoot::Unused);
    setup_events::record(&ev, STATE, false, "guided starts");
    assert_eq!(fb(), FirstBoot::Unused, "this run's own start");
    setup_events::record(&ev, STATE, false, "guided starts");
    assert_eq!(fb(), FirstBoot::Plain, "an earlier paddock booted it");
    let t2 = tempfile::tempdir().unwrap();
    let ev2 = t2.path().join("events.jsonl");
    setup_events::record(&ev2, STATE, false, "configure starts");
    assert_eq!(first_boot(&setup_events::read(&ev2)), FirstBoot::Plain);
    // The record is written once: a later boot does not change it.
    let t3 = tempfile::tempdir().unwrap();
    let ev3 = t3.path().join("events.jsonl");
    record_first_boot(&ev3, true);
    record_first_boot(&ev3, false);
    let lines = setup_events::read(&ev3);
    assert_eq!(lines.len(), 1);
    assert_eq!(first_boot(&lines), FirstBoot::Provisioned);
    let t4 = tempfile::tempdir().unwrap();
    let ev4 = t4.path().join("events.jsonl");
    record_first_boot(&ev4, false);
    assert_eq!(first_boot(&setup_events::read(&ev4)), FirstBoot::Plain);
}

/// The values that macOS refuses fail before the boot, and the error
/// never holds the password.
#[test]
fn the_options_are_checked_before_the_boot() {
    let pw = "bcdefghijklnoprstuvx";
    assert_eq!(check_options(FULL_NAME, "paddock", pw), Ok(()));
    let long = "a".repeat(PASSWORD_MAX_BYTES + 1);
    for (full, user, p) in [
        ("", "paddock", pw),
        (FULL_NAME, "", pw),
        (FULL_NAME, "päddock", pw),
        (FULL_NAME, "pad dock", pw),
        (FULL_NAME, "paddock", ""),
        (FULL_NAME, "paddock", long.as_str()),
        (FULL_NAME, "paddock", "a\0b"),
    ] {
        let e = check_options(full, user, p).unwrap_err();
        let text = crate::macvm::text::guided_why(&e);
        assert!(!p.is_empty() && !text.contains(p) || p.is_empty(), "{text}");
        assert!(matches!(e, Why::Refused(_)));
    }
    assert_eq!(
        check_options(FULL_NAME, "paddock", &"a".repeat(PASSWORD_MAX_BYTES)),
        Ok(())
    );
}

#[test]
fn majors() {
    assert_eq!(major("27.0.1"), Some(27));
    assert_eq!(major("27"), Some(27));
    assert_eq!(major(""), None);
    assert_eq!(major("x.1"), None);
}
