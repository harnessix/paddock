use super::*;

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

#[test]
fn panes_split_at_a_lone_plus() {
    let p = parse(&words(
        "run claude -- --resume + shell + board + view 1a2b3c --write",
    ))
    .unwrap();
    assert_eq!(p.len(), 4);
    assert_eq!(p[0].kind, Kind::Run);
    assert_eq!(p[0].args, words("claude -- --resume"));
    assert_eq!(p[1].kind, Kind::Shell);
    assert_eq!(p[2].kind, Kind::Board);
    assert_eq!(p[3].kind, Kind::View);
    assert_eq!(p[3].args, words("1a2b3c --write"));
}

#[test]
fn bad_panes_say_what_is_wrong() {
    let err = |s: &str| parse(&words(s)).unwrap_err().to_string();
    assert!(err("run").contains("needs a harness"));
    assert!(err("run --backend bwrap").contains("needs a harness"));
    assert!(err("view").contains("one sandbox id"));
    assert!(err("view a b").contains("one sandbox id"));
    assert!(err("shell extra").contains("takes no arguments"));
    assert!(err("board --json").contains("takes no arguments"));
    assert!(err("htop").contains("is not a kind of pane"));
    assert!(err("shell + + board").contains("a pane is empty"));
    assert!(err("shell +").contains("a pane is empty"));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn a_mac_pane_needs_macos() {
    let e = parse(&words("mac")).unwrap_err().to_string();
    assert!(e.contains("needs macOS"), "{e}");
}

#[cfg(target_os = "macos")]
#[test]
fn a_mac_pane_takes_an_app_session() {
    assert_eq!(parse(&words("mac safari")).unwrap()[0].kind, Kind::Mac);
    assert!(parse(&words("mac a b")).is_err());
}

#[test]
fn titles_name_the_side_first_and_carry_no_escapes() {
    assert_eq!(
        title(Side::Sandbox, "claude", "claude-app-1a2b3c"),
        "sandbox · claude · claude-app-1a2b3c"
    );
    assert_eq!(title(Side::Host, "shell", "~/src"), "host · shell · ~/src");
    // A folder name cannot carry a tmux style or a terminal escape.
    let t = title(Side::Host, "shell", "~/a#[fg=red]\x1b]2;x\x07");
    assert!(
        !t.contains('#') && !t.contains('\x1b') && !t.contains('\x07'),
        "{t}"
    );
    assert!(t.starts_with("host · shell · "), "{t}");
    // A long folder keeps the side word, its start and its end, and loses
    // its middle (review J 1).
    let long = format!("/{}end", "d/".repeat(200));
    let t = title(Side::Host, "shell", &long);
    assert!(t.chars().count() <= TITLE_CHARS, "{t}");
    assert!(t.starts_with("host · shell · /d/"), "{t}");
    assert!(t.ends_with("d/end") && t.contains('…'), "{t}");
}

/// Review J 1: a folder over 81 characters cut the side word off a host
/// header, so only the color told the host from a sandbox.
#[test]
fn review_j_a_long_title_never_loses_its_side_word() {
    let deep = format!(
        "~/{}/sandbox · claude · ab-cd-ef",
        "very-long-folder-name".repeat(5)
    );
    let t = title(Side::Host, "shell", &deep);
    assert!(t.starts_with("host · shell · ~/very-long"), "{t}");
    assert!(t.ends_with("sandbox · claude · ab-cd-ef"), "{t}");
    assert!(t.chars().count() <= TITLE_CHARS, "{t}");
    // A long harness name and a long id keep the side word too.
    let t = title(Side::Sandbox, &"h".repeat(300), &"i".repeat(300));
    assert!(t.starts_with("sandbox · hhh"), "{t}");
    assert!(t.chars().count() <= TITLE_CHARS, "{t}");
    // clean() on a label, as label_args uses it, keeps the start.
    let c = clean(&format!("host · shell · {}", "x".repeat(300)));
    assert!(c.starts_with("host · shell · ") && c.ends_with('…'), "{c}");
    assert_eq!(c.chars().count(), TITLE_CHARS);
    // Short text stays as it is.
    assert_eq!(middle("abc", 3), "abc");
    assert_eq!(middle("abcdef", 4), "a…ef");
}

/// Review J 7: the status line names the side of the active pane, from
/// paddock's label. A pane can draw a fake border inside itself, but no
/// pane draws the status line.
#[test]
fn review_j_the_status_line_names_the_side_of_the_active_pane() {
    let left = status_left();
    assert!(
        left.contains("#{?@paddock_side,#{@paddock_side},host}"),
        "{left}"
    );
    assert!(left.contains(HOST_COLOR), "{left}");
    assert!(
        !left.contains("pane_title") && !left.contains("#T"),
        "{left}"
    );
    let a = style_args(None);
    assert!(a.windows(2).any(|w| w[0] == "status-left" && w[1] == left));
}

/// Review J 4: the split lays out again when a terminal attaches or
/// changes its size, through a hook with plain paths only.
#[test]
fn review_j_the_layout_follows_the_terminal() {
    let t = Tmux {
        exe: "/usr/bin/tmux".into(),
        sock: "/run/user/1000/paddock/view.sock".into(),
        vars: vec![],
        scope: None,
    };
    let hook = layout_hook(Path::new("/usr/bin/paddock"), &t).unwrap();
    assert_eq!(
        hook,
        "/usr/bin/paddock split-layout /usr/bin/tmux /run/user/1000/paddock/view.sock >/dev/null 2>&1 || true"
    );
    let a = style_args(Some(&hook));
    for h in ["client-attached", "client-resized"] {
        assert!(
            a.windows(2)
                .any(|w| w[0] == h && w[1] == format!("run-shell -b '{hook}'")),
            "{h} missing in {a:?}"
        );
    }
    // A path that tmux or the shell would read differently: no hook.
    for bad in [
        "/a b/paddock",
        "/a'b/paddock",
        "/a#{x}/paddock",
        "rel/paddock",
        "/a$b",
    ] {
        assert_eq!(layout_hook(Path::new(bad), &t), None, "{bad}");
    }
    assert!(!style_args(None).iter().any(|x| x == "client-resized"));
}

/// A narrow terminal stacks the panes, the board among them.
#[test]
fn review_j_an_80_column_terminal_stacks_the_board() {
    assert_eq!(layout(3, 80, Some(1)), Layout::Stack);
    assert_eq!(layout(5, 80, Some(1)), Layout::Tiled);
}

#[test]
fn folders_below_home_start_with_a_tilde() {
    let home = Path::new("/home/demo");
    assert_eq!(folder(home, Path::new("/home/demo")), "~");
    assert_eq!(folder(home, Path::new("/home/demo/src/app")), "~/src/app");
    assert_eq!(folder(home, Path::new("/srv/x")), "/srv/x");
}

#[test]
fn the_border_reads_only_paddock_options() {
    let f = border_format();
    assert!(
        f.contains("#{@paddock_title}") && f.contains("#{@paddock_color}"),
        "{f}"
    );
    // pane_title is what an OSC title sets: the border never shows it.
    assert!(!f.contains("pane_title") && !f.contains("#T"), "{f}");
    // A pane without a label is a host shell in the host color.
    assert!(f.contains(UNLABELED) && f.contains(HOST_COLOR), "{f}");
    assert_ne!(SANDBOX_COLOR, HOST_COLOR);
}

#[test]
fn the_status_line_has_the_keys_and_no_host_or_clock() {
    let a = style_args(None);
    let joined = a.join(" ");
    assert!(joined.contains(KEYS));
    for v in ["#H", "#h", "%H", "%M", "#T", "pane_title", "host_short"] {
        assert!(
            !session_options().iter().any(|(_, x)| x.contains(v)),
            "{v} in the status"
        );
    }
    for (k, v) in [
        ("set-titles", "off"),
        ("allow-rename", "off"),
        ("automatic-rename", "off"),
        ("mouse", "on"),
    ] {
        assert!(
            a.windows(2).any(|w| w[0] == k && w[1] == v),
            "{k} {v} missing in {joined}"
        );
    }
    // Every option targets the split, never the global options of the
    // server that the views share.
    assert!(!a.iter().any(|x| x == "-g"), "{joined}");
    assert!(a.iter().any(|x| x == "after-new-window"));
}

fn job(side: Side) -> Job {
    Job {
        side,
        title: "sandbox · fake · starting".into(),
        argv: vec!["/bin/true".into()],
        cwd: "/".into(),
        env: vec![],
        keep_tmux: false,
        kind: Kind::Run,
        watch: None,
        passthrough: false,
        tmux: "/usr/bin/tmux".into(),
        sock: "/run/x/view.sock".into(),
    }
}

#[test]
fn a_label_sets_the_pane_options_of_one_pane() {
    let a = label_args("%7", &job(Side::Sandbox));
    let sets: Vec<&[String]> = a.split(|x| x == ";").collect();
    assert!(sets
        .iter()
        .all(|c| c[..5] == s(&["set-option", "-q", "-p", "-t", "%7"])[..]));
    let val = |k: &str| {
        sets.iter()
            .find(|c| c[5] == k)
            .map(|c| c[6].clone())
            .unwrap_or_default()
    };
    assert_eq!(val("@paddock_side"), "sandbox");
    assert_eq!(val("@paddock_color"), SANDBOX_COLOR);
    assert_eq!(val("remain-on-exit"), "on");
    assert_eq!(val("allow-passthrough"), "off");
    let host = label_args("%8", &job(Side::Host));
    assert!(host
        .windows(2)
        .any(|w| w[0] == "@paddock_color" && w[1] == HOST_COLOR));
    assert!(host
        .windows(2)
        .any(|w| w[0] == "remain-on-exit" && w[1] == "off"));
}

#[test]
fn the_caller_pane_counts_only_on_paddocks_socket() {
    let t = Tmux {
        exe: "/usr/bin/tmux".into(),
        sock: "/run/user/1/paddock/view.sock".into(),
        vars: vec![],
        scope: None,
    };
    let ours = Some("/run/user/1/paddock/view.sock,123,0");
    let users = Some("/tmp/tmux-1/default,99,0");
    assert_eq!(caller_pane(&t, ours, Some("%3")).as_deref(), Some("%3"));
    assert_eq!(caller_pane(&t, users, Some("%3")), None);
    assert_eq!(caller_pane(&t, None, Some("%3")), None);
    assert_eq!(caller_pane(&t, ours, Some("3")), None);
    assert!(in_paddock_tmux(&t, ours));
    assert!(!in_paddock_tmux(&t, users));
    assert!(!in_paddock_tmux(&t, None));
}

#[test]
fn job_files_are_private_and_read_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "XDG_RUNTIME_DIR".to_string(),
        tmp.path().display().to_string(),
    );
    let env = HostEnv::from_parts(tmp.path().into(), "t".into(), 1000, 1000, vars);
    let j = job(Side::Host);
    let f = write_job(&env, &j).unwrap();
    assert_eq!(std::fs::metadata(&f).unwrap().mode() & 0o077, 0);
    assert_eq!(take_job(&f).unwrap(), j);
    assert!(!f.exists());
    assert!(take_job(&f).is_err());
    // A file that others can read is not a job.
    let open = tmp.path().join("open.job");
    std::fs::write(&open, serde_json::to_vec(&j).unwrap()).unwrap();
    std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    assert!(take_job(&open).is_err());
}

