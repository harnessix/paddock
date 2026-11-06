use super::*;
use std::path::PathBuf;

fn running(network: &str, gui: &str) -> Running {
    serde_json::from_value(serde_json::json!({
        "id": "vscode-api-1a2b",
        "harness": "vscode",
        "project": "/work/api",
        "backend": "seatbelt",
        "network": network,
        "gui": gui,
        "pid": 4242,
        "started": 1000,
        "auth": "isolated",
    }))
    .unwrap()
}

#[test]
fn proxy_only_modes_are_sage_and_the_rest_amber() {
    for n in ["none", "api", "egress", "allowlist", "ask"] {
        assert_eq!(Tone::of(n), Tone::Sage, "{n}");
    }
    for n in ["host", "", "something-new"] {
        assert_eq!(Tone::of(n), Tone::Amber, "{n}");
    }
    assert_ne!(Tone::Sage.rgb(), Tone::Amber.rgb());
}

#[test]
fn tone_ask_is_sage() {
    // Mode ask exists only in the macOS guest, and every connection of the
    // guest goes through paddock's proxy.
    assert_eq!(Tone::of("ask"), Tone::Sage);
    assert_eq!(Tone::of("ask").word(), "sage");
    assert_eq!(Tone::Amber.word(), "amber");
}

#[test]
fn badge_vm_and_input_source() {
    let mut r = running("ask", "app");
    r.backend = "macvm".into();
    r.harness = "chatgpt-desktop".into();
    // A guest session gains the word vm.
    assert_eq!(
        run_badge(&r, None, 1000 + 7200),
        "paddock · chatgpt-desktop · ask · vm · 2h"
    );
    // The guest input source name while it differs from the host's.
    let facts = VmFacts {
        display_name: "ChatGPT (paddock)".into(),
        guest_input: Some("Pinyin - Simplified".into()),
        note: None,
    };
    assert_eq!(
        run_badge(&r, Some(&facts), 1000 + 7200),
        "paddock · chatgpt-desktop · ask · vm · Pinyin - Simplified · 2h"
    );
    // Equal sources: no name.
    let same = VmFacts {
        guest_input: None,
        ..facts.clone()
    };
    assert!(!run_badge(&r, Some(&same), 1000).contains("Pinyin"));
    // The menu entry names the app.
    assert_eq!(
        run_menu_title(&r, Some(&facts), 1000 + 60, 1, 1, None),
        "ChatGPT (paddock) · api · ask · vm · 1m"
    );
    // A Seatbelt run keeps its badge and menu entry, with no vm.
    let seatbelt = running("egress", "app");
    assert_eq!(
        run_badge(&seatbelt, Some(&facts), 1000 + 7200),
        "paddock · vscode · egress · 2h"
    );
    assert_eq!(
        run_menu_title(&seatbelt, Some(&facts), 1100, 1, 1, None),
        menu_title(&seatbelt, 1100, 1, 1, None)
    );
}

#[test]
fn ages_are_short() {
    assert_eq!(age(0), "<1m");
    assert_eq!(age(59), "<1m");
    assert_eq!(age(60), "1m");
    assert_eq!(age(3599), "59m");
    assert_eq!(age(7200), "2h");
    assert_eq!(age(86_399), "23h");
    assert_eq!(age(3 * 86_400 + 5), "3d");
}

#[test]
fn badge_text_matches_omarchy() {
    assert_eq!(
        badge_text("vscode", "egress", 2 * 3600 + 10),
        "paddock · vscode · egress · 2h"
    );
}

#[test]
fn the_badge_names_the_pasture_if_there_is_one() {
    let mut r = running("egress", "app");
    assert_eq!(badge_name(&r), "vscode");
    r.pasture = Some("api-team".into());
    assert_eq!(badge_name(&r), "api-team");
    assert_eq!(
        details(&r, 1000, None)[0],
        "paddock · api-team · egress · <1m"
    );
}

#[test]
fn details_name_the_sandbox() {
    let r = running("egress", "app");
    let d = details(&r, 1000 + 7200, None);
    assert_eq!(d[0], "paddock · vscode · egress · 2h");
    assert!(d.contains(&"Project: /work/api".to_string()));
    assert!(d.contains(&"Network: egress, only through paddock's proxy".to_string()));
    assert!(d.contains(&"Login: isolated".to_string()));
    assert!(d.contains(&"Sandbox: vscode-api-1a2b".to_string()));
    assert!(d.contains(&"Process: 4242".to_string()));
    let d = details(&running("host", "app"), 1000, None);
    assert!(d.contains(&"Network: host, a wider network".to_string()));
    assert!(!d.iter().any(|l| l.starts_with("Terminal run")));
    let d = details(&running("egress", "off"), 1000, Some("Terminal"));
    assert!(d.contains(&"Terminal run in Terminal".to_string()));
}

