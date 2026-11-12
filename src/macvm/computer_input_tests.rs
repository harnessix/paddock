//! The input mapping: keys, combos and the plans of the actions.

use super::*;

fn p(x: f64, y: f64) -> Point {
    Point { x, y }
}

#[test]
fn keys_by_name_and_by_character() {
    assert_eq!(key_code("a"), Some((0, Some('a'))));
    assert_eq!(key_code("A"), Some((0, Some('a'))));
    assert_eq!(key_code("z"), Some((6, Some('z'))));
    assert_eq!(key_code("5"), Some((23, Some('5'))));
    assert_eq!(key_code("/"), Some((44, Some('/'))));
    assert_eq!(key_code("slash"), Some((44, Some('/'))));
    assert_eq!(key_code("Return"), Some((36, None)));
    assert_eq!(key_code("page_down"), Some((121, None)));
    assert_eq!(key_code("space"), Some((49, Some(' '))));
    assert_eq!(key_code("f12"), Some((111, None)));
    assert_eq!(key_code("plus"), Some((24, Some('='))));
    assert_eq!(key_code("hyper"), None);
    assert_eq!(key_code(""), None);
}

#[test]
fn every_letter_and_digit_has_its_own_code() {
    let mut seen = std::collections::BTreeSet::new();
    for c in ('a'..='z').chain('0'..='9') {
        let (code, ch) = key_code(&c.to_string()).unwrap();
        assert_eq!(ch, Some(c));
        assert!(seen.insert(code), "{c}");
    }
}

#[test]
fn combos_parse_with_their_modifiers_sorted() {
    let c = parse_combo("Shift+Cmd+T").unwrap();
    assert_eq!(c.mods, vec![Mod::Cmd, Mod::Shift]);
    assert_eq!(c.key, "T");
    let c = parse_combo("ctrl+alt+delete").unwrap();
    assert_eq!(c.mods, vec![Mod::Option, Mod::Ctrl]);
    let c = parse_combo("cmd++").unwrap();
    assert_eq!((c.mods, c.key.as_str()), (vec![Mod::Cmd], "plus"));
    let c = parse_combo("+").unwrap();
    assert_eq!((c.mods, c.key.as_str()), (vec![Mod::Shift], "="));
    let c = parse_combo("shift").unwrap();
    assert_eq!((c.mods, c.key.as_str()), (vec![Mod::Shift], ""));
    let c = parse_combo("cmd+cmd+a").unwrap();
    assert_eq!(c.mods, vec![Mod::Cmd]);
}

#[test]
fn bad_combos_say_what_is_wrong() {
    assert!(parse_combo("").unwrap_err().contains("no key"));
    assert!(parse_combo("hyper+a")
        .unwrap_err()
        .contains("not a modifier"));
    assert!(parse_combo("cmd+nokey").unwrap_err().contains("not a key"));
    assert!(parse_combo("cmd+").unwrap_err().contains("not a key"));
}

#[test]
fn flags_of_modifiers() {
    assert_eq!(flags(&[]), 0);
    assert_eq!(flags(&[Mod::Cmd, Mod::Shift]), FLAG_CMD | FLAG_SHIFT);
    assert_eq!(mod_code(Mod::Cmd), 55);
    assert_eq!(parse_mod("⌘"), Some(Mod::Cmd));
    assert_eq!(parse_mod("Alt"), Some(Mod::Option));
}

