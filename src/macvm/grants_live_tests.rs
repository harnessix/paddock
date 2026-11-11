//! The live grants of the shim: the control lines, the plans of each
//! change, the open file handles and the record.

use super::*;

fn shares() -> Vec<Share> {
    let s = |name: &str, host: &str, read_only| Share {
        name: name.into(),
        host: host.into(),
        read_only,
    };
    vec![
        s("ws-1", "/Users/me/src/web", false),
        s("drop", "/p/drop", false),
        s("downloads", "/p/downloads", false),
        s("apps", "/p/apps", true),
    ]
}

fn live() -> Live {
    Live::new("sample-notes", shares(), Clipboard::default(), "ask")
}

#[test]
fn every_line_parses_back_to_its_op() {
    let ops = [
        Op::ShareAdd {
            path: "/Users/me/My Projects".into(),
            read_only: true,
        },
        Op::ShareRemove {
            path: "/Users/me/x".into(),
        },
        Op::ShareMode {
            path: "/Users/me/x".into(),
            read_only: false,
        },
        Op::Network {
            mode: "none".into(),
        },
        Op::Clipboard(Clipboard {
            to_app: false,
            from_app: true,
        }),
        Op::List,
    ];
    for op in ops {
        let line = op.line();
        let rest = line.strip_prefix("grant ").unwrap();
        assert_eq!(Op::parse(rest).unwrap(), op, "{line}");
    }
    assert_eq!(
        Op::ShareAdd {
            path: "/a b".into(),
            read_only: false
        }
        .line(),
        "grant share add rw /a b"
    );
    assert_eq!(
        Op::parse("share add ro /a/../b/").unwrap(),
        Op::ShareAdd {
            path: "/b".into(),
            read_only: true
        }
    );
    for bad in [
        "",
        "share",
        "share add ro",
        "share add xx /a",
        "share add ro relative",
        "share drop /a",
        "network egress",
        "network",
        "clipboard on",
        "clipboard on maybe",
        "list now",
        "reboot",
    ] {
        assert!(Op::parse(bad).is_err(), "{bad}");
    }
    assert!(Op::parse("network egress")
        .unwrap_err()
        .contains("ask, allowlist or none"));
}

#[test]
fn an_added_folder_gets_a_new_name_before_paddocks_shares() {
    let l = live();
    let p = l
        .plan(
            &Op::ShareAdd {
                path: "/Users/me/docs".into(),
                read_only: true,
            },
            10,
        )
        .unwrap();
    assert_eq!(p.answer.name.as_deref(), Some("ws-2"));
    assert_eq!(
        p.answer.guest.as_deref(),
        Some(Path::new("/Volumes/My Shared Files/ws-2"))
    );
    assert_eq!(p.answer.applies, Applies::Live);
    let set = p.vm_shares.clone().unwrap();
    let names: Vec<&str> = set.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["ws-1", "ws-2", "drop", "downloads", "apps"]);
    assert!(set[1].read_only);
    // Nothing changed before the commit.
    assert_eq!(l.shares.len(), 4);
    // The same folder again is refused; a removed name never comes back.
    let l = p.live;
    let e = l
        .plan(
            &Op::ShareAdd {
                path: "/Users/me/docs".into(),
                read_only: false,
            },
            11,
        )
        .unwrap_err();
    assert!(
        e.contains("is shared with sample-notes already, read only"),
        "{e}"
    );
    let l = l
        .plan(
            &Op::ShareRemove {
                path: "/Users/me/docs".into(),
            },
            12,
        )
        .unwrap()
        .live;
    let p = l
        .plan(
            &Op::ShareAdd {
                path: "/Users/me/other".into(),
                read_only: true,
            },
            13,
        )
        .unwrap();
    assert_eq!(p.answer.name.as_deref(), Some("ws-3"));
}

#[test]
fn only_workspace_folders_can_be_removed_or_changed() {
    let l = live();
    for op in [
        Op::ShareRemove {
            path: "/p/drop".into(),
        },
        Op::ShareMode {
            path: "/p/apps".into(),
            read_only: false,
        },
        Op::ShareRemove {
            path: "/Users/me/none".into(),
        },
    ] {
        let e = l.plan(&op, 1).unwrap_err();
        assert!(e.contains("is not shared with sample-notes"), "{e}");
    }
}

#[test]
fn a_mode_change_with_open_files_asks_for_a_reopen_for_thirty_seconds() {
    let mut l = live();
    let ro = Op::ShareMode {
        path: "/Users/me/src/web".into(),
        read_only: true,
    };
    // No count yet: the change is live, the open files are unknown.
    let p = l.plan(&ro, 100).unwrap();
    assert_eq!(p.answer.applies, Applies::Live);
    assert_eq!(p.answer.open_handles, None);
    assert!(!p.answer.reopen_needed);
    // Two open files: reopen-files until 30 s after the change.
    l.on_handles([("ws-1".to_string(), 2)].into(), 100);
    let p = l.plan(&ro, 100).unwrap();
    assert_eq!(p.answer.applies, Applies::ReopenFiles);
    assert_eq!(p.answer.open_handles, Some(2));
    assert!(p.answer.reopen_needed);
    assert!(p.vm_shares.as_ref().unwrap()[0].read_only);
    let l = p.live;
    assert_eq!(l.applies_of("ws-1", 129), Applies::ReopenFiles);
    assert_eq!(l.applies_of("ws-1", 130), Applies::Live);
    let rec = l.record(110);
    assert_eq!(rec[0].reopen_until, Some(130));
    assert_eq!(rec[0].open_handles, Some(2));
    assert_eq!(rec[0].last_use, Some(100));
    assert!(l.record(131)[0].reopen_until.is_none());
    // The same mode again changes nothing in the VM.
    let p = l.plan(&ro, 140).unwrap();
    assert!(p.vm_shares.is_none());
}

