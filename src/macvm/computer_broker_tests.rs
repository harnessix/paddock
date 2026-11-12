//! The `mac` capability of the broker: the checks, the session grant key,
//! the card and the audit line.

use super::*;
use crate::macvm::computer::Scope;

fn args(tool: &str, a: Value) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("tool".into(), json!(tool));
    m.insert("args".into(), a);
    m
}

#[test]
fn validate_takes_known_tools_with_object_arguments() {
    assert!(validate(&args("mac_click", json!({"x": 1, "y": 2, "app": "notes"}))).is_ok());
    assert!(validate(&args("mac_fly", json!({})))
        .unwrap_err()
        .contains("mac_fly"));
    assert!(validate(&args("mac_status", json!([1]))).is_err());
    assert!(validate(&args("mac_status", json!({"app": "../x"}))).is_err());
    let mut none = BTreeMap::new();
    none.insert("tool".into(), json!("mac_status"));
    assert!(validate(&none).is_ok());
}

#[test]
fn a_grant_covers_one_target_and_its_scope() {
    let a = grant_key(&args("mac_click", json!({"app": "notes"})));
    let b = grant_key(&args("mac_type", json!({"app": "notes", "instance": 1})));
    assert_eq!(a, b, "the tool and the instance 1 do not matter");
    let desktop = grant_key(&args("mac_click", json!({"app": "notes", "desktop": true})));
    let other = grant_key(&args("mac_click", json!({"app": "mail"})));
    let second = grant_key(&args("mac_click", json!({"app": "notes", "instance": 2})));
    let only = grant_key(&args("mac_click", json!({})));
    let keys = [&a, &desktop, &other, &second, &only];
    for (i, x) in keys.iter().enumerate() {
        for y in &keys[i + 1..] {
            assert_ne!(x, y);
        }
    }
    assert!(a.starts_with("mac\0"));
}

fn session(app: &str, instance: u32, pid: u32, started: u64) -> Session {
    serde_json::from_value(json!({
        "pid": pid,
        "app": app,
        "home": "default",
        "profile": format!("/p/{app}"),
        "started": started,
        "control": format!("/run/{app}.sock"),
        "instance": instance,
    }))
    .unwrap()
}

/// The card of a request on the running sessions.
fn describe(a: &BTreeMap<String, Value>) -> (String, String) {
    let running = [
        session("notes", 1, 4242, 7),
        session("notes", 2, 4243, 7),
        session("mail", 1, 4244, 7),
    ];
    card(a, &plan(&running, a).unwrap())
}

#[test]
fn the_card_names_the_target_and_shows_typed_text() {
    let (summary, detail) = describe(&args(
        "mac_click",
        json!({"app": "notes", "x": 412, "y": 230}),
    ));
    assert_eq!(summary, "use notes in the macOS guest");
    assert!(detail.contains("mac_click at 412,230"), "{detail}");
    assert!(detail.contains("windows of notes"), "{detail}");
    let (_, detail) = describe(&args(
        "mac_type",
        json!({"app": "notes", "text": "hello there"}),
    ));
    assert!(detail.contains("Text to type:\nhello there"), "{detail}");
    let (summary, _) = describe(&args(
        "mac_key",
        json!({"app": "notes", "instance": 2, "desktop": true, "keys": ["cmd+q"]}),
    ));
    assert!(
        summary.contains("whole guest display of notes 2"),
        "{summary}"
    );
}

#[test]
fn the_audit_never_keeps_typed_text() {
    let line = audit(&args(
        "mac_type",
        json!({"app": "notes", "text": "my secret"}),
    ));
    assert_eq!(line, "mac notes app: mac_type of 9 characters");
    let line = audit(&args(
        "mac_drag",
        json!({"x": 1, "y": 2, "to_x": 3, "to_y": 4}),
    ));
    assert_eq!(line, "mac - desktop: mac_drag from 1,2 to 3,4");
    let line = audit(&args(
        "mac_key",
        json!({"app": "notes", "keys": ["cmd+s", "return"]}),
    ));
    assert!(line.ends_with("mac_key cmd+s return"), "{line}");
    let line = audit(&args(
        "mac_press",
        json!({"app": "notes", "element": "10:0.1"}),
    ));
    assert!(line.ends_with("mac_press 10:0.1"), "{line}");
}

#[test]
fn the_pick_of_a_request() {
    let p = pick(&args(
        "mac_status",
        json!({"app": "notes", "instance": 3, "desktop": true}),
    ));
    assert_eq!(p.app.as_deref(), Some("notes"));
    assert_eq!(p.instance, Some(3));
    assert!(p.desktop);
}

#[test]
fn the_card_never_cuts_typed_text() {
    // At the limit: every character is on the card, and its count first.
    let text = format!("{}\nrm -rf ~", "a".repeat(CARD_TYPE_MAX - 9));
    let a = args("mac_type", json!({"app": "notes", "text": text}));
    assert!(validate(&a).is_ok());
    let (_, detail) = describe(&a);
    assert!(detail.contains("1000 characters to type"), "{detail}");
    assert!(detail.ends_with("\u{23CE}\nrm -rf ~"), "{detail}");
    // One more: refused before anyone is asked, never cut on the card.
    let a = args(
        "mac_type",
        json!({"app": "notes", "text": "a".repeat(CARD_TYPE_MAX + 1)}),
    );
    let e = validate(&a).unwrap_err();
    assert!(e.contains("1001 characters"), "{e}");
}