#[test]
fn menu_titles_count_windows_only_when_there_are_several() {
    let r = running("api", "app");
    assert_eq!(menu_title(&r, 1100, 1, 1, None), "vscode · api · api · 1m");
    assert_eq!(
        menu_title(&r, 1100, 2, 3, None),
        "vscode · api · api · 1m (2/3)"
    );
    assert_eq!(
        menu_title(&r, 1100, 1, 1, Some("Terminal")),
        "vscode · api · api · 1m (Terminal)"
    );
    let mut r = r;
    r.project = PathBuf::from("/");
    assert_eq!(project_name(&r), "/");
}

fn terminal_run(window: Option<u32>) -> Running {
    Running {
        terminal_window: window,
        terminal_pid: window.map(|_| 530),
        ..running("egress", "off")
    }
}

#[test]
fn app_runs_and_terminal_runs_with_a_window_get_marks() {
    let list = vec![
        running("egress", "app"),
        terminal_run(None),
        terminal_run(Some(812)),
        // Window mode has its own window, never a terminal mark.
        Running {
            terminal_window: Some(9),
            ..running("egress", "window")
        },
    ];
    let marked = marked_runs(list);
    let got: Vec<(&str, Option<u32>)> = marked
        .iter()
        .map(|r| (r.gui.as_str(), r.terminal_window))
        .collect();
    assert_eq!(got, vec![("app", None), ("off", Some(812))]);
    let procs: ProcTable = [(4242, 1), (4243, 4242)].into_iter().collect();
    assert_eq!(
        Target::of(&marked[0], &procs),
        Target::Tree(BTreeSet::from([4242, 4243]))
    );
    assert_eq!(Target::of(&marked[1], &procs), Target::Window(812, 530));
}

