//! The `Running` record of a guest session.

use super::*;
use crate::mark::{run_badge, run_menu_title, Target, Tone};

fn session() -> Session {
    Session {
        pid: 812,
        app: "sample-notes".into(),
        profile: "/data/macvm/profiles.noindex/sample-notes/default".into(),
        started: 1000,
        display_name: "Sample Notes (paddock)".into(),
        ..Default::default()
    }
}

#[test]
fn the_record_names_the_shim_with_the_macvm_backend() {
    let s = session();
    let r = record(&s, &[PathBuf::from("/work/web")], "ask", 700);
    assert_eq!(r.id, "macvm-sample-notes");
    assert_eq!(r.backend, "macvm");
    assert_eq!(r.gui, "app");
    assert_eq!(r.pid, 812);
    assert_eq!(r.host_pid, 700);
    assert_eq!(r.project, PathBuf::from("/work/web"));
    assert_eq!(Tone::of(&r.network), Tone::Sage);
    // The marker marks the windows of the shim's process tree.
    assert_eq!(crate::mark::marked_runs(vec![r.clone()]).len(), 1);
    let procs = crate::mark::ProcTable::new();
    assert!(matches!(Target::of(&r, &procs), Target::Tree(t) if t.contains(&812)));
    // Without a workspace folder the project is the profile.
    let r = record(&s, &[], "none", 700);
    assert_eq!(r.project, s.profile);
}

#[test]
fn the_display_name_is_the_dock_name() {
    assert_eq!(
        display_name("sample-notes", 1, None),
        "sample-notes (paddock)"
    );
    assert_eq!(
        display_name("sample-notes", 2, None),
        "sample-notes 2 (paddock)"
    );
}

#[test]
fn each_instance_has_its_own_running_id() {
    let mut s = session();
    let first = record(&s, &[], "ask", 1).id;
    s.instance = 2;
    let second = record(&s, &[], "ask", 1).id;
    assert_ne!(first, second);
    assert_eq!(second, format!("{first}--2"));
}

#[test]
fn the_marks_of_a_session_come_from_both_records() {
    let mut s = session();
    s.guest_input = Some("Pinyin - Simplified".into());
    s.badge = Some(crate::macvm::text::input_source("Pinyin - Simplified"));
    let r = record(&s, &[PathBuf::from("/work/web")], "ask", 700);
    let facts = crate::mark::vm::match_facts(std::slice::from_ref(&r), [&s].into_iter());
    let f = facts[0].as_ref().expect("the session has facts");
    // The input badge text is not repeated as a note.
    assert_eq!(f.note, None);
    assert_eq!(
        run_badge(&r, Some(f), 1000 + 7200),
        "paddock · sample-notes · ask · vm · Pinyin - Simplified · 2h"
    );
    assert_eq!(
        run_menu_title(&r, Some(f), 1000 + 60, 1, 2, None),
        "Sample Notes (paddock) · web · ask · vm · 1m (1/2)"
    );
    // Another badge of the session is a note at the end.
    s.badge = Some(crate::macvm::text::display_changed());
    s.guest_input = None;
    let facts = crate::mark::vm::match_facts(std::slice::from_ref(&r), [&s].into_iter());
    assert_eq!(
        run_badge(&r, facts[0].as_ref(), 1000),
        "paddock · sample-notes · ask · vm · <1m · display changed, restart for full size"
    );
    // A session record of another shim pid gives no facts.
    s.pid = 9;
    let facts = crate::mark::vm::match_facts(std::slice::from_ref(&r), [&s].into_iter());
    assert_eq!(facts, vec![None]);
    assert_eq!(
        run_badge(&r, None, 1000),
        "paddock · sample-notes · ask · vm · <1m"
    );
}
