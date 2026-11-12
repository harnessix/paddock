//! The board drops or escapes bidi, zero-width and other invisible
//! characters, C1 controls and DEL in every field it shows.

use super::*;

/// Bidi controls, zero-width characters and other invisible format
/// characters that `broker::clean_text` removes.
const INVISIBLE: [char; 12] = [
    '\u{061C}', '\u{180E}', '\u{200B}', '\u{200D}', '\u{200F}', '\u{202A}', '\u{202E}', '\u{2066}',
    '\u{2069}', '\u{FEFF}', '\u{FFF9}', '\u{FFFB}',
];

fn has_invisible(s: &str) -> bool {
    s.contains(INVISIBLE)
}

#[test]
fn one_line_drops_bidi_and_zero_width_characters() {
    for c in INVISIBLE {
        assert_eq!(
            rules::one_line(&format!("ab{c}cd")),
            "abcd",
            "U+{:04X}",
            c as u32
        );
    }
    // Escapes go first: the rest of a sequence does not stay as text.
    assert_eq!(
        rules::one_line("a\u{202E}\x1b[31mb\u{9b}2Jc\u{200B}"),
        "abc"
    );
    // Invisible characters do not count against the length.
    let long = format!("{}x", "\u{200B}".repeat(rules::MAX_LAST * 4));
    assert_eq!(rules::one_line(&long), "x");
}

#[test]
fn a_ticket_id_title_and_status_drop_bidi_and_zero_width_characters() {
    let t = tickets::parse(
        "K\u{202E}V2\u{200B}-worker.md",
        "# K\u{202E}V2\u{200B}: na\u{2066}med\u{2069}\nStatus: bl\u{200F}ocked\u{FEFF}: key\n"
            .as_bytes(),
    );
    assert_eq!(t.id, "KV2");
    assert_eq!(t.title, "named");
    assert_eq!(t.status.as_deref(), Some("blocked: key"));
    // The file keeps the exact name: the panel opens it.
    assert_eq!(t.file, ".paddock/tickets/K\u{202E}V2\u{200B}-worker.md");
}

#[test]
fn run_fields_slot_holders_and_project_names_drop_bidi_and_zero_width_characters() {
    let mut f = facts();
    f.id = "claude-p\u{202E}-aaaaaa".into();
    f.harness = "cla\u{200B}ude".into();
    f.model = "op\u{202E}us".into();
    f.repo = "/r/al\u{2066}pha".into();
    f.status = said(200, "wor\u{200F}king");
    let slots = vec![Slot {
        slot: 1,
        holder: "wor\u{202E}ker".into(),
    }];
    let b = build(&[f], &BTreeMap::new(), 0, slots);
    let p = &b.projects[0];
    assert_eq!(p.name, "alpha");
    // The path stays exact: a reader opens it.
    assert_eq!(p.path, "/r/al\u{2066}pha");
    let r = &p.runs_without_ticket[0];
    assert!(!has_invisible(&r.id), "{:?}", r.id);
    assert_eq!(r.harness, "claude");
    assert_eq!(r.model, "opus");
    assert_eq!(r.last, "working");
    assert_eq!(b.compile_slots[0].holder, "worker");
}

/// A board whose paths hold an 8-bit CSI, DEL, bidi and zero-width
/// characters.
fn board_with_unsafe_paths() -> Board {
    let mut f = facts();
    f.repo = "/r/al\u{9b}2J\u{202E}pha\u{7f}".into();
    f.origin = Some(Origin {
        file: Some(".paddock/tickets/X\u{9b}2J\u{200B}-a.md".into()),
        ..Origin::default()
    });
    let files = BTreeMap::from([(
        f.repo.clone(),
        vec![ticket(
            "X\u{9b}2J\u{200B}-a.md",
            None,
            "# a \u{9b}31m\u{2028}ticket".as_bytes(),
        )],
    )]);
    build(&[f], &files, 0, Vec::new())
}

#[test]
fn json_escapes_c1_controls_and_invisible_characters_and_keeps_the_values() {
    let b = board_with_unsafe_paths();
    assert_eq!(
        b.projects[0].tickets[0].file,
        ".paddock/tickets/X\u{9b}2J\u{200B}-a.md"
    );
    for pretty in [false, true] {
        let json = to_json(&b, pretty).unwrap();
        assert!(
            !json.contains(['\u{9b}', '\u{7f}', '\u{2028}']) && !has_invisible(&json),
            "{json:?}"
        );
        assert!(json.contains("X\\u009b2J\\u200b-a.md"), "{json}");
        assert!(json.contains("/r/al\\u009b2J\\u202epha\\u007f"), "{json}");
        // The same values come back, exactly.
        let back: Board = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
        // Only the strings change: the layout of serde_json stays.
        assert_eq!(
            json.lines().count(),
            if pretty {
                serde_json::to_string_pretty(&b).unwrap().lines().count()
            } else {
                1
            }
        );
    }
}

#[test]
fn the_text_view_shows_a_project_path_without_escapes_or_invisible_characters() {
    let b = board_with_unsafe_paths();
    let snap = collect::Snapshot {
        board: b,
        tickets: BTreeMap::new(),
    };
    let text = text::render(&snap, None, false);
    assert!(text.contains("/r/alpha"), "{text:?}");
    assert!(
        !text.contains(['\u{9b}', '\u{7f}', '\u{2028}', '\x1b']) && !has_invisible(&text),
        "{text:?}"
    );
}