#[test]
fn a_combo_holds_its_modifiers_around_the_key() {
    let mut out = Vec::new();
    combo_plan(&parse_combo("cmd+shift+t").unwrap(), &mut out).unwrap();
    let f = FLAG_CMD | FLAG_SHIFT;
    assert_eq!(
        out,
        vec![
            Low::Modifier {
                code: 55,
                down: true,
                flags: FLAG_CMD
            },
            Low::Modifier {
                code: 56,
                down: true,
                flags: f
            },
            Low::Key {
                code: 17,
                down: true,
                flags: f,
                chars: Some('T')
            },
            Low::Wait(PRESS_MS),
            Low::Key {
                code: 17,
                down: false,
                flags: f,
                chars: Some('T')
            },
            Low::Modifier {
                code: 56,
                down: false,
                flags: FLAG_CMD
            },
            Low::Modifier {
                code: 55,
                down: false,
                flags: 0
            },
            Low::Wait(PRESS_MS),
        ]
    );
    let mut out = Vec::new();
    combo_plan(&parse_combo("shift+1").unwrap(), &mut out).unwrap();
    assert!(out.contains(&Low::Key {
        code: 18,
        down: true,
        flags: FLAG_SHIFT,
        chars: Some('!')
    }));
}

#[test]
fn text_goes_in_chunks_that_never_split_a_character() {
    let plan = type_plan("hello");
    assert_eq!(
        plan,
        vec![Low::Text("hello".into()), Low::Wait(TYPE_PAUSE_MS)]
    );
    let long = "x".repeat(45);
    let texts: Vec<String> = type_plan(&long)
        .into_iter()
        .filter_map(|l| match l {
            Low::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts.iter().map(|t| t.len()).collect::<Vec<_>>(),
        vec![20, 20, 5]
    );
    // 19 units and an emoji of 2: the emoji starts the next chunk.
    let s = format!("{}😀", "a".repeat(19));
    let texts: Vec<String> = type_plan(&s)
        .into_iter()
        .filter_map(|l| match l {
            Low::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["a".repeat(19), "😀".to_string()]);
    for t in &texts {
        assert!(t.encode_utf16().count() <= UNICODE_CHUNK);
    }
}

#[test]
fn returns_and_tabs_are_their_keys() {
    let plan = type_plan("a\r\nb\tc\n");
    let keys: Vec<u16> = plan
        .iter()
        .filter_map(|l| match l {
            Low::Key {
                code, down: true, ..
            } => Some(*code),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec![36, 48, 36]);
    let texts: Vec<&str> = plan
        .iter()
        .filter_map(|l| match l {
            Low::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["a", "b", "c"]);
}

#[test]
fn a_double_click_numbers_its_clicks() {
    let plan = plan(&Input::Click {
        at: p(10.0, 20.0),
        button: Button::Right,
        count: 2,
        mods: vec![Mod::Option],
    })
    .unwrap();
    let downs: Vec<(u32, Button, u64)> = plan
        .iter()
        .filter_map(|l| match l {
            Low::Down {
                clicks,
                button,
                flags,
                ..
            } => Some((*clicks, *button, *flags)),
            _ => None,
        })
        .collect();
    assert_eq!(
        downs,
        vec![
            (1, Button::Right, FLAG_OPTION),
            (2, Button::Right, FLAG_OPTION)
        ]
    );
    assert_eq!(plan[0], Low::Move(p(10.0, 20.0)));
    assert_eq!(event_count(&plan), 5);
    assert!(super::plan(&Input::Click {
        at: p(0.0, 0.0),
        button: Button::Left,
        count: 0,
        mods: vec![]
    })
    .is_err());
    assert!(super::plan(&Input::Click {
        at: p(0.0, 0.0),
        button: Button::Left,
        count: 4,
        mods: vec![]
    })
    .is_err());
}

#[test]
fn a_drag_moves_in_small_steps_and_ends_at_its_point() {
    let path = drag_path(p(0.0, 0.0), p(120.0, 0.0));
    assert_eq!(path.len(), 10);
    assert_eq!(*path.last().unwrap(), p(120.0, 0.0));
    assert!(path
        .windows(2)
        .all(|w| (w[1].x - w[0].x) <= DRAG_STEP + 1e-9));
    assert_eq!(drag_path(p(0.0, 0.0), p(1.0, 1.0)).len(), 4);
    let plan = plan(&Input::Drag {
        from: p(0.0, 0.0),
        to: p(120.0, 0.0),
        button: Button::Left,
        mods: vec![],
    })
    .unwrap();
    assert!(
        matches!(plan.iter().find(|l| matches!(l, Low::Down { .. })), Some(Low::Down { at, .. }) if *at == p(0.0, 0.0))
    );
    assert!(matches!(plan.last(), Some(Low::Up { at, .. }) if *at == p(120.0, 0.0)));
    assert_eq!(
        plan.iter()
            .filter(|l| matches!(l, Low::Drag { .. }))
            .count(),
        10
    );
}

#[test]
fn a_scroll_down_is_a_negative_wheel_in_steps_of_five() {
    let plan = plan(&Input::Scroll {
        at: p(5.0, 5.0),
        dx: 0,
        dy: 12,
    })
    .unwrap();
    let wheels: Vec<(i32, i32)> = plan
        .iter()
        .filter_map(|l| match l {
            Low::Scroll { x, y } => Some((*x, *y)),
            _ => None,
        })
        .collect();
    assert_eq!(wheels, vec![(0, -5), (0, -5), (0, -2)]);
    let plan = super::plan(&Input::Scroll {
        at: p(5.0, 5.0),
        dx: -3,
        dy: 0,
    })
    .unwrap();
    assert!(plan.contains(&Low::Scroll { x: 3, y: 0 }));
    assert!(super::plan(&Input::Scroll {
        at: p(5.0, 5.0),
        dx: 0,
        dy: 0
    })
    .is_err());
    assert!(super::plan(&Input::Scroll {
        at: p(5.0, 5.0),
        dx: 0,
        dy: SCROLL_MAX + 1
    })
    .is_err());
}

#[test]
fn bad_values_are_refused_before_any_event() {
    assert!(plan(&Input::Type {
        text: String::new()
    })
    .is_err());
    assert!(plan(&Input::Type {
        text: "x".repeat(super::super::computer::TYPE_MAX + 1)
    })
    .is_err());
    assert!(plan(&Input::Key { combos: vec![] }).is_err());
    let many = vec![parse_combo("a").unwrap(); KEYS_MAX + 1];
    assert!(plan(&Input::Key { combos: many }).is_err());
    assert!(plan(&Input::Move {
        at: p(f64::NAN, 1.0)
    })
    .is_err());
    let bad = Combo {
        mods: vec![],
        key: "nokey".into(),
    };
    assert!(plan(&Input::Key { combos: vec![bad] }).is_err());
}

/// Every name of a key that `parse_combo` takes: the name, in upper
/// case, and with `_` or a space at each place inside it.
fn aliases(key: &str) -> Vec<String> {
    let mut out = vec![key.to_string(), key.to_ascii_uppercase()];
    if key.chars().count() > 1 {
        for i in 0..=key.len() {
            for sep in ['_', ' '] {
                let (a, b) = key.split_at(i);
                out.push(format!("{a}{sep}{b}"));
                out.push(format!("{a}{sep}{sep}{b}").to_ascii_uppercase());
            }
        }
    }
    out
}

#[test]
fn system_shortcuts_are_refused_by_any_name_of_their_key() {
    let refused = [
        ("cmd", "space"),
        ("ctrl", "space"),
        ("opt", "space"),
        ("fn", "space"),
        ("cmd", "tab"),
        ("cmd+opt", "escape"),
        ("cmd+opt", "esc"),
        ("cmd+ctrl", "q"),
        ("cmd+shift", "q"),
        ("cmd+shift", "3"),
        ("cmd+shift", "4"),
        ("cmd+shift", "5"),
        ("cmd+shift", "6"),
        ("ctrl", "left"),
        ("ctrl", "right"),
        ("ctrl", "up"),
        ("ctrl", "down"),
        ("ctrl", "1"),
        ("ctrl", "0"),
        ("cmd+opt", "8"),
        ("cmd+opt", "="),
        ("cmd+opt", "equal"),
        ("cmd+opt", "-"),
        ("cmd+opt", "minus"),
        ("cmd+opt", "plus"),
        ("cmd+opt", "d"),
        ("fn", "a"),
        ("fn", "7"),
        ("cmd", "f3"),
        ("ctrl", "f3"),
        ("opt", "f5"),
        ("fn", "f1"),
        ("", "f11"),
        ("", "f12"),
    ];
    let mut tried = 0;
    for (mods, key) in refused {
        for alias in aliases(key) {
            let text = if mods.is_empty() {
                alias.clone()
            } else {
                format!("{mods}+{alias}")
            };
            // Only the aliases that the parse takes: a space at an end
            // is trimmed, and a digit has no other name.
            let Ok(c) = parse_combo(&text) else {
                continue;
            };
            tried += 1;
            assert!(system_shortcut(&c), "{text:?} gives {c:?}");
        }
    }
    assert!(tried > 300, "{tried}");
    // The examples of the review, each one by name.
    for text in [
        "cmd+s_pace",
        "cmd+t_ab",
        "ctrl+le_ft",
        "ctrl+f_3",
        "cmd+opt+es_c",
        "CMD+SPACE",
        "cmd+opt++",
        "cmd+opt+e_q_u_a_l",
    ] {
        let c = parse_combo(text).unwrap();
        assert!(system_shortcut(&c), "{text:?}");
    }
    // A combo of the wire comes without the parse: the same holds.
    let c = Combo {
        mods: vec![Mod::Cmd],
        key: "S pace".into(),
    };
    assert!(system_shortcut(&c));
    // The shortcuts of an app still go, by any name too.
    for (mods, key) in [
        ("cmd", "s"),
        ("cmd+shift", "t"),
        ("cmd", "grave"),
        ("ctrl", "tab"),
        ("", "space"),
        ("shift", "tab"),
        ("cmd", "q"),
        ("", "f5"),
        ("cmd", "left"),
        ("cmd+shift", "7"),
    ] {
        for alias in aliases(key) {
            let text = if mods.is_empty() {
                alias.clone()
            } else {
                format!("{mods}+{alias}")
            };
            if let Ok(c) = parse_combo(&text) {
                assert!(!system_shortcut(&c), "{text:?} gives {c:?}");
            }
        }
    }
}

#[test]
fn a_drag_is_two_steps_the_press_and_the_rest() {
    let d = Input::Drag {
        from: p(0.0, 0.0),
        to: p(120.0, 0.0),
        button: Button::Left,
        mods: vec![],
    };
    let s = steps(&d).unwrap();
    assert_eq!(s.len(), 2);
    assert!(matches!(
        s[0].iter().rev().find(|l| !matches!(l, Low::Wait(_))),
        Some(Low::Down { .. })
    ));
    assert!(!s[0]
        .iter()
        .any(|l| matches!(l, Low::Drag { .. } | Low::Up { .. })));
    assert!(matches!(s[1].last(), Some(Low::Up { .. })));
    assert_eq!(s.concat(), plan(&d).unwrap());
}

#[test]
fn a_right_or_ctrl_multi_click_is_a_menu_click() {
    let click = |button, count, mods: Vec<Mod>| Input::Click {
        at: p(10.0, 10.0),
        button,
        count,
        mods,
    };
    assert!(menu_clicks(&click(Button::Right, 2, vec![])));
    assert!(menu_clicks(&click(Button::Right, 3, vec![])));
    assert!(menu_clicks(&click(Button::Left, 2, vec![Mod::Ctrl])));
    // One click opens the menu and picks nothing; a left double click
    // opens no menu.
    assert!(!menu_clicks(&click(Button::Right, 1, vec![])));
    assert!(!menu_clicks(&click(Button::Left, 1, vec![Mod::Ctrl])));
    assert!(!menu_clicks(&click(Button::Left, 3, vec![Mod::Shift])));
}
