use super::diff::{self, Gate, Verdict};
use super::encode::{self, Cells};
use super::fit::{self, Term};
use super::fixtures;
use super::keys::{Button, Event, Mods, Mouse, MouseKind, Parser};
use super::map::{Action, GuestButton, Mapper, Out};
use super::png::{self, Rgba};
use super::screen::{clean, Screen};
use super::sixel;
use super::source::{FakeSource, Frame, FrameSource, InputSink, Item, Max, Recorder, Target};
use super::stream::{Stream, SETTLE_MS};
use super::term::{self, Env, Protocol, Replies, Tmux};
use crate::macvm::text;
use std::time::Duration;

fn cells(col: u16, row: u16, cols: u16, rows: u16) -> Cells {
    Cells {
        col,
        row,
        cols,
        rows,
    }
}

// ---- the protocol encoders, golden bytes ----

#[test]
fn kitty_golden_one_chunk() {
    let out = encode::kitty(b"hello", 7, &cells(0, 0, 10, 5));
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0],
        b"\x1b_Ga=T,f=100,t=d,i=7,p=1,q=2,C=1,c=10,r=5,m=0;aGVsbG8=\x1b\\".to_vec()
    );
}

#[test]
fn kitty_chunks_are_4096_and_only_the_last_has_m0() {
    // 3073 bytes are 4100 base64 characters: a full chunk and 4 more.
    let out = encode::kitty(&[0u8; 3073], 1, &cells(0, 0, 2, 1));
    assert_eq!(out.len(), 2);
    let first = String::from_utf8(out[0].clone()).unwrap();
    assert!(first.starts_with("\x1b_Ga=T,f=100,t=d,i=1,p=1,q=2,C=1,c=2,r=1,m=1;"));
    let payload = first.split(';').nth(1).unwrap().trim_end_matches("\x1b\\");
    assert_eq!(payload.len(), encode::KITTY_CHUNK);
    assert_eq!(out[1], b"\x1b_Gm=0,q=2;AA==\x1b\\".to_vec());
}

#[test]
fn kitty_delete_and_query_golden() {
    assert_eq!(
        encode::kitty_delete(9),
        b"\x1b_Ga=d,d=I,i=9,q=2\x1b\\".to_vec()
    );
    assert_eq!(
        encode::KITTY_QUERY,
        b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\"
    );
}

#[test]
fn iterm_golden() {
    assert_eq!(
        encode::iterm(b"hello", &cells(3, 4, 10, 5)),
        b"\x1b]1337;File=inline=1;size=5;width=10;height=5;preserveAspectRatio=0;doNotMoveCursor=1:aGVsbG8=\x07".to_vec()
    );
}

#[test]
fn tmux_wrap_doubles_every_escape() {
    assert_eq!(
        encode::tmux_wrap(b"\x1b_Gx\x1b\\"),
        b"\x1bPtmux;\x1b\x1b_Gx\x1b\x1b\\\x1b\\".to_vec()
    );
    assert_eq!(encode::cursor_to(&cells(3, 4, 1, 1)), b"\x1b[5;4H".to_vec());
}

#[test]
fn sixel_golden_two_by_two() {
    let img = Rgba {
        width: 2,
        height: 2,
        data: vec![
            255, 0, 0, 255, 0, 255, 0, 255, // red, green
            0, 0, 255, 255, 255, 255, 255, 255, // blue, white
        ],
    };
    let out = String::from_utf8(sixel::encode(&img)).unwrap();
    assert_eq!(
        out,
        "\x1bP0;1;0q\"1;1;2;2#5;2;0;0;100#30;2;0;100;0#180;2;100;0;0#215;2;100;100;100#5A$#30?@$#180@$#215?A-\x1b\\"
    );
}

#[test]
fn sixel_run_length_and_bands() {
    // 5 x 7: one color, two bands, the second band one row high.
    let img = Rgba {
        width: 5,
        height: 7,
        data: [0u8, 0, 0, 255].repeat(35),
    };
    let out = String::from_utf8(sixel::encode(&img)).unwrap();
    assert_eq!(out, "\x1bP0;1;0q\"1;1;5;7#0;2;0;0;0#0!5~-#0!5@-\x1b\\");
}