#[test]
fn the_tree_holds_all_descendants_and_nothing_else() {
    // launchd 1 -> 100 (sandbox) -> 101 -> 103, 100 -> 102. 200 is apart.
    let procs: ProcTable = [
        (1, 0),
        (100, 1),
        (101, 100),
        (102, 100),
        (103, 101),
        (200, 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(tree(&procs, 100), BTreeSet::from([100, 101, 102, 103]));
    // A process that is gone from the table still counts as its own root.
    assert_eq!(tree(&procs, 999), BTreeSet::from([999]));
    // A loop in a broken table ends.
    let looped: ProcTable = [(5, 6), (6, 5)].into_iter().collect();
    assert_eq!(tree(&looped, 5), BTreeSet::from([5, 6]));
}

fn win(id: u32, pid: u32, layer: i64, w: f64) -> Win {
    Win {
        id,
        pid,
        layer,
        bounds: Rect::new(10.0, 50.0, w, w),
        alpha: 1.0,
    }
}

#[test]
fn windows_match_by_process_tree() {
    let trees = vec![
        Target::Tree(BTreeSet::from([100, 101])),
        Target::Tree(BTreeSet::from([300])),
    ];
    let wins = vec![
        win(1, 101, 0, 800.0), // sandbox 0, from a child process
        win(2, 555, 0, 800.0), // not protected
        win(3, 100, 8, 800.0), // a menu layer
        win(4, 100, 0, 20.0),  // a tiny helper window
        win(5, 300, 0, 400.0), // sandbox 1
        win(6, 9, 0, 400.0),   // the marker itself
        Win {
            alpha: 0.0,
            ..win(7, 100, 0, 800.0)
        }, // invisible
    ];
    let m = match_windows(&wins, &trees, 9);
    let got: Vec<(u32, usize)> = m.iter().map(|(w, i)| (w.id, *i)).collect();
    assert_eq!(got, vec![(1, 0), (5, 1)]);
}

#[test]
fn a_window_target_matches_by_number_and_owner() {
    // Terminal 530 with three windows; the run is in window 12.
    let targets = vec![Target::Tree(BTreeSet::from([100])), Target::Window(12, 530)];
    let wins = vec![
        win(11, 530, 0, 800.0), // another window of the terminal
        win(12, 530, 0, 800.0), // the run's window
        win(13, 530, 0, 800.0),
        win(12, 777, 0, 800.0), // the same number from another owner
        win(4, 100, 0, 800.0),  // an app run as before
    ];
    let got: Vec<(u32, u32, usize)> = match_windows(&wins, &targets, 9)
        .iter()
        .map(|(w, i)| (w.id, w.pid, *i))
        .collect();
    assert_eq!(got, vec![(12, 530, 1), (4, 100, 0)]);
    // A closed window: nothing to draw.
    assert!(match_windows(&wins[2..3], &targets[1..], 9).is_empty());
}

/// launchd 1 -> Terminal 530 -> login 531 -> zsh 532 -> paddock 600.
/// tmux server 700 -> zsh 701 -> paddock 702. Safari 800.
fn table() -> ProcTable {
    [
        (530, 1),
        (531, 530),
        (532, 531),
        (600, 532),
        (700, 1),
        (701, 700),
        (702, 701),
        (800, 1),
    ]
    .into_iter()
    .collect()
}

#[test]
fn the_ancestors_end_at_launchd_or_a_loop() {
    assert_eq!(ancestors(&table(), 600, 32), vec![600, 532, 531, 530]);
    assert_eq!(ancestors(&table(), 600, 2), vec![600, 532, 531]);
    assert_eq!(ancestors(&table(), 999, 32), vec![999]);
    let looped: ProcTable = [(5, 6), (6, 5)].into_iter().collect();
    assert_eq!(ancestors(&looped, 5, 32), vec![5, 6]);
}

#[test]
fn the_terminal_window_is_the_front_window_of_the_nearest_terminal() {
    let wins = vec![
        win(40, 530, 25, 800.0), // a panel of the terminal
        win(41, 530, 0, 30.0),   // a tiny helper window
        win(42, 530, 0, 800.0),  // the front window
        win(43, 530, 0, 800.0),
        win(50, 800, 0, 800.0),
    ];
    assert_eq!(
        terminal_window(&table(), 600, &wins, Some(530)),
        Ok(TerminalWindow { id: 42, pid: 530 })
    );
    // The terminal is not in front: no guess.
    assert_eq!(
        terminal_window(&table(), 600, &wins, Some(800)),
        Err(NoMark::NotFront {
            terminal: 530,
            front: Some(800)
        })
    );
    // Under tmux no ancestor owns a window.
    assert_eq!(
        terminal_window(&table(), 702, &wins, Some(530)),
        Err(NoMark::NoTerminal)
    );
    // Only small windows: no terminal.
    let small = vec![win(41, 530, 0, 30.0)];
    assert_eq!(
        terminal_window(&table(), 600, &small, Some(530)),
        Err(NoMark::NoTerminal)
    );
    // A nearer ancestor with a window wins over a farther one.
    let mut procs = table();
    procs.insert(530, 900);
    let wins = vec![win(60, 900, 0, 800.0), win(42, 530, 0, 800.0)];
    assert_eq!(
        terminal_window(&procs, 600, &wins, Some(530)),
        Ok(TerminalWindow { id: 42, pid: 530 })
    );
}

#[test]
fn the_reason_names_the_terminal_and_the_front_app() {
    let name = |p: u32| match p {
        530 => "Terminal".to_string(),
        800 => "Safari".to_string(),
        p => format!("process {p}"),
    };
    let e = NoMark::NotFront {
        terminal: 530,
        front: Some(800),
    };
    assert_eq!(
        e.reason(name),
        "no window mark: Terminal is not the front app at the start, Safari is"
    );
    let e = NoMark::NotFront {
        terminal: 530,
        front: None,
    };
    assert_eq!(
        e.reason(name),
        "no window mark: Terminal is not the front app at the start"
    );
    assert!(NoMark::NoTerminal
        .reason(name)
        .starts_with("no window mark: "));
}

#[test]
fn the_focused_window_is_the_front_window_of_the_front_app() {
    let wins = vec![
        win(1, 7, 25, 800.0),
        win(2, 5, 0, 800.0),
        win(3, 5, 0, 800.0),
    ];
    assert_eq!(focused_window(&wins, Some(5)), Some(2));
    assert_eq!(focused_window(&wins, Some(7)), None);
    assert_eq!(focused_window(&wins, None), None);
}

#[test]
fn the_frame_sits_outside_the_window() {
    let f = frame_rect(Rect::new(100.0, 100.0, 800.0, 600.0));
    assert!(f.x < 100.0 && f.y < 100.0);
    assert!(f.x + f.w > 900.0 && f.y + f.h > 700.0);
    assert_eq!(f.w - 800.0, 2.0 * FRAME_WIDTH);
}

#[test]
fn flipping_moves_the_origin_to_the_bottom() {
    let r = Rect::new(10.0, 20.0, 100.0, 50.0);
    assert_eq!(r.flip(1000.0), Rect::new(10.0, 930.0, 100.0, 50.0));
    assert_eq!(r.flip(1000.0).flip(1000.0), r);
}

#[test]
fn the_badge_goes_above_the_window_when_there_is_room() {
    // A screen of 1440x900 with a 30 pt menu bar.
    let screens = [Rect::new(0.0, 30.0, 1440.0, 870.0)];
    let win = Rect::new(100.0, 200.0, 800.0, 500.0);
    let b = badge_rect(win, 220.0, 22.0, &screens);
    assert!(b.y + b.h < win.y, "above the window: {b:?}");
    assert_eq!(b.x + b.w, win.x + win.w - BADGE_INSET);
    // A window at the top of the screen: the badge goes inside, at the
    // bottom right, never over the menu bar.
    let full = Rect::new(0.0, 30.0, 1440.0, 870.0);
    let b = badge_rect(full, 220.0, 22.0, &screens);
    assert_eq!(b.y + b.h, full.y + full.h - BADGE_INSET);
    assert!(b.y > full.y);
}

#[test]
fn the_badge_finds_the_screen_of_its_window() {
    // A second screen to the right, without a menu bar, top at 0.
    let screens = [
        Rect::new(0.0, 30.0, 1440.0, 870.0),
        Rect::new(1440.0, 0.0, 1920.0, 1080.0),
    ];
    let win = Rect::new(1500.0, 10.0, 800.0, 500.0);
    let b = badge_rect(win, 200.0, 22.0, &screens);
    assert!(b.y > win.y, "no room above on the second screen: {b:?}");
    let win = Rect::new(1500.0, 100.0, 800.0, 500.0);
    let b = badge_rect(win, 200.0, 22.0, &screens);
    assert!(b.y < win.y);
}

#[test]
fn the_badge_shows_on_focus_for_a_while_and_on_hover() {
    assert!(badge_visible(Some(0.0), false));
    assert!(badge_visible(Some(BADGE_FOCUS_SECS - 0.1), false));
    assert!(!badge_visible(Some(BADGE_FOCUS_SECS), false));
    assert!(badge_visible(Some(60.0), true));
    assert!(badge_visible(None, true));
    assert!(!badge_visible(None, false));
}

#[test]
fn near_frames_compare_with_a_tolerance() {
    let a = Rect::new(10.0, 10.0, 100.0, 100.0);
    assert!(a.near(Rect::new(11.0, 9.0, 101.0, 100.0), 2.0));
    assert!(!a.near(Rect::new(20.0, 10.0, 100.0, 100.0), 2.0));
}

#[cfg(target_os = "macos")]
#[test]
fn the_process_table_holds_this_process_and_its_parent() {
    let t = procs::proc_table();
    assert_eq!(
        t.get(&std::process::id()).copied(),
        Some(std::os::unix::process::parent_id())
    );
    // The chain of a test process ends at launchd within the limit.
    let chain = ancestors(&t, std::process::id(), TERMINAL_DEPTH);
    assert!(chain.len() >= 2, "{chain:?}");
}

#[test]
fn a_run_that_is_not_its_own_terminal_gets_no_window() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::env::HostEnv::from_parts(
        tmp.path().join("home"),
        "demo".into(),
        1000,
        1000,
        Default::default(),
    );
    assert_eq!(own_terminal_window(&env, false), None);
}

#[test]
fn the_firewall_panel_sits_under_the_badge_corner_of_its_window() {
    let screens = [
        Rect::new(0.0, 25.0, 1440.0, 875.0),
        Rect::new(1440.0, 0.0, 1920.0, 1080.0),
    ];
    let size = PANEL_SIZE;
    // Under the top-right corner of the window, inside its screen.
    let win = Rect::new(100.0, 100.0, 800.0, 600.0);
    let r = panel_rect(Some(win), size, &screens);
    assert_eq!((r.w, r.h), size);
    assert_eq!(r.x, 100.0 + 800.0 - size.0 - BADGE_INSET);
    assert_eq!(r.y, 100.0 + BADGE_INSET + 22.0);
    // A window at the right edge: the panel stays on the screen.
    let edge = Rect::new(1300.0, 100.0, 800.0, 600.0);
    let r = panel_rect(Some(edge), size, &screens);
    assert!(r.x + r.w <= 1440.0 + 1920.0);
    // A window on the second screen keeps the panel there.
    let second = Rect::new(2000.0, 50.0, 800.0, 600.0);
    let r = panel_rect(Some(second), size, &screens);
    assert!(r.x >= 1440.0 && r.y >= 0.0);
    // Without a window: the top-right of the main screen.
    let r = panel_rect(None, size, &screens);
    assert_eq!(r.x, 1440.0 - size.0 - BADGE_INSET);
    assert_eq!(r.y, 25.0 + BADGE_INSET);
    // No screens at all: something, not a panic.
    let r = panel_rect(None, size, &[]);
    assert_eq!((r.w, r.h), size);
}
