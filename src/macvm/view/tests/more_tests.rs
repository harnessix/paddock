//! Tests of the input mapping, the menu, the reply strings and the stream.

use super::*;

#[test]
fn map_menu_adds_cmd_and_holds_the_view_keys() {
    let mut m = Mapper::default();
    let mut menu = |ev| {
        assert_eq!(m.map(k("]", CTRL), None, false, 0), vec![Out::MenuOpen]);
        m.map(ev, None, false, 0)
    };
    let cmd_key = |key: &str, mods: Mods| {
        Out::Act(Action::Key {
            key: key.into(),
            mods,
        })
    };
    assert_eq!(
        menu(Event::Text("q".into())),
        vec![cmd_key("q", CMD), Out::MenuClose]
    );
    assert_eq!(
        menu(Event::Text("Z".into())),
        vec![
            cmd_key(
                "z",
                Mods {
                    shift: true,
                    cmd: true,
                    ..Mods::NONE
                }
            ),
            Out::MenuClose
        ]
    );
    assert_eq!(
        menu(k("tab", Mods::NONE)),
        vec![cmd_key("tab", CMD), Out::MenuClose]
    );
    assert_eq!(
        menu(Event::Text(" ".into())),
        vec![cmd_key("space", CMD), Out::MenuClose]
    );
    assert_eq!(
        menu(k("h", ALT)),
        vec![
            cmd_key(
                "h",
                Mods {
                    option: true,
                    cmd: true,
                    ..Mods::NONE
                }
            ),
            Out::MenuClose
        ]
    );
    assert_eq!(
        menu(k("e", CTRL)),
        vec![
            cmd_key(
                "escape",
                Mods {
                    option: true,
                    cmd: true,
                    ..Mods::NONE
                }
            ),
            Out::MenuClose
        ]
    );
    assert_eq!(menu(k("]", CTRL)), vec![cmd_key("]", CTRL), Out::MenuClose]);
    assert_eq!(menu(k("escape", Mods::NONE)), vec![Out::MenuClose]);
    assert_eq!(menu(k("l", CTRL)), vec![Out::Redraw, Out::MenuClose]);
    assert_eq!(menu(k("c", CTRL)), vec![Out::Leave]);
    assert!(!m.menu_open());
    // Outside the menu, Ctrl-C goes to the Mac.
    assert_eq!(
        acts(m.map(k("c", CTRL), None, false, 0)),
        vec![Action::Key {
            key: "c".into(),
            mods: CTRL
        }]
    );
}

// ---- the terminal detection ----

fn env(term: &str, program: &str) -> Env {
    Env {
        term: term.into(),
        term_program: program.into(),
        ..Env::default()
    }
}

fn da1(p: &[u32]) -> Replies {
    Replies {
        da1: Some(p.to_vec()),
        ..Replies::default()
    }
}

#[test]
fn parse_probe_replies() {
    let (r, done) = term::parse_replies(
        b"\x1b_Gi=31;OK\x1b\\\x1b[?1u\x1b[6;20;10t\x1b[?1016;2$y\x1b[?62;4;22cx",
    );
    assert!(done);
    assert!(r.kitty && r.kitty_keys && r.mouse_pixels);
    assert_eq!(r.cell, Some((10, 20)));
    assert_eq!(r.da1, Some(vec![62, 4, 22]));
    assert_eq!(r.rest, b"x".to_vec());
    let (r, done) = term::parse_replies(b"\x1b_Gi=31;ENOTSUPPORTED\x1b\\\x1b[?1016;0$y");
    assert!(!done && !r.kitty && !r.mouse_pixels);
    // A partial reply waits.
    let (_, done) = term::parse_replies(b"\x1b[?62;");
    assert!(!done);
}

#[test]
fn probe_wraps_only_the_kitty_query_in_tmux() {
    let p = term::probe(true);
    assert!(p.starts_with(b"\x1bPtmux;\x1b\x1b_Gi=31"));
    assert!(p.ends_with(b"\x1b[?u\x1b[16t\x1b[?1016$p\x1b[c"));
    assert!(term::probe(false).starts_with(encode::KITTY_QUERY));
}