#[test]
fn sixel_alpha_is_over_black_and_scale_averages() {
    let img = Rgba {
        width: 2,
        height: 1,
        data: vec![255, 255, 255, 0, 255, 255, 255, 255],
    };
    let s = sixel::scale(&img, 1, 1);
    assert_eq!(s.data, vec![255, 255, 255, 127]);
    let out = String::from_utf8(sixel::encode(&img)).unwrap();
    assert!(
        out.contains("#0;2;0;0;0") && out.contains("#215;2;100;100;100"),
        "{out}"
    );
    let up = sixel::scale(&img, 4, 2);
    assert_eq!((up.width, up.height, up.data.len()), (4, 2, 32));
}

// ---- the PNG decoder ----

#[test]
fn png_decodes_fixed_huffman_and_all_filters() {
    let img = png::decode(fixtures::RGBA_7X5).unwrap();
    assert_eq!((img.width, img.height), (7, 5));
    assert_eq!(img.data, fixtures::RGBA_7X5_PIXELS);
}

#[test]
fn png_decodes_dynamic_huffman() {
    let img = png::decode(fixtures::DYN_64X40).unwrap();
    assert_eq!((img.width, img.height), (64, 40));
    for (y, x) in [(0usize, 0usize), (39, 63), (17, 22)] {
        let p = &img.data[(y * 64 + x) * 4..][..4];
        assert_eq!(
            p,
            &[
                ((x / 8) * 30 % 256) as u8,
                ((y / 5) * 25 % 256) as u8,
                128,
                255
            ]
        );
    }
}

#[test]
fn png_decodes_palette_with_trns_and_gray16() {
    let img = png::decode(fixtures::PAL4_6X2).unwrap();
    assert_eq!(&img.data[..8], &[0, 0, 0, 0, 255, 0, 0, 255]);
    assert_eq!(&img.data[8..12], &[0, 255, 0, 255]);
    let img = png::decode(fixtures::GRAY16_3X2).unwrap();
    // 20000 >> 8 = 78; row 1 adds 1000.
    assert_eq!(&img.data[4..8], &[78, 78, 78, 255]);
    assert_eq!(&img.data[12..16], &[3, 3, 3, 255]);
}

#[test]
fn png_round_trips_the_stored_encoder() {
    let px = FakeSource::picture(300, 230, 3);
    let enc = png::encode(300, 230, &px);
    let img = png::decode(&enc).unwrap();
    assert_eq!(img.data, px);
}