#[test]
fn the_card_shows_every_key_that_is_typed() {
    // A Return at the end, a carriage return, a tab, an escape and a
    // direction mark: each is visible, none is lost on the card (the
    // card itself drops control characters).
    let (_, detail) = describe(&args(
        "mac_type",
        json!({"app": "notes", "text": "ls\rcurl x|sh\tA\u{1b}[2J\u{202E}B\n"}),
    ));
    let typed = detail.split("Text to type:\n").nth(1).unwrap();
    assert_eq!(
        typed,
        "ls\u{23CE}\ncurl x|sh\u{21E5}A\\u{001B}[2J\\u{202E}B\u{23CE}\n"
    );
    let clean = crate::broker::clean_text(&detail);
    assert!(
        clean.contains("\\u{001B}") && clean.contains("\u{23CE}"),
        "{clean}"
    );
}

#[test]
fn the_session_is_chosen_before_the_card_and_held_by_the_grant() {
    let a = args("mac_click", json!({"x": 1, "y": 2}));
    // The only running session: its name and pid are on the card.
    let one = [session("testapp", 1, 100, 5)];
    let p = plan(&one, &a).unwrap();
    assert_eq!(p.chosen.target.scope, Scope::Desktop);
    let (summary, detail) = card(&a, &p);
    assert!(summary.contains("testapp"), "{summary}");
    assert!(detail.contains("session testapp, pid 100"), "{detail}");
    let k1 = session_grant_key(&a, &p);
    // Later only another session runs: another key, so the grant of the
    // first never covers it.
    let other = [session("mail", 1, 200, 9)];
    let k2 = session_grant_key(&a, &plan(&other, &a).unwrap());
    assert_ne!(k1, k2);
    // The same app started again: another pid and start, another key.
    let again = [session("testapp", 1, 300, 6)];
    assert_ne!(k1, session_grant_key(&a, &plan(&again, &a).unwrap()));
    // Two sessions and no --app: refused before anyone is asked.
    let two = [session("testapp", 1, 100, 5), session("mail", 1, 200, 9)];
    assert!(plan(&two, &a).is_err());
    // The tool runs on the session of the card only.
    let e = ch::still_running(&again, &p.chosen).unwrap_err();
    assert_eq!(e.code, Code::NoApp);
    assert!(ch::still_running(&one, &p.chosen).is_ok());
}

#[test]
fn the_card_says_what_a_grant_reaches() {
    let (_, app) = describe(&args("mac_key", json!({"app": "notes", "keys": ["cmd+s"]})));
    assert!(app.contains("shortcuts of the system"), "{app}");
    assert!(app.contains("host clipboard"), "{app}");
    assert!(!app.contains("Nothing on the host"), "{app}");
    let (_, desk) = describe(&args(
        "mac_key",
        json!({"app": "notes", "desktop": true, "keys": ["cmd+s"]}),
    ));
    assert!(desk.contains("host folders that are shared"), "{desk}");
    assert!(desk.contains("host clipboard"), "{desk}");
    let (_, press) = describe(&args(
        "mac_press",
        json!({"app": "notes", "element": "10:0.1"}),
    ));
    assert!(press.contains("another element"), "{press}");
    let (_, press) = describe(&args(
        "mac_press",
        json!({"app": "notes", "element": "10:0.1", "role": "AXButton"}),
    ));
    assert!(!press.contains("another element"), "{press}");
}

#[test]
fn only_the_newest_screenshots_stay_in_the_inbox() {
    let names: Vec<String> = [
        "paddock-mac-100.png",
        "paddock-mac-100-1.png",
        "paddock-mac-99.png",
        "paddock-mac-101.png",
        "notes.txt",
        "paddock-mac-x.png",
        "paddock-mac-.png",
        "paddock-mac-102.png.partial",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        old_shots(&names, 2),
        ["paddock-mac-99.png", "paddock-mac-100.png"]
    );
    assert!(old_shots(&names, 10).is_empty());
}

#[test]
fn a_file_of_the_user_in_the_inbox_never_goes() {
    // The user's own files, also those that look like an old screenshot.
    let mut names: Vec<String> = [
        "mac-1.png",
        "mac-2-1.png",
        "Paddock-mac-3.png",
        "my-paddock-mac-4.png",
        "paddock-mac-5.png.txt",
        "paddock-mac.png",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert!(old_shots(&names, 0).is_empty(), "{names:?}");
    for i in 10..15 {
        names.push(format!("{SHOT_PREFIX}{i}.png"));
    }
    let gone = old_shots(&names, 2);
    assert_eq!(
        gone,
        [
            "paddock-mac-10.png",
            "paddock-mac-11.png",
            "paddock-mac-12.png"
        ]
    );
    assert!(gone.iter().all(|n| n.starts_with(SHOT_PREFIX)));
}