#[test]
fn choose_by_replies_and_environment() {
    let kitty = Replies {
        kitty: true,
        ..da1(&[62])
    };
    let c = term::choose(&env("xterm-kitty", ""), None, &kitty, None);
    assert_eq!((c.protocol, c.wrap), (Some(Protocol::Kitty), false));
    let c = term::choose(
        &env("xterm-256color", "iTerm.app"),
        None,
        &da1(&[62, 4]),
        None,
    );
    assert_eq!(c.protocol, Some(Protocol::Iterm2));
    let c = term::choose(&env("xterm", ""), None, &da1(&[62, 4, 22]), None);
    assert_eq!(c.protocol, Some(Protocol::Sixel));
    let c = term::choose(
        &env("xterm-256color", "Apple_Terminal"),
        None,
        &da1(&[1, 2]),
        None,
    );
    assert_eq!(c.protocol, None);
    assert!(c.notes[0].contains("Apple_Terminal"), "{:?}", c.notes);
    let c = term::choose(&env("xterm", ""), None, &da1(&[1]), Some(Protocol::Sixel));
    assert_eq!(c.protocol, Some(Protocol::Sixel));
}

fn tmux(passthrough: &str, client: &str) -> Tmux {
    Tmux {
        version: "3.4".into(),
        passthrough: passthrough.into(),
        client: client.into(),
    }
}

#[test]
fn choose_in_tmux_uses_passthrough_or_says_why_not() {
    let mut e = env("tmux-256color", "tmux");
    e.tmux = true;
    let kitty = Replies {
        kitty: true,
        ..da1(&[62])
    };
    let c = term::choose(&e, Some(&tmux("on", "xterm-kitty")), &kitty, None);
    assert_eq!((c.protocol, c.wrap), (Some(Protocol::Kitty), true));
    let c = term::choose(&e, Some(&tmux("off", "xterm-kitty")), &da1(&[62]), None);
    assert_eq!(c.protocol, None);
    assert!(c.notes[0].contains("tmux set -g allow-passthrough on"));
    assert_eq!(c.notes.len(), 1);
    // tmux built with sixel answers 4 itself: no passthrough needed.
    let c = term::choose(&e, Some(&tmux("off", "xterm")), &da1(&[62, 4]), None);
    assert_eq!((c.protocol, c.wrap), (Some(Protocol::Sixel), false));
    assert!(!c.notes.is_empty());
    let c = term::choose(
        &e,
        Some(&tmux("all", "xterm-256color iTerm2 3.5.0")),
        &da1(&[62]),
        None,
    );
    assert_eq!((c.protocol, c.wrap), (Some(Protocol::Iterm2), true));
    // A forced protocol still needs passthrough.
    let c = term::choose(
        &e,
        Some(&tmux("off", "")),
        &da1(&[62]),
        Some(Protocol::Kitty),
    );
    assert_eq!(c.protocol, None);
    // tmux older than 3.3 has no such option and passes everything.
    let old = Tmux {
        version: "3.2a".into(),
        ..tmux("", "")
    };
    assert!(old.passes());
    assert!(!tmux("", "").passes());
}

#[test]
fn choose_in_screen_says_it_blocks_images() {
    let mut e = env("screen", "");
    e.screen = true;
    let c = term::choose(
        &e,
        None,
        &Replies {
            kitty: true,
            ..da1(&[62])
        },
        None,
    );
    assert_eq!(c.protocol, None);
    assert!(c.notes[0].contains("GNU screen"));
}

#[test]
fn env_from_vars() {
    let e = Env::from_vars(|k| match k {
        "TERM" => Some("xterm-kitty".into()),
        "TMUX" => Some("/tmp/tmux-1/default,1,0".into()),
        "STY" => Some(String::new()),
        _ => None,
    });
    assert!(e.tmux && !e.screen);
    assert_eq!(e.term, "xterm-kitty");
    assert_eq!(Protocol::parse("auto"), Ok(None));
    assert!(Protocol::parse("png").is_err());
}