#[test]
fn png_refuses_hostile_frames_before_allocating() {
    let mut big = png::encode(1, 1, &[0, 0, 0, 255]);
    // 100000 x 100000 in the header.
    big[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    big[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    assert!(png::header(&big).unwrap_err().contains("per side"));
    let mut wide = png::encode(1, 1, &[0, 0, 0, 255]);
    wide[16..20].copy_from_slice(&16_000u32.to_be_bytes());
    wide[20..24].copy_from_slice(&16_000u32.to_be_bytes());
    assert!(png::header(&wide).unwrap_err().contains("pixels"));
    assert!(png::header(b"GIF89a").is_err());
    // A header that promises 2x2 but inflates to more: the limit stops it.
    let mut lie = png::encode(4, 4, &[9u8; 64]);
    lie[16..20].copy_from_slice(&2u32.to_be_bytes());
    lie[20..24].copy_from_slice(&2u32.to_be_bytes());
    let crc = png::crc32(&lie[12..29]);
    lie[29..33].copy_from_slice(&crc.to_be_bytes());
    assert!(png::decode(&lie).is_err());
    // A flipped byte fails the chunk checksum.
    let mut bad = png::encode(2, 2, &[1u8; 16]);
    let n = bad.len() - 20;
    bad[n] ^= 0xff;
    assert!(png::decode(&bad).is_err());
}

#[test]
fn frame_check_compares_the_stated_size() {
    let png = png::encode(3, 2, &[0u8; 24]);
    let ok = Frame {
        seq: 1,
        width: 3,
        height: 2,
        png: png.clone(),
    };
    ok.check().unwrap();
    let lie = Frame { width: 30, ..ok };
    assert!(lie.check().unwrap_err().contains("says 30x2"));
}

// ---- change detection ----

fn place(cols: u16) -> fit::Placement {
    fit::place(&Term::new(cols, 25, None, Some((10, 20))), 800, 600).unwrap()
}

#[test]
fn gate_draws_only_changed_frames() {
    let mut g = Gate::default();
    let p = place(80);
    assert_eq!(g.check(1, b"a", &p), Verdict::Draw);
    assert_eq!(g.check(2, b"a", &p), Verdict::Same);
    assert_eq!(g.check(3, b"b", &p), Verdict::Draw);
    assert_eq!(g.check(2, b"c", &p), Verdict::Old);
    assert_eq!(g.check(3, b"b", &p), Verdict::Old);
    // A new place draws the same pixels again.
    assert_eq!(g.check(4, b"b", &place(60)), Verdict::Draw);
    g.invalidate();
    assert_eq!(g.check(4, b"b", &place(60)), Verdict::Draw);
    g.cleared();
    assert_eq!(g.check(5, b"b", &place(60)), Verdict::Draw);
    assert_eq!(g.skipped, 3);
}

#[test]
fn newest_keeps_the_present() {
    let (f, dropped) = diff::newest(vec![3u64, 9, 4], |s| *s).unwrap();
    assert_eq!((f, dropped), (9, 2));
    assert!(diff::newest(Vec::<u64>::new(), |s| *s).is_none());
}

// ---- the fit ----

#[test]
fn fit_keeps_the_aspect_and_never_grows() {
    let t = Term::new(100, 31, None, Some((10, 20)));
    // Room: 1000 x 600 px. A 2880x1800 Retina frame scales to 960x600.
    let p = fit::place(&t, 2880, 1800).unwrap();
    assert_eq!(p.cells, cells(2, 0, 96, 30));
    // A small window stays 1:1, centered.
    let p = fit::place(&t, 200, 100).unwrap();
    assert_eq!(p.cells, cells(40, 12, 20, 5));
    assert!(fit::place(&Term::new(80, 1, None, None), 10, 10).is_none());
}

#[test]
fn fit_cell_size_from_pixels_or_the_guess() {
    let t = Term::new(80, 24, Some((1600, 960)), Some((7, 14)));
    assert_eq!((t.cell_w, t.cell_h, t.guessed), (20, 40, false));
    let t = Term::new(80, 24, Some((0, 0)), Some((7, 14)));
    assert_eq!((t.cell_w, t.cell_h), (7, 14));
    let t = Term::new(80, 24, Some((0, 0)), None);
    assert_eq!((t.cell_w, t.cell_h, t.guessed), (8, 16, true));
}

#[test]
fn mouse_points_map_into_the_frame() {
    let t = Term::new(100, 31, None, Some((10, 20)));
    let p = fit::place(&t, 2880, 1800).unwrap(); // at col 2, 96 x 30 cells
    assert_eq!(p.point_px(20, 0), Some((0, 0)));
    assert_eq!(p.point_px(20 + 959, 599), Some((2877, 1797)));
    assert_eq!(p.point_px(19, 10), None);
    assert_eq!(p.point_px(20 + 960, 10), None);
    // The center of cell (2, 0) is pixel (25, 10): frame (15, 30).
    assert_eq!(p.point_cell(2, 0), Some((15, 30)));
    assert_eq!(p.point_cell(1, 0), None);
}

// ---- the input ----

fn parse(bytes: &[u8]) -> Vec<Event> {
    let mut p = Parser::new(false);
    let mut v = p.feed(bytes);
    v.extend(p.idle());
    v
}

fn k(name: &str, mods: Mods) -> Event {
    Event::Key {
        key: name.into(),
        mods,
    }
}

const CTRL: Mods = Mods {
    control: true,
    ..Mods::NONE
};
const ALT: Mods = Mods {
    option: true,
    ..Mods::NONE
};
const CMD: Mods = Mods {
    cmd: true,
    ..Mods::NONE
};

#[test]
fn keys_legacy_and_text() {
    assert_eq!(parse(b"hi\xc3\xa9"), vec![Event::Text("hi\u{e9}".into())]);
    assert_eq!(
        parse(b"a\rb\x7f\x09"),
        vec![
            Event::Text("a".into()),
            k("return", Mods::NONE),
            Event::Text("b".into()),
            k("delete", Mods::NONE),
            k("tab", Mods::NONE),
        ]
    );
    assert_eq!(parse(b"\x01"), vec![k("a", CTRL)]);
    assert_eq!(parse(b"\x1d"), vec![k("]", CTRL)]);
    assert_eq!(parse(b"\x1bx"), vec![k("x", ALT)]);
    assert_eq!(
        parse(b"\x1bX"),
        vec![k(
            "x",
            Mods {
                shift: true,
                option: true,
                ..Mods::NONE
            }
        )]
    );
    assert_eq!(parse(b"\x1b"), vec![k("escape", Mods::NONE)]);
}

#[test]
fn keys_csi_ss3_and_modifiers() {
    assert_eq!(
        parse(b"\x1b[A\x1bOB"),
        vec![k("up", Mods::NONE), k("down", Mods::NONE)]
    );
    assert_eq!(parse(b"\x1b[1;5C"), vec![k("right", CTRL)]);
    assert_eq!(
        parse(b"\x1b[3~\x1b[5;3~"),
        vec![k("forward-delete", Mods::NONE), k("page-up", ALT)]
    );
    assert_eq!(
        parse(b"\x1b[24~\x1bOP"),
        vec![k("f12", Mods::NONE), k("f1", Mods::NONE)]
    );
    assert_eq!(
        parse(b"\x1b[Z"),
        vec![k(
            "tab",
            Mods {
                shift: true,
                ..Mods::NONE
            }
        )]
    );
}

#[test]
fn keys_kitty_protocol_reports_super_as_cmd() {
    // Super+c, Ctrl+], Escape, Cmd+Shift+Z, keypad 7.
    assert_eq!(
        parse(b"\x1b[99;9u\x1b[93;5u\x1b[27u\x1b[90;10u\x1b[57406u"),
        vec![
            k("c", CMD),
            k("]", CTRL),
            k("escape", Mods::NONE),
            k(
                "z",
                Mods {
                    shift: true,
                    cmd: true,
                    ..Mods::NONE
                }
            ),
            k("7", Mods::NONE),
        ]
    );
    // A modifier key alone and a reply are not keys.
    assert_eq!(parse(b"\x1b[57441;2u\x1b[?1u"), vec![]);
}

#[test]
fn keys_split_across_reads_wait_for_the_rest() {
    let mut p = Parser::new(false);
    assert_eq!(p.feed(b"\x1b["), vec![]);
    assert!(p.pending());
    assert_eq!(p.feed(b"1;2"), vec![]);
    assert_eq!(
        p.feed(b"Dz\xe2\x82"),
        vec![
            k(
                "left",
                Mods {
                    shift: true,
                    ..Mods::NONE
                }
            ),
            Event::Text("z".into())
        ]
    );
    assert_eq!(p.feed(b"\xac"), vec![Event::Text("\u{20ac}".into())]);
}

#[test]
fn keys_paste_and_focus() {
    assert_eq!(
        parse(b"\x1b[200~line1\nline\x1b[2\x1b[201~\x1b[I"),
        vec![Event::Paste("line1\nline\x1b[2".into()), Event::Focus(true)]
    );
    let mut p = Parser::new(false);
    assert_eq!(p.feed(b"\x1b[200~abc\x1b[20"), vec![]);
    assert_eq!(p.feed(b"1~"), vec![Event::Paste("abc".into())]);
}

#[test]
fn keys_sgr_mouse() {
    assert_eq!(
        parse(b"\x1b[<0;10;5M\x1b[<32;11;5M\x1b[<0;11;5m\x1b[<65;1;1M\x1b[<18;3;4M"),
        vec![
            Event::Mouse(Mouse {
                kind: MouseKind::Press,
                button: Button::Left,
                x: 9,
                y: 4,
                mods: Mods::NONE
            }),
            Event::Mouse(Mouse {
                kind: MouseKind::Motion,
                button: Button::Left,
                x: 10,
                y: 4,
                mods: Mods::NONE
            }),
            Event::Mouse(Mouse {
                kind: MouseKind::Release,
                button: Button::Left,
                x: 10,
                y: 4,
                mods: Mods::NONE
            }),
            Event::Mouse(Mouse {
                kind: MouseKind::Press,
                button: Button::WheelDown,
                x: 0,
                y: 0,
                mods: Mods::NONE
            }),
            Event::Mouse(Mouse {
                kind: MouseKind::Press,
                button: Button::Right,
                x: 2,
                y: 3,
                mods: CTRL
            }),
        ]
    );
}

fn acts(outs: Vec<Out>) -> Vec<Action> {
    outs.into_iter()
        .filter_map(|o| match o {
            Out::Act(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn mouse(kind: MouseKind, button: Button, x: u32, y: u32) -> Event {
    Event::Mouse(Mouse {
        kind,
        button,
        x,
        y,
        mods: Mods::NONE,
    })
}

#[test]
fn map_clicks_double_clicks_and_drags() {
    // One terminal pixel is one frame pixel.
    let t = Term::new(100, 31, None, Some((10, 20)));
    let p = fit::place(&t, 1000, 600).unwrap();
    let mut m = Mapper::default();
    let mut run = |ev, now| acts(m.map(ev, Some(&p), true, now));
    assert!(run(mouse(MouseKind::Press, Button::Left, 100, 50), 0).is_empty());
    let click = |count| Action::Click {
        x: 100,
        y: 50,
        button: GuestButton::Left,
        count,
        mods: Mods::NONE,
    };
    assert_eq!(
        run(mouse(MouseKind::Release, Button::Left, 101, 50), 10),
        vec![click(1)]
    );
    run(mouse(MouseKind::Press, Button::Left, 100, 50), 200);
    assert_eq!(
        run(mouse(MouseKind::Release, Button::Left, 100, 50), 210),
        vec![click(2)]
    );
    // Too late for a third.
    run(mouse(MouseKind::Press, Button::Left, 100, 50), 900);
    assert_eq!(
        run(mouse(MouseKind::Release, Button::Left, 100, 50), 905),
        vec![click(1)]
    );
    // A drag, released outside the frame: it ends at the last point inside.
    run(mouse(MouseKind::Press, Button::Left, 10, 10), 2000);
    run(mouse(MouseKind::Motion, Button::Left, 300, 200), 2010);
    assert_eq!(
        run(mouse(MouseKind::Release, Button::Left, 5000, 5000), 2020),
        vec![Action::Drag {
            from: (10, 10),
            to: (300, 200),
            button: GuestButton::Left,
            mods: Mods::NONE
        }]
    );
    // The wheel; shift turns it sideways.
    assert_eq!(
        run(mouse(MouseKind::Press, Button::WheelUp, 5, 5), 3000),
        vec![Action::Scroll {
            x: 5,
            y: 5,
            dx: 0,
            dy: -1
        }]
    );
    let shifted = Event::Mouse(Mouse {
        kind: MouseKind::Press,
        button: Button::WheelDown,
        x: 5,
        y: 5,
        mods: Mods {
            shift: true,
            ..Mods::NONE
        },
    });
    assert_eq!(
        run(shifted, 3001),
        vec![Action::Scroll {
            x: 5,
            y: 5,
            dx: 1,
            dy: 0
        }]
    );
    // Outside the frame nothing happens.
    assert!(run(mouse(MouseKind::Press, Button::WheelUp, 5000, 5), 3002).is_empty());
}

#[test]
fn map_cells_use_the_cell_center() {
    let t = Term::new(100, 31, None, Some((10, 20)));
    let p = fit::place(&t, 2000, 1200).unwrap(); // half scale
    let mut m = Mapper::default();
    m.map(
        mouse(MouseKind::Press, Button::Right, 0, 0),
        Some(&p),
        false,
        0,
    );
    assert_eq!(
        acts(m.map(
            mouse(MouseKind::Release, Button::Right, 0, 0),
            Some(&p),
            false,
            1
        )),
        vec![Action::Click {
            x: 10,
            y: 20,
            button: GuestButton::Right,
            count: 1,
            mods: Mods::NONE
        }]
    );
}

#[test]
fn map_text_keys_and_paste() {
    let mut m = Mapper::default();
    assert_eq!(
        acts(m.map(Event::Paste("a\r\nb\x07c".into()), None, false, 0)),
        vec![
            Action::Type("a".into()),
            Action::Key {
                key: "return".into(),
                mods: Mods::NONE
            },
            Action::Type("bc".into()),
        ]
    );
    let long = "é".repeat(3000);
    let parts = acts(m.map(Event::Text(long), None, false, 0));
    assert_eq!(parts.len(), 2);
    assert_eq!(
        acts(m.map(k("left", CTRL), None, false, 0)),
        vec![Action::Key {
            key: "left".into(),
            mods: CTRL
        }]
    );
}

#[path = "tests/more_tests.rs"]
mod more;

#[path = "tests/string_tests.rs"]
mod strings;
