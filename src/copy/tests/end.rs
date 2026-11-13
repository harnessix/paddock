//! The end of a run on a copy, the cleanup of old copies, and what
//! `paddock ps` learns of a copy.

use super::*;
use crate::copy::backup;
use crate::copy::config::Apply;
use crate::copy::end::{self, End, Ended};

fn set_auto(w: &World) {
    let f = w.env.paddock_config().join("config.toml");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, "[copy]\napply = \"auto\"\n").unwrap();
    assert_eq!(Config::load(&w.env).unwrap().apply, Apply::Auto);
}

fn quiet(auto_approvals: bool, launcher: bool) -> End {
    End {
        auto_approvals,
        launcher,
        quiet: true,
    }
}

fn never(q: &str) -> Option<bool> {
    panic!("unexpected question: {q}")
}

#[test]
fn a_run_without_a_copy_and_a_copy_without_a_change() {
    let w = world();
    assert_eq!(
        end::of_run(
            &w.env,
            "claude-webshop-000000",
            quiet(true, false),
            &mut never
        ),
        Ended::NoCopy
    );
    let run = w.start("claude-webshop-1a2b3c");
    assert_eq!(
        end::of_run(&w.env, &run.rec.id, quiet(true, false), &mut never),
        Ended::Clean
    );
    // The copy is gone, the record says when the run ended, and the backup
    // of the start stays.
    let run = Run::load(&run.store, &run.rec.id).unwrap();
    assert!(!run.has_tree());
    assert!(run.rec.ended.is_some() && run.rec.removed.is_some());
    assert_eq!(backup::list(&run.store).len(), 1);
}

#[test]
fn open_changes_wait_for_the_user_and_the_folder_stays() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "new.txt", "new\n");
    put(&run.tree(), "src/price.ts", "the copy\n");
    put(&w.project, "src/price.ts", "the user\n");
    assert_eq!(
        end::of_run(&w.env, &run.rec.id, quiet(true, false), &mut never),
        Ended::Open(3, 1)
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert!(!w.project.join("new.txt").exists());
    let run = Run::load(&run.store, &run.rec.id).unwrap();
    assert!(run.has_tree() && run.rec.ended.is_some());
    let text = end::preview(&status(&run), 2);
    assert_eq!(
        text,
        "  + new       new.txt\n  ~ changed   src/cart.ts\n  and 1 more\n"
    );
    assert!(end::preview(&status(&run), 10).contains("  ~ conflict  src/price.ts\n"));
}

#[test]
fn the_setting_applies_only_with_auto_approvals_and_without_a_conflict() {
    let w = world();
    set_auto(&w);
    // Manual approvals: no automatic apply.
    let run = w.start("claude-webshop-000001");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    assert_eq!(
        end::of_run(&w.env, &run.rec.id, quiet(false, false), &mut never),
        Ended::Open(1, 0)
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    // Auto approvals: applied, after a backup.
    assert_eq!(
        end::of_run(&w.env, &run.rec.id, quiet(true, false), &mut never),
        Ended::Applied(1)
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    let kinds: Vec<backup::Kind> = backup::list(&run.store)
        .iter()
        .map(|b| b.meta.kind)
        .collect();
    assert_eq!(kinds, [backup::Kind::Apply, backup::Kind::Start]);
    // A conflict: nothing is applied, also not the files without one.
    let run = w.start("claude-webshop-000002");
    put(&run.tree(), "src/cart.ts", "the copy, again\n");
    put(&w.project, "src/cart.ts", "the user\n");
    put(&run.tree(), "src/price.ts", "price 2\n");
    assert_eq!(
        end::of_run(&w.env, &run.rec.id, quiet(true, false), &mut never),
        Ended::Open(2, 1)
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "price\n");
}

#[test]
fn a_launcher_run_asks_at_its_end_and_applies_only_on_yes() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    let mut asked = Vec::new();
    let r = end::of_run(&w.env, &run.rec.id, quiet(true, true), &mut |q| {
        asked.push(q.to_string());
        Some(false)
    });
    assert_eq!(r, Ended::Open(1, 0));
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].starts_with("Apply 1 change(s) to "),
        "{}",
        asked[0]
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    // Nobody to answer: nothing is applied.
    let r = end::of_run(&w.env, &run.rec.id, quiet(true, true), &mut |_| None);
    assert_eq!(r, Ended::Open(1, 0));
    let r = end::of_run(&w.env, &run.rec.id, quiet(true, true), &mut |_| Some(true));
    assert_eq!(r, Ended::Applied(1));
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    // A run from a terminal never asks: the user has the commands.
    let run = w.start("claude-webshop-222222");
    put(&run.tree(), "src/price.ts", "price 2\n");
    assert_eq!(
        end::of_run(&w.env, &run.rec.id, quiet(true, false), &mut never),
        Ended::Open(1, 0)
    );
}

#[test]
fn old_copies_without_an_open_change_go_and_one_with_a_change_never_does() {
    let w = world();
    let mut applied = w.start("claude-webshop-000001");
    put(&applied.tree(), "src/cart.ts", "the copy\n");
    w.apply_all(&mut applied);
    let open = w.start("claude-webshop-000002");
    put(&open.tree(), "src/price.ts", "price 2\n");
    let live = w.start("claude-webshop-000003");
    for id in [&applied.rec.id, &open.rec.id] {
        end::of_run(&w.env, id, quiet(false, false), &mut never);
    }
    let now = crate::state::now();
    // Too early: an undo of the apply must still show the changes.
    assert_eq!(
        end::sweep(&w.env, &applied.store, now + end::KEEP_CLEAN - 60),
        0
    );
    assert_eq!(
        end::sweep(&w.env, &applied.store, now + end::KEEP_CLEAN + 60),
        1
    );
    let load = |r: &Run| Run::load(&r.store, &r.rec.id).unwrap();
    assert!(!load(&applied).has_tree());
    assert!(load(&open).has_tree(), "open changes are never removed");
    assert!(
        load(&live).has_tree(),
        "a run that did not end keeps its copy"
    );
    assert_eq!(
        get(&load(&open).tree(), "src/price.ts").unwrap(),
        "price 2\n"
    );
}

#[test]
fn ps_learns_the_folder_and_the_copy_of_a_run() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    assert_eq!(
        crate::copy::run::summary(&w.env, &run.rec.id),
        Some((w.project.clone(), run.tree()))
    );
    assert_eq!(
        crate::copy::run::summary(&w.env, "claude-webshop-ffffff"),
        None
    );
    assert_eq!(crate::copy::run::summary(&w.env, "../x"), None);
    assert_eq!(crate::copy::start::work_path(&run), w.project);
}
