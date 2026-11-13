use super::*;
use crate::macvm::grants::{Change, Source};
use crate::macvm::grants_cli::{Downloads, Row};
use crate::macvm::grants_live::Answer;
use std::path::PathBuf;

fn outcome(
    what: What,
    action: &str,
    value: &str,
    applies: Applies,
    live: Option<Answer>,
) -> Outcome {
    Outcome::new(
        Change::new(1, what, action, "/h/me/src", value, applies, Source::You),
        live,
        Vec::new(),
    )
}

#[test]
fn grants_unreadable() {
    assert_eq!(
        super::grants_unreadable(Path::new("/p/grants.json")),
        "paddock cannot read or make /p/grants.json"
    );
}

#[test]
fn grants_damaged() {
    let t = super::grants_damaged(Path::new("/p/grants.json"), &"bad");
    assert!(
        t.starts_with("the grants file /p/grants.json is damaged (bad)."),
        "{t}"
    );
    assert!(t.contains("move it away and set the network again"));
    assert!(t.contains("network none"));
}

#[test]
fn grants_missing() {
    let t = super::grants_missing(Path::new("/p/grants.json"));
    assert!(t.contains("/p/grants.json is missing"), "{t}");
    assert!(t.contains("paddock app network NAME"), "{t}");
}

#[test]
fn grants_missing_unchanged() {
    let t = super::grants_missing_unchanged(Path::new("/p/grants.json"));
    assert!(t.starts_with(&super::grants_missing(Path::new("/p/grants.json"))));
    assert!(t.ends_with("paddock saves no other grant change"), "{t}");
}

#[test]
fn grants_bad_network() {
    let t = super::grants_bad_network(Path::new("/p/grants.json"), "host");
    assert!(
        t.starts_with("the grants file /p/grants.json names the network mode \"host\""),
        "{t}"
    );
    assert!(t.contains("paddock does not change it"), "{t}");
}

#[test]
fn grants_changed_at_start() {
    assert!(super::grants_changed_at_start().contains("Nothing started"));
}

#[test]
fn grants_busy_at_start() {
    let t = super::grants_busy_at_start(Path::new("/p/grants.json"));
    assert!(t.contains("holds /p/grants.json too long"), "{t}");
    assert!(t.contains("Nothing started"), "{t}");
}

#[test]
fn grants_unconfirmed() {
    let t = super::grants_unconfirmed(Path::new("/p/grants.json"));
    assert!(t.contains("/p/grants.json is in place"), "{t}");
    assert!(t.contains("cannot confirm that it is on disk"), "{t}");
}

#[test]
fn grants_newer() {
    assert!(super::grants_newer(Path::new("/g"), 3).contains("version 3, from a newer paddock"));
}

#[test]
fn grants_skipped() {
    assert!(super::grants_skipped("x").starts_with("x. This start uses none"));
}

#[test]
fn grants_unwritable() {
    assert!(super::grants_unwritable(Path::new("/g")).contains("cannot confirm the write"));
}

#[test]
fn grant_share_refused() {
    assert_eq!(
        super::grant_share_refused("sample-notes", "~/Library", "it holds your keys, logins or app data"),
        "paddock does not share ~/Library with sample-notes: it holds your keys, logins or app data. Choose a project folder"
    );
}

#[test]
fn grant_share_anyway() {
    assert!(super::grant_share_anyway("sample-notes", "~/src")
        .ends_with("paddock app share sample-notes add '~/src' --unprotected"));
}

#[test]
fn grant_bad_mode() {
    assert_eq!(
        super::grant_bad_mode("rx"),
        "the folder mode \"rx\" is not ro or rw"
    );
}

#[test]
fn grant_bad_switch() {
    assert_eq!(super::grant_bad_switch("yes"), "\"yes\" is not on or off");
}

#[test]
fn grant_bad_line() {
    assert_eq!(super::grant_bad_line("x"), "unknown grant request \"x\"");
}

#[test]
fn grant_bad_network() {
    assert!(super::grant_bad_network("egress").contains("not one of ask, allowlist or none"));
}

#[test]
fn grant_already_shared() {
    assert_eq!(
        super::grant_already_shared("app", Path::new("/a"), true),
        "/a is shared with app already, read only. To change the mode: paddock app share app mode '/a' rw"
    );
    assert!(super::grant_already_shared("app", Path::new("/a"), false).contains("read and write"));
}

#[test]
fn grant_not_shared() {
    assert!(super::grant_not_shared("app", Path::new("/a"))
        .ends_with("paddock app share app list shows the folders"));
}

#[test]
fn open_files_failed() {
    assert_eq!(
        super::open_files_failed("/usr/sbin/lsof", "denied"),
        "cannot count the open files with /usr/sbin/lsof: denied"
    );
}