// ---- the screen ----

fn frame(seq: u64, n: u64) -> Frame {
    Frame {
        seq,
        width: 40,
        height: 30,
        png: png::encode(40, 30, &FakeSource::picture(40, 30, n)),
    }
}

fn screen(p: Protocol) -> Screen {
    Screen::new(p, false, Term::new(20, 11, None, Some((4, 4))), "Notes")
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn screen_kitty_alternates_ids_and_skips_same_frames() {
    let mut s = screen(Protocol::Kitty);
    let a = s.frame(frame(1, 0));
    assert!(has(&a, b"\x1b_Ga=T,f=100,t=d,i=1885601793,p=1"));
    assert!(has(&a, b"\x1b[11;1H")); // the status line
    let b = s.frame(frame(2, 1));
    assert!(has(&b, b"i=1885601794,"));
    assert!(has(&b, &encode::kitty_delete(encode::KITTY_IDS[0])));
    assert!(s.frame(frame(3, 1)).is_empty());
    assert!(s.frame(frame(2, 5)).is_empty());
    assert_eq!((s.drawn, s.skipped()), (2, 2));
    let bye = s.leave();
    assert!(has(&bye, &encode::kitty_delete(encode::KITTY_IDS[1])));
    assert!(bye.ends_with(b"\x1b[?25h\x1b[?1049l"));
}

#[test]
fn screen_sixel_and_iterm_draw_and_resize() {
    let mut s = screen(Protocol::Sixel);
    let out = s.frame(frame(1, 0));
    // 40x30 in 20x10 cells of 4 px: 80x40 room, so 40x30 at 1:1 is 10x8 cells.
    assert!(
        has(&out, b"\x1bP0;1;0q\"1;1;40;32"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    let mut s = screen(Protocol::Iterm2);
    s.frame(frame(1, 0));
    let out = s.resize(Term::new(10, 6, None, Some((4, 4))));
    assert!(has(&out, b"\x1b[2J") && has(&out, b"\x1b]1337;File=inline=1;"));
    assert!(s.resize(Term::new(10, 6, None, Some((4, 4)))).is_empty());
}

#[test]
fn screen_wraps_images_in_tmux_but_not_the_cursor() {
    let mut s = Screen::new(
        Protocol::Kitty,
        true,
        Term::new(20, 11, None, Some((4, 4))),
        "x",
    );
    let out = s.frame(frame(1, 0));
    assert!(has(&out, b"\x1bPtmux;\x1b\x1b_Ga=T"));
    assert!(!has(&out, b"\x1bPtmux;\x1b\x1b["));
}

#[test]
fn screen_menu_hides_the_picture_and_brings_it_back() {
    let mut s = Screen::new(
        Protocol::Kitty,
        false,
        Term::new(80, 24, None, Some((4, 4))),
        "Notes",
    );
    s.frame(frame(1, 0));
    let open = s.input(b"\x1d", 0);
    assert!(has(&open.bytes, b"Ctrl-C  leave the view"));
    assert!(has(
        &open.bytes,
        &encode::kitty_delete(encode::KITTY_IDS[0])
    ));
    // Frames wait while the menu is open.
    assert!(s.frame(frame(2, 1)).is_empty());
    let q = s.input(b"w", 1);
    assert_eq!(
        q.actions,
        vec![Action::Key {
            key: "w".into(),
            mods: CMD
        }]
    );
    assert!(has(&q.bytes, b"\x1b_Ga=T"));
    let leave = {
        s.input(b"\x1d", 2);
        s.input(b"\x03", 3)
    };
    assert!(leave.leave && leave.actions.is_empty());
}

#[test]
fn screen_cleans_guest_text_and_refuses_bad_frames() {
    let mut s = Screen::new(
        Protocol::Kitty,
        false,
        Term::new(200, 11, None, Some((4, 4))),
        "evil\x1b]52;c;AAAA\x07app",
    );
    let out = s.frame(Frame {
        seq: 1,
        width: 1,
        height: 1,
        png: b"not a png".to_vec(),
    });
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("a frame was refused"), "{text}");
    assert!(!text.contains("\x1b]52"), "{text}");
    s.note = "bad\x1b[2Jnote".into();
    assert!(!has(&s.status(), b"\x1b[2Jnote"));
    assert_eq!(clean("a\u{7}b\nc", 10), "a b c");
}

#[test]
fn screen_escape_waits_for_idle() {
    let mut s = screen(Protocol::Kitty);
    assert!(s.input(b"\x1b", 0).actions.is_empty());
    assert!(s.pending());
    assert_eq!(
        s.idle(50).actions,
        vec![Action::Key {
            key: "escape".into(),
            mods: Mods::NONE
        }]
    );
}

#[test]
fn screen_enter_turns_on_the_modes_it_knows() {
    let mut s = screen(Protocol::Kitty);
    s.kitty_keys = true;
    s.mouse_pixels = true;
    let e = s.enter();
    assert!(has(&e, b"\x1b[?1049h") && has(&e, b"\x1b[?1006h") && has(&e, b"\x1b[?1016h"));
    assert!(has(&e, b"\x1b[>1u"));
    let l = s.leave();
    assert!(has(&l, b"\x1b[<u") && has(&l, b"\x1b[?1016l") && has(&l, b"\x1b[?1000l"));
    let mut plain = screen(Protocol::Sixel);
    assert!(!has(&plain.enter(), b"\x1b[>1u"));
}

// ---- the fakes ----

#[test]
fn fake_source_streams_and_stops() {
    let mut src = FakeSource::new(8, 6, Duration::from_millis(1), Some(5));
    let t = Target::App {
        name: "notes".into(),
        instance: Some(2),
    };
    assert_eq!(t.label(), "notes (instance 2)");
    let rx = src
        .subscribe(
            &t,
            120,
            Max {
                width: 100,
                height: 100,
            },
        )
        .unwrap();
    let mut frames = Vec::new();
    while let Item::Frame(f) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("a frame or the end")
    {
        frames.push(f);
    }
    assert_eq!(frames.len(), 5);
    assert_eq!(frames[3].png, frames[4].png);
    frames.iter().for_each(|f| f.check().unwrap());
    // The screen draws four of the five.
    let mut s = screen(Protocol::Kitty);
    let seqs: Vec<u64> = frames.iter().map(|f| f.seq).collect();
    for f in frames {
        s.frame(f);
    }
    assert_eq!((s.drawn, s.skipped()), (4, 1), "{seqs:?}");
    src.stop();
    let mut r = Recorder::default();
    r.send(&Action::Type("x".into())).unwrap();
    assert_eq!(r.actions.lock().unwrap().len(), 1);
}

// ---- the review fixes ----

/// The replies of a terminal in tmux, in the order that tmux gives them:
/// tmux answers the keyboard, cell, mouse and device queries at once, and
/// the kitty reply of the outer terminal comes after a round trip.
const TMUX_OWN: &[u8] = b"\x1b[?1u\x1b[6;20;10t\x1b[?1016;2$y\x1b[?62;22c";

fn reader(chunks: Vec<&'static [u8]>) -> impl FnMut(Duration) -> std::io::Result<Vec<u8>> {
    let mut it = chunks.into_iter();
    move |_| Ok(it.next().map(<[u8]>::to_vec).unwrap_or_default())
}

#[test]
fn probe_in_tmux_waits_for_the_wrapped_kitty_reply() {
    // tmux's own DA1 does not end the probe in tmux.
    let (own, da1_done) = term::parse_replies(TMUX_OWN);
    assert!(da1_done);
    assert!(!term::probe_complete(&own, true));
    assert!(term::probe_complete(&own, false));
    // The fake stream: tmux's answers, two empty reads, then the kitty reply.
    let r = term::collect_replies(
        true,
        Duration::from_secs(10),
        reader(vec![TMUX_OWN, b"", b"", b"\x1b_Gi=31;OK\x1b\\"]),
    )
    .unwrap();
    assert!(r.kitty && r.kitty_answered, "{r:?}");
    assert_eq!(r.da1, Some(vec![62, 22]));
    let mut e = env("tmux-256color", "tmux");
    e.tmux = true;
    let c = term::choose(&e, Some(&tmux("on", "xterm-kitty")), &r, None);
    assert_eq!((c.protocol, c.wrap), (Some(Protocol::Kitty), true));
    // An error reply also ends the wait, without kitty.
    let r = term::collect_replies(
        true,
        Duration::from_secs(10),
        reader(vec![TMUX_OWN, b"\x1b_Gi=31;ENOTSUPPORTED:x\x1b\\"]),
    )
    .unwrap();
    assert!(r.kitty_answered && !r.kitty);
    // An outer terminal that never answers: the probe ends at its deadline
    // and keeps what tmux said.
    let start = std::time::Instant::now();
    let r = term::collect_replies(true, Duration::from_millis(60), reader(vec![TMUX_OWN])).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(60));
    assert!(!r.kitty && r.da1.is_some() && r.kitty_keys);
    // Outside tmux the DA1 still ends the probe at once: no read after it.
    let mut reads = 0;
    let r = term::collect_replies(false, Duration::from_secs(10), |_| {
        reads += 1;
        Ok(if reads == 1 {
            TMUX_OWN.to_vec()
        } else {
            panic!("read after DA1")
        })
    })
    .unwrap();
    assert_eq!((reads, r.kitty), (1, false));
}

#[test]
fn late_terminal_replies_send_no_key() {
    let mut s = screen(Protocol::Kitty);
    // A kitty reply after the probe, whole and split across reads.
    assert_eq!(s.input(b"\x1b_Gi=31;OK\x1b\\", 0), Default::default());
    assert!(s.input(b"\x1b_Gi=31;", 1).actions.is_empty());
    assert!(s.pending());
    assert!(s.input(b"OK\x1b", 2).actions.is_empty());
    assert!(s.input(b"\\", 3).actions.is_empty());
    assert!(!s.pending());
    // OSC up to BEL, DCS and SOS/PM up to ST.
    assert!(s
        .input(b"\x1b]11;rgb:0000/0000/0000\x07", 4)
        .actions
        .is_empty());
    assert!(s.input(b"\x1bP>|kitty(0.36)\x1b\\", 5).actions.is_empty());
    assert!(s
        .input(b"\x1bXsos\x1b\\\x1b^pm\x1b\\", 6)
        .actions
        .is_empty());
    // A broken reply cut off by a quiet input is dropped, not typed.
    assert!(s.input(b"\x1b_Gi=31;O", 7).actions.is_empty());
    assert!(s.idle(60).actions.is_empty());
    assert!(!s.pending());
    // Keys after the replies still go to the guest.
    assert_eq!(s.input(b"a", 70).actions, vec![Action::Type("a".into())]);
    // Alt-] by hand: ESC ] alone, then quiet.
    assert!(s.input(b"\x1b]", 80).actions.is_empty());
    assert_eq!(
        s.idle(130).actions,
        vec![Action::Key {
            key: "]".into(),
            mods: ALT
        }]
    );
}

#[test]
fn a_new_subscription_restarts_the_frame_numbers() {
    let mut g = Gate::default();
    let p = place(10);
    assert_eq!(g.check(5, b"a", &p), Verdict::Draw);
    assert_eq!(g.check(1, b"b", &p), Verdict::Old);
    g.new_stream();
    assert_eq!(g.check(1, b"b", &p), Verdict::Draw);
    assert_eq!(g.check(1, b"c", &p), Verdict::Old);
    // The screen: frames of the same size, the second stream from seq 1.
    let mut s = screen(Protocol::Kitty);
    assert!(!s.frame(frame(5, 0)).is_empty());
    s.subscribed();
    assert!(!s.frame(frame(1, 1)).is_empty());
    assert!(!s.frame(frame(2, 2)).is_empty());
    assert_eq!(s.drawn, 3);
}

fn next_frame(rx: &std::sync::mpsc::Receiver<Item>) -> Frame {
    match rx.recv_timeout(Duration::from_secs(10)).expect("a frame") {
        Item::Frame(f) => f,
        Item::End(why) => panic!("the stream ended: {why}"),
    }
}

#[test]
fn the_view_subscribes_with_its_room_and_again_after_a_resize() {
    // A Retina-sized desktop and a small terminal: 20x11 cells of 4 px.
    let mut src = FakeSource::new(2000, 1000, Duration::from_millis(1), None);
    let subs = src.subscriptions.clone();
    let t = Target::Desktop {
        app: None,
        instance: None,
    };
    let mut s = screen(Protocol::Kitty);
    assert_eq!(
        s.max(),
        Max {
            width: 80,
            height: 40
        }
    );
    let mut stream = Stream::start(&mut src, &t, 30, s.max()).unwrap();
    assert_eq!(*subs.lock().unwrap(), vec![s.max()]);
    let f = next_frame(stream.frames());
    assert_eq!((f.seq, f.width, f.height), (1, 80, 40));
    assert!(!s.frame(f).is_empty());
    // The same room: no new subscription.
    assert!(!stream.room(&mut src, s.max(), 10).unwrap());
    // The terminal grows to 40x21 cells: 160x80 px of room.
    s.resize(Term::new(40, 21, None, Some((4, 4))));
    let big = s.max();
    assert_eq!((big.width, big.height), (160, 80));
    assert!(!stream.room(&mut src, big, 1000).unwrap());
    assert!(!stream.room(&mut src, big, 1000 + SETTLE_MS - 1).unwrap());
    assert_eq!(
        subs.lock().unwrap().len(),
        1,
        "a drag does not subscribe at each size"
    );
    assert!(stream.room(&mut src, big, 1000 + SETTLE_MS).unwrap());
    s.subscribed();
    assert_eq!(
        *subs.lock().unwrap(),
        vec![
            Max {
                width: 80,
                height: 40
            },
            big
        ]
    );
    assert_eq!(stream.max(), big);
    // The new stream starts at seq 1 with frames of the new room, and the
    // view draws them.
    let f = next_frame(stream.frames());
    assert_eq!((f.seq, f.width, f.height), (1, 160, 80));
    assert!(!s.frame(f).is_empty());
    let f = next_frame(stream.frames());
    assert_eq!(f.seq, 2);
    assert!(!s.frame(f).is_empty());
    // A size that changes back before it settles is no new subscription.
    assert!(!stream
        .room(
            &mut src,
            Max {
                width: 9,
                height: 9
            },
            5000
        )
        .unwrap());
    assert!(!stream.room(&mut src, big, 5000 + SETTLE_MS).unwrap());
    assert!(!stream
        .room(
            &mut src,
            Max {
                width: 9,
                height: 9
            },
            5000 + 2 * SETTLE_MS
        )
        .unwrap());
    assert_eq!(subs.lock().unwrap().len(), 2);
    src.stop();
}

#[test]
fn max_fits_down_and_never_up() {
    let m = Max {
        width: 160,
        height: 80,
    };
    assert_eq!(m.fit(2000, 1000), (160, 80));
    assert_eq!(m.fit(3024, 1964), (123, 80));
    assert_eq!(m.fit(100, 50), (100, 50));
    assert_eq!(m.fit(10_000, 1), (160, 1));
}

#[test]
fn the_menu_lists_every_key_that_leaves() {
    let lines = text::view_menu("x").join("\n");
    let mut leaving = Vec::new();
    for c in b'a'..=b'z' {
        let mut m = Mapper::default();
        assert_eq!(m.map(k("]", CTRL), None, false, 0), vec![Out::MenuOpen]);
        let key = (c as char).to_string();
        if m.map(k(&key, CTRL), None, false, 0) == vec![Out::Leave] {
            leaving.push(key.to_ascii_uppercase());
        }
    }
    assert!(leaving.contains(&"C".to_string()), "{leaving:?}");
    for key in &leaving {
        let line = format!("Ctrl-{key}  leave the view");
        assert!(
            lines.contains(&line),
            "the menu does not say {line:?}:\n{lines}"
        );
    }
}