#[test]
fn a_remove_says_how_many_files_were_open() {
    let mut l = live();
    l.on_handles([("ws-1".to_string(), 3), ("drop".to_string(), 1)].into(), 5);
    let p = l
        .plan(
            &Op::ShareRemove {
                path: "/Users/me/src/web".into(),
            },
            6,
        )
        .unwrap();
    assert_eq!(p.answer.open_handles, Some(3));
    assert!(p.answer.reopen_needed);
    assert!(!p.live.has_workspace());
    assert!(p.live.record(6).is_empty());
}

#[test]
fn network_clipboard_and_list() {
    let mut l = live();
    l.on_handles([("ws-1".to_string(), 0)].into(), 50);
    let p = l
        .plan(
            &Op::Network {
                mode: "none".into(),
            },
            1,
        )
        .unwrap();
    assert!(p.vm_shares.is_none());
    assert_eq!(p.live.network, "none");
    let c = Clipboard {
        to_app: false,
        from_app: true,
    };
    let p = p.live.plan(&Op::Clipboard(c), 2).unwrap();
    assert_eq!(p.live.clipboard, c);
    let a = p.live.plan(&Op::List, 60).unwrap().answer;
    assert_eq!(a.network.as_deref(), Some("none"));
    assert_eq!(a.clipboard, Some(c));
    assert_eq!(
        a.shares,
        [LiveShare {
            name: "ws-1".into(),
            path: "/Users/me/src/web".into(),
            read_only: false,
            guest: "/Volumes/My Shared Files/ws-1".into(),
            open_handles: Some(0),
            last_use: None,
            applies: Some(Applies::Live),
        }]
    );
    // The answer is one JSON line that the CLI reads back.
    let json = serde_json::to_string(&a).unwrap();
    assert!(!json.contains('\n'));
    let back: Answer = serde_json::from_str(&json).unwrap();
    assert_eq!(back, a);
}

#[test]
fn lsof_lines_count_per_share_without_the_agent() {
    let out =
        "p100\nfcwd\nn/Volumes/My Shared Files/ws-1\nf3\nn/Volumes/My Shared Files/ws-1/a b.txt\n\
               f4\nn/Users/paddock/x\np7\nf5\nn/Volumes/My Shared Files/ws-1/c\n\
               f6\nn/Volumes/My Shared Files/downloads/d.zip\nf7\nn/Volumes/My Shared Files\n";
    let c = count_open(out, "/Volumes/My Shared Files", 7);
    assert_eq!(c, [("ws-1".to_string(), 2)].into());
    let c = count_open(out, "/Volumes/My Shared Files", 1);
    assert_eq!(
        c,
        [("downloads".to_string(), 1), ("ws-1".to_string(), 3)].into()
    );
    assert!(count_open("", "/Volumes/My Shared Files", 1).is_empty());
}

#[test]
fn the_wire_form_of_shares_open() {
    use crate::macvm::proto::{timeout, Call, Reply, ReplyData, Request};
    let call = Call {
        req: 4,
        request: Request::SharesOpen,
    };
    let json = serde_json::to_string(&call).unwrap();
    assert_eq!(json, r#"{"req":4,"op":"shares.open"}"#);
    assert_eq!(serde_json::from_str::<Call>(&json).unwrap(), call);
    assert_eq!(timeout(&Request::SharesOpen), Duration::from_secs(15));
    assert_eq!(Request::SharesOpen.kind(), "shares.open");
    let r = Reply::data(
        4,
        ReplyData::OpenFiles {
            per_share: [("ws-1".to_string(), 3)].into(),
        },
    );
    let json = serde_json::to_string(&r).unwrap();
    assert_eq!(
        json,
        r#"{"req":4,"result":"ok","data":{"open_files":{"per_share":{"ws-1":3}}}}"#
    );
    assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), r);
    assert_eq!(r.kind(), "reply 4 ok with open files of 1");
}

#[test]
fn late_share_answer_does_not_accept_change() {
    let (tx, rx) = std::sync::mpsc::channel();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
    let shares = std::sync::Arc::new(std::sync::Mutex::new(vec!["old"]));
    let fake = shares.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        *fake.lock().unwrap() = vec!["new"];
        super::deliver_before_deadline(&tx, deadline, Ok("ok".into()), || {
            *fake.lock().unwrap() = vec!["old"];
        })
    });
    assert!(rx
        .recv_timeout(std::time::Duration::from_millis(10))
        .is_err());
    assert!(!worker.join().unwrap());
    assert_eq!(*shares.lock().unwrap(), vec!["old"]);
}

#[test]
fn live_add_refuses_paddock_data() {
    let op = Op::ShareAdd {
        path: "/Users/me/.local/share/paddock".into(),
        read_only: true,
    };
    assert!(live()
        .plan_checked(
            &op,
            1,
            Path::new("/Users/me"),
            &[PathBuf::from("/Users/me/.local/share/paddock")]
        )
        .is_err());
}