#[test]
fn a_process_descends_from_itself_and_its_parents() {
    let me = std::process::id();
    assert!(descends(me, me));
    let parent = parent_pid(me).unwrap();
    assert!(descends(me, parent));
    assert!(!descends(parent, me));
}

#[test]
fn the_split_session_is_never_a_stale_view() {
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().into(),
        "t".into(),
        1000,
        1000,
        Default::default(),
    );
    let sessions = vec![SESSION.to_string(), "fake-p-123456".to_string()];
    let stale = super::super::stale(&env, &sessions, &[], 1_000_000);
    assert_eq!(stale, vec!["fake-p-123456".to_string()]);
}

#[test]
fn a_board_sits_on_the_left_only_in_a_wide_window() {
    assert_eq!(
        layout(3, 200, Some(2)),
        Layout::BoardLeft {
            board: 2,
            width: 60
        }
    );
    // Narrow: the board is one pane among the others.
    assert_eq!(layout(3, WIDE - 1, Some(0)), Layout::Stack);
    // Alone, the board takes the whole window.
    assert_eq!(layout(1, 300, Some(0)), Layout::Stack);
    assert_eq!(layout(2, 300, None), Layout::Stack);
    assert_eq!(layout(4, 300, None), Layout::Tiled);
    // The board keeps room for its lines.
    assert_eq!(
        layout(2, WIDE, Some(1)),
        Layout::BoardLeft {
            board: 1,
            width: 42
        }
    );
}

#[test]
fn badges_follow_the_board_states() {
    use crate::board::State as S;
    assert_eq!(badge(Some(S::Working)), "running");
    for st in [S::Waiting, S::Blocked, S::Stuck, S::Idle, S::Done] {
        assert_eq!(badge(Some(st)), "waits for you", "{st:?}");
    }
    assert_eq!(badge(Some(S::Ended)), "ended");
    assert_eq!(badge(None), "ended");
    assert_eq!(
        badge_args("%2", "ended"),
        s(&[
            "set-option",
            "-q",
            "-p",
            "-t",
            "%2",
            "@paddock_badge",
            "ended"
        ])
    );
    // The border shows the badge after the title, in brackets.
    assert!(border_format().contains("[#{@paddock_badge}]"));
}