#[test]
fn open_files_timeout() {
    assert!(super::open_files_timeout("lsof").starts_with("lsof did not end in time"));
}

#[test]
fn grant_no_profile() {
    assert!(super::grant_no_profile("x").ends_with("paddock app list shows the apps"));
}

#[test]
fn grant_session_unreachable() {
    assert!(super::grant_session_unreachable(Path::new("/c.sock")).contains("Nothing changed"));
}

#[test]
fn grant_no_answer() {
    assert!(super::grant_no_answer(Path::new("/c.sock")).contains("Nothing was written"));
}

#[test]
fn grant_bad_answer() {
    assert_eq!(
        super::grant_bad_answer("x"),
        "the session sent an unknown answer: x"
    );
}

#[test]
fn grant_done() {
    let live = Answer {
        guest: Some(PathBuf::from("/Volumes/My Shared Files/ws-2")),
        ..Answer::live()
    };
    let o = outcome(What::Folder, "add", "rw", Applies::Live, Some(live));
    assert_eq!(
        super::grant_done("sample-notes", "~/Projects", &o),
        "Added ~/Projects to sample-notes, read and write. Visible in the app under /Volumes/My Shared Files/ws-2."
    );
    let o = outcome(What::Folder, "add", "ro", Applies::NextStart, None);
    assert_eq!(
        super::grant_done("sample-notes", "~/Projects", &o),
        "Added ~/Projects to sample-notes, read only. It applies from the next start of the app."
    );
    let reopen = Answer {
        open_handles: Some(2),
        applies: Applies::ReopenFiles,
        ..Answer::live()
    };
    let o = outcome(
        What::Folder,
        "mode",
        "ro",
        Applies::ReopenFiles,
        Some(reopen.clone()),
    );
    assert_eq!(
        super::grant_done("sample-notes", "~/Projects", &o),
        "~/Projects is now read only for sample-notes. The app holds 2 files of it open: it must open them again to see the change."
    );
    let o = outcome(What::Folder, "remove", "-", Applies::Live, Some(reopen));
    assert_eq!(
        super::grant_done("sample-notes", "~/Projects", &o),
        "Removed ~/Projects from sample-notes. The app does not see it now. The app had 2 files open in it."
    );
    let o = outcome(What::Folder, "remove", "-", Applies::NextStart, None);
    assert!(super::grant_done("c", "~/P", &o).ends_with("The next start leaves it out."));
    let closed = Answer {
        closed: Some(3),
        ..Answer::live()
    };
    let o = outcome(What::Network, "set", "none", Applies::Live, Some(closed));
    assert_eq!(
        super::grant_done("sample-notes", "none", &o),
        "The network mode of sample-notes is now none. 3 open connections closed."
    );
    let o = outcome(
        What::Clipboard,
        "set",
        "to-app off, from-app on",
        Applies::Live,
        None,
    );
    assert_eq!(
        super::grant_done("sample-notes", "clipboard", &o),
        "sample-notes: to-app off, from-app on."
    );
}

#[test]
fn grant_remove_open() {
    assert!(super::grant_remove_open("sample-notes", "~/P", 2).starts_with(
        "sample-notes has 2 files open in ~/P. Removing the folder closes them for the app, and unsaved changes in those files can be lost."
    ));
}

#[test]
fn grant_list_from_record() {
    assert!(super::grant_list_from_record("timeout").contains("from the session record"));
}

#[test]
fn grant_clipboard_now() {
    let c = Clipboard {
        to_app: true,
        from_app: false,
    };
    assert_eq!(
        super::grant_clipboard_now("sample-notes", c),
        "Clipboard of sample-notes: paste from this Mac into the app on, copy from the app to this Mac off"
    );
}

#[test]
fn grant_list() {
    let l = Listing {
        app: "sample-notes".into(),
        running: true,
        folders: vec![Row {
            path: "/h/me/src".into(),
            shown: "~/src".into(),
            mode: "ro".into(),
            status: Applies::ReopenFiles,
            guest: Some("/Volumes/My Shared Files/ws-1".into()),
            open_handles: Some(1),
            last_use: Some(5),
            from: "grant",
        }],
        downloads: Downloads {
            path: "/p/downloads".into(),
            shown: "/p/downloads".into(),
            guest: "/Volumes/My Shared Files/downloads".into(),
        },
        clipboard: Clipboard::default(),
        network: Some("ask".into()),
        warning: None,
    };
    let t = super::grant_list(&l);
    assert!(t.starts_with("Folders of sample-notes (running):\n"), "{t}");
    assert!(
        t.contains("  ~/src  read only  live, reopen files, 1 open files -> /Volumes/My Shared Files/ws-1\n"),
        "{t}"
    );
    assert!(t.contains("Network mode: ask\n"));
    let empty = Listing {
        folders: vec![],
        running: false,
        ..l
    };
    assert!(super::grant_list(&empty).contains("  none. Add one"));
}

#[test]
fn grant_quitting() {
    assert!(super::grant_quitting().starts_with("the session is ending"));
}

#[test]
fn grant_busy() {
    assert!(super::grant_busy().contains("still going on"));
}

#[test]
fn grant_vm_refused() {
    assert_eq!(
        super::grant_vm_refused("e"),
        "the VM did not take the folder change (e). The app sees the folders of before"
    );
}

#[test]
fn no_share_device() {
    assert!(super::no_share_device().contains("no shared folders device"));
}

#[test]
fn open_files_off() {
    assert!(super::open_files_off("x").contains("does not count open files (x)"));
}

#[test]
fn grant_reverted() {
    assert!(super::grant_reverted().contains("reverted"));
}

#[test]
fn grant_revert_failed() {
    assert!(super::grant_revert_failed("offline").contains("offline"));
    assert!(super::grant_revert_failed("offline").contains("until the app stops"));
}

#[test]
fn grant_open_unknown() {
    assert!(super::grant_open_unknown(Path::new("/folder")).contains("--force"));
}

#[test]
fn grant_missing_live_setting() {
    assert!(super::grant_missing_live_setting("network").contains("network"));
}

#[test]
fn grant_historical() {
    assert_eq!(
        super::grant_historical("allow session"),
        "allow session (historical; order unknown)"
    );
    assert!(super::grant_historical("x").ends_with(HISTORICAL_MARK));
}

#[test]
fn grant_history_found() {
    assert!(super::grant_history_found().starts_with("Checkpoint grant history was found."));
}

#[test]
fn grant_history_not_moved() {
    let s = super::grant_history_not_moved(
        Path::new("/p/grants.json"),
        &anyhow::anyhow!("disk full").context("move"),
    );
    assert!(s.contains("/p/grants.json: move: disk full."), "{s}");
    assert!(s.contains("older paddock versions"), "{s}");
}

#[test]
fn grant_history_at() {
    assert_eq!(
        super::grant_history_at(Path::new("/p/h.json")),
        "grant history at /p/h.json"
    );
}

#[test]
fn grant_checkpoint_not_kept() {
    assert_eq!(
        super::grant_checkpoint_not_kept(),
        "could not preserve checkpoint grant history"
    );
}

#[test]
fn grant_checkpoint_unpreservable() {
    assert_eq!(
        super::grant_checkpoint_unpreservable(),
        "cannot preserve the original checkpoint grant history"
    );
}

#[test]
fn grant_history_not_saved() {
    assert_eq!(
        super::grant_history_not_saved(&anyhow::anyhow!("denied").context("sync")),
        "grant history was not saved: sync: denied"
    );
}

#[test]
fn grant_history_writing() {
    assert_eq!(
        super::grant_history_writing(Path::new("/p/r0.json")),
        "writing /p/r0.json"
    );
}

#[test]
fn grant_history_no_recovery_name() {
    assert_eq!(
        super::grant_history_no_recovery_name(),
        "no free grant history recovery name"
    );
}

#[test]
fn grant_kept_unconfirmed() {
    assert!(super::grant_kept_unconfirmed().starts_with("The live change stays"));
}

#[test]
fn grant_session_changed() {
    let t = super::grant_session_changed("Notes");
    assert!(t.starts_with("Notes started or stopped"), "{t}");
    assert!(t.contains("Nothing changed"), "{t}");
}

#[test]
fn grant_done_keep() {
    let o = outcome(What::Folder, "keep", "rw", Applies::NextStart, None);
    assert_eq!(
        super::grant_done("sample-notes", "~/Projects", &o),
        "Kept ~/Projects for the next starts of sample-notes, read and write. The running session does not change."
    );
}

#[test]
fn grant_clipboard_both() {
    let on = Clipboard {
        to_app: true,
        from_app: false,
    };
    let off = Clipboard {
        to_app: false,
        from_app: false,
    };
    let line = super::grant_clipboard_both("sample-notes", on, Some(off));
    assert!(
        line.contains("Saved clipboard of sample-notes") && line.contains("This session"),
        "{line}"
    );
    assert!(!super::grant_clipboard_both("n", on, None).contains("This session"));
}

#[test]
fn grant_session_not_running() {
    let e = super::grant_session_not_running("sample-notes");
    assert!(e.contains("paddock app run sample-notes"), "{e}");
}

#[test]
fn grant_already_kept() {
    let e = super::grant_already_kept("sample-notes", std::path::Path::new("/h/me/src"));
    assert!(
        e.contains("/h/me/src") && e.contains("share sample-notes list"),
        "{e}"
    );
}

#[test]
fn grant_keep_not_running() {
    let e = super::grant_keep_not_running("sample-notes");
    assert!(
        e.contains("not running") && e.contains("share sample-notes add PATH"),
        "{e}"
    );
}
