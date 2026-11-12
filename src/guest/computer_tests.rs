//! The guest server of computer use with a fake screen: the target
//! region, the checks of a point, the input plan, and the connection with
//! its frame stream.

use super::*;
use crate::guest::computer_fake::{self as fake, raw, PNG};
use crate::macvm::computer::{Button, Combo, Element, MaxSize, Mod};
use std::io::Read;

fn display() -> Display {
    Display { w: 1440, h: 900 }
}

#[test]
fn the_desktop_region_is_the_display() {
    let (r, w) = region(Scope::Desktop, &Scene::default(), display()).unwrap();
    assert_eq!(r, Frame::new(0.0, 0.0, 1440.0, 900.0));
    assert!(w.is_none());
}

#[test]
fn the_app_region_is_its_windows_clipped_to_the_display() {
    let mut sc = fake::scene();
    sc.windows.push(fake::window(
        12,
        500,
        Frame::new(1300.0, 800.0, 400.0, 300.0),
    ));
    let mut gone = fake::window(13, 500, Frame::new(0.0, 0.0, 50.0, 50.0));
    gone.minimized = true;
    sc.windows.push(gone);
    let (r, w) = region(Scope::App, &sc, display()).unwrap();
    assert_eq!(r, Frame::new(100.0, 100.0, 1340.0, 800.0));
    assert_eq!(w.unwrap(), vec![10, 11, 12]);
}

#[test]
fn the_app_region_needs_a_running_app_with_a_window() {
    let e = region(Scope::App, &Scene::default(), display()).unwrap_err();
    assert_eq!(e.code, Code::NoApp);
    let mut sc = fake::scene();
    sc.windows.clear();
    assert_eq!(
        region(Scope::App, &sc, display()).unwrap_err().code,
        Code::NoWindow
    );
    let mut sc = fake::scene();
    for w in &mut sc.windows {
        w.level = Level::Hidden;
    }
    assert_eq!(
        region(Scope::App, &sc, display()).unwrap_err().code,
        Code::NoWindow
    );
}

fn pt(x: f64, y: f64) -> Point {
    Point { x, y }
}

#[test]
fn a_point_outside_the_display_is_refused_in_both_scopes() {
    for scope in [Scope::Desktop, Scope::App] {
        let e = check_point(scope, pt(1440.0, 10.0), display(), &fake::scene(), &[]).unwrap_err();
        assert_eq!(e.code, Code::Outside, "{scope:?}");
        assert!(e.text.contains("1440x900"), "{}", e.text);
    }
    assert!(check_point(
        Scope::Desktop,
        pt(5.0, 5.0),
        display(),
        &Scene::default(),
        &[]
    )
    .is_ok());
}

#[test]
fn an_app_point_must_be_on_an_app_window_that_nothing_covers() {
    let sc = fake::scene();
    let on = vec![
        raw(77, 900, "Finder", Frame::new(150.0, 150.0, 100.0, 100.0)),
        raw(10, 500, "Notes", Frame::new(100.0, 100.0, 800.0, 600.0)),
        // A menu of the app that the scene does not list yet.
        raw(90, 501, "Notes", Frame::new(1000.0, 50.0, 200.0, 300.0)),
        raw(1, 0, "Dock", Frame::new(0.0, 850.0, 1440.0, 50.0)),
    ];
    assert!(check_point(Scope::App, pt(500.0, 500.0), display(), &sc, &on).is_ok());
    let e = check_point(Scope::App, pt(160.0, 160.0), display(), &sc, &on).unwrap_err();
    assert_eq!(e.code, Code::Covered);
    assert!(e.text.contains("Finder"), "{}", e.text);
    assert!(check_point(Scope::App, pt(1100.0, 100.0), display(), &sc, &on).is_ok());
    let e = check_point(Scope::App, pt(50.0, 870.0), display(), &sc, &on).unwrap_err();
    assert_eq!(e.code, Code::Outside);
    // The desktop scope takes the covered point.
    assert!(check_point(Scope::Desktop, pt(160.0, 160.0), display(), &sc, &on).is_ok());
    // An invisible window does not cover.
    let mut glass = on.clone();
    glass[0].alpha = 0.0;
    assert!(check_point(Scope::App, pt(160.0, 160.0), display(), &sc, &glass).is_ok());
}

#[test]
fn screenshot_captures_the_region_at_the_scale_or_the_max_size() {
    let (server, st, _) = fake::server(fake::scene());
    let (g, png) = server.screenshot(Target::APP, None, false).unwrap();
    assert_eq!((g.width, g.height), (1600, 1200));
    assert_eq!(g.origin, pt(100.0, 100.0));
    assert!(png.starts_with(PNG));
    let (g, _) = server
        .screenshot(Target::DESKTOP, Some(MaxSize::of(1440, 1440)), true)
        .unwrap();
    assert_eq!((g.width, g.height), (1440, 900));
    let caps = st.lock().unwrap().captures.clone();
    assert_eq!(caps[0].1, Some(vec![10, 11]));
    assert_eq!(caps[1].1, None);
    assert!(caps[1].2, "the cursor");
}

#[test]
fn a_click_posts_its_plan_after_the_checks() {
    let (server, st, _) = fake::server(fake::scene());
    let click = Input::Click {
        at: pt(500.0, 400.0),
        button: Button::Left,
        count: 2,
        mods: vec![Mod::Cmd],
    };
    let done = server.input(Target::APP, &click).unwrap();
    assert_eq!(done.events, 5);
    assert!(!done.activated);
    let posted = st.lock().unwrap().posted.clone();
    assert_eq!(posted.len(), 5);
    assert!(matches!(posted[0], Low::Move(_)));
    // Outside the app: nothing posted.
    st.lock().unwrap().posted.clear();
    let far = Input::Move { at: pt(10.0, 10.0) };
    assert_eq!(
        server.input(Target::APP, &far).unwrap_err().code,
        Code::Outside
    );
    assert!(st.lock().unwrap().posted.is_empty());
    assert!(server.input(Target::DESKTOP, &far).is_ok());
}

#[test]
fn input_is_refused_on_a_locked_screen_and_without_accessibility() {
    let (server, st, _) = fake::server(fake::scene());
    let t = Input::Type { text: "hi".into() };
    st.lock().unwrap().locked = true;
    assert_eq!(
        server.input(Target::DESKTOP, &t).unwrap_err().code,
        Code::Locked
    );
    st.lock().unwrap().locked = false;
    st.lock().unwrap().accessibility = false;
    assert_eq!(
        server.input(Target::DESKTOP, &t).unwrap_err().code,
        Code::Accessibility
    );
    assert!(st.lock().unwrap().posted.is_empty());
}

#[test]
fn keys_bring_the_app_to_the_front_first() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().front = Some(900);
    st.lock().unwrap().activate_to = Some(500);
    let k = Input::Key {
        combos: vec![Combo {
            mods: vec![Mod::Cmd],
            key: "s".into(),
        }],
    };
    let done = server.input(Target::APP, &k).unwrap();
    assert!(done.activated);
    assert_eq!(st.lock().unwrap().activated, vec![500]);
    // An app that does not come: refused, no key goes to the other app.
    st.lock().unwrap().front = Some(900);
    st.lock().unwrap().activate_to = None;
    st.lock().unwrap().posted.clear();
    assert_eq!(
        server.input(Target::APP, &k).unwrap_err().code,
        Code::NotFront
    );
    assert!(st.lock().unwrap().posted.is_empty());
    // The desktop scope types into whatever is in front.
    assert!(!server.input(Target::DESKTOP, &k).unwrap().activated);
}

#[test]
fn a_bad_action_is_a_bad_request() {
    let (server, _, _) = fake::server(fake::scene());
    let k = Input::Key {
        combos: vec![Combo {
            mods: vec![],
            key: "nokey".into(),
        }],
    };
    assert_eq!(
        server.input(Target::DESKTOP, &k).unwrap_err().code,
        Code::BadRequest
    );
}

fn el(id: &str, role: &str) -> Element {
    Element {
        id: id.into(),
        role: role.into(),
        subrole: String::new(),
        label: "Save".into(),
        value: None,
        frame: Frame::new(120.0, 130.0, 60.0, 20.0),
        enabled: true,
        focused: false,
        depth: 1,
    }
}

#[test]
fn ax_reads_the_app_windows_or_the_app_windows_on_the_desktop() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().elements = vec![el("10:0", "AXButton"), el("10:1", "AXButton")];
    st.lock().unwrap().on_screen = vec![
        raw(10, 500, "Notes", Frame::new(100.0, 100.0, 800.0, 600.0)),
        RawWindow {
            layer: 25,
            ..raw(2, 0, "Window Server", Frame::new(0.0, 0.0, 1440.0, 24.0))
        },
    ];
    let (g, e, cut) = server.ax(Target::APP, None, Some(1)).unwrap();
    assert_eq!(g.origin, pt(100.0, 100.0));
    assert_eq!(e.len(), 1);
    assert!(cut);
    server.ax(Target::DESKTOP, Some(99), None).unwrap();
    let reads = st.lock().unwrap().ax_reads.clone();
    assert_eq!(reads[0], (vec![10, 11], cu::AX_DEPTH_DEFAULT, 1));
    assert_eq!(reads[1], (vec![10], cu::AX_DEPTH_MAX, cu::AX_NODES_DEFAULT));
}

#[test]
fn press_finds_the_window_of_the_id() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().elements = vec![el("10:0.2", "AXButton")];
    assert_eq!(
        server
            .press(Target::APP, "10:0.2", Some("AXButton"))
            .unwrap(),
        ("AXButton".to_string(), "Save".to_string())
    );
    assert_eq!(
        server
            .press(Target::APP, "nonsense", None)
            .unwrap_err()
            .code,
        Code::BadRequest
    );
    assert_eq!(
        server.press(Target::APP, "99:0", None).unwrap_err().code,
        Code::NoElement
    );
    assert_eq!(
        server
            .press(Target::APP, "10:0.2", Some("AXLink"))
            .unwrap_err()
            .code,
        Code::NoElement
    );
}

#[test]
fn status_names_the_app_its_windows_and_the_geometry() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().front = Some(501);
    let s = server.status(Target::APP);
    assert!(s.app_running && s.app_front);
    assert_eq!(s.windows.len(), 2);
    assert_eq!(s.geometry.unwrap().width, 1600);
    let s = Server::new(fake::FakeScreen::new().0, Arc::new(Scene::default)).status(Target::APP);
    assert!(!s.app_running);
    assert!(s.geometry.is_none());
}

// ---- The connection --------------------------------------------------------

/// A server on one end of a socket pair, in a thread: the other end.
fn connect(server: Server<fake::FakeScreen>) -> (UnixStream, BufReader<UnixStream>) {
    let (a, b) = UnixStream::pair().unwrap();
    std::thread::spawn(move || serve(server, b));
    a.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let r = BufReader::new(a.try_clone().unwrap());
    (a, r)
}

fn send(w: &mut UnixStream, id: u64, op: Op) {
    let mut line = serde_json::to_vec(&Call { id, op }).unwrap();
    line.push(b'\n');
    w.write_all(&line).unwrap();
}

fn recv(r: &mut BufReader<UnixStream>) -> (Msg, Vec<u8>) {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let m: Msg = serde_json::from_str(line.trim_end()).unwrap();
    let mut p = vec![0; m.bytes() as usize];
    r.read_exact(&mut p).unwrap();
    (m, p)
}

#[test]
fn a_connection_says_hello_and_answers_each_call_with_its_id() {
    let (server, _, _) = fake::server(fake::scene());
    let (mut w, mut r) = connect(server);
    match recv(&mut r).0 {
        Msg::Hello(h) => {
            assert_eq!(h.v, cu::VERSION);
            assert_eq!(h.scale, 2.0);
            assert!(h.screen_recording && h.accessibility);
        }
        m => panic!("{m:?}"),
    }
    send(
        &mut w,
        7,
        Op::Screenshot {
            target: Target::APP,
            max: None,
            cursor: false,
        },
    );
    match recv(&mut r) {
        (
            Msg::Reply {
                id: 7,
                outcome:
                    Outcome::Ok {
                        data: Some(Data::Shot(g)),
                        bytes: Some(n),
                    },
            },
            png,
        ) => {
            assert_eq!(g.width, 1600);
            assert_eq!(n as usize, png.len());
            assert!(png.starts_with(PNG));
        }
        m => panic!("{m:?}"),
    }
    // An unknown op gets an error with its id, and the connection lives.
    w.write_all(b"{\"id\":8,\"op\":\"teleport\"}\n").unwrap();
    match recv(&mut r).0 {
        Msg::Reply {
            id: 8,
            outcome: Outcome::Error(f),
        } => assert_eq!(f.code, Code::Unsupported),
        m => panic!("{m:?}"),
    }
    send(
        &mut w,
        9,
        Op::Status {
            target: Target::DESKTOP,
        },
    );
    assert!(matches!(
        recv(&mut r).0,
        Msg::Reply {
            id: 9,
            outcome: Outcome::Ok {
                data: Some(Data::Status(_)),
                ..
            }
        }
    ));
}

#[test]
fn ax_sends_the_elements_as_json_after_the_line() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().elements = vec![el("10:0", "AXButton")];
    let (mut w, mut r) = connect(server);
    recv(&mut r);
    send(
        &mut w,
        1,
        Op::Ax {
            target: Target::APP,
            depth: None,
            max_nodes: None,
        },
    );
    match recv(&mut r) {
        (
            Msg::Reply {
                outcome:
                    Outcome::Ok {
                        data:
                            Some(Data::Ax {
                                count: 1,
                                cut: false,
                                ..
                            }),
                        ..
                    },
                ..
            },
            json,
        ) => {
            let e: Vec<Element> = serde_json::from_slice(&json).unwrap();
            assert_eq!(e[0].id, "10:0");
        }
        m => panic!("{m:?}"),
    }
}

/// The next frame, skipping nothing: a reply here fails the test.
fn frame(r: &mut BufReader<UnixStream>) -> (FrameHeader, Vec<u8>) {
    match recv(r) {
        (Msg::Frame(h), p) => (h, p),
        m => panic!("{m:?}"),
    }
}

#[test]
fn frames_come_only_on_change_and_stop_names_the_last() {
    let (server, st, _) = fake::server(fake::scene());
    let (mut w, mut r) = connect(server);
    recv(&mut r);
    send(
        &mut w,
        1,
        Op::FramesSubscribe {
            target: Target::APP,
            max_fps: 30.0,
            max: Some(MaxSize::of(800, 800)),
            cursor: true,
        },
    );
    match recv(&mut r).0 {
        Msg::Reply {
            id: 1,
            outcome:
                Outcome::Ok {
                    data: Some(Data::Subscribed(g)),
                    ..
                },
        } => {
            assert_eq!((g.width, g.height), (800, 600));
        }
        m => panic!("{m:?}"),
    }
    let (h, png) = frame(&mut r);
    assert_eq!(h.seq, 1);
    assert_eq!((h.width, h.height), (800, 600));
    assert_eq!(png, [PNG, &[1, 2, 3]].concat());
    // Unchanged pixels: captures go on, but no frame and no encoding.
    std::thread::sleep(Duration::from_millis(150));
    let before = st.lock().unwrap().captures.len();
    assert!(before >= 2, "{before} captures");
    assert_eq!(st.lock().unwrap().encoded, 1);
    st.lock().unwrap().pixels = vec![9, 9];
    let (h, png) = frame(&mut r);
    assert_eq!(h.seq, 2);
    assert!(png.ends_with(&[9, 9]));
    send(&mut w, 2, Op::FramesStop);
    // Frames sent before the stop may come first; none after its reply.
    loop {
        match recv(&mut r).0 {
            Msg::Frame(_) => continue,
            Msg::Reply {
                id: 2,
                outcome:
                    Outcome::Ok {
                        data: Some(Data::Stopped { last_seq }),
                        ..
                    },
            } => {
                assert_eq!(last_seq, 2);
                break;
            }
            m => panic!("{m:?}"),
        }
    }
    let n = st.lock().unwrap().captures.len();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        st.lock().unwrap().captures.len(),
        n,
        "no capture after the stop"
    );
}

#[test]
fn a_moved_window_is_a_new_frame_with_the_same_pixels() {
    let (server, _, sc) = fake::server(fake::scene());
    let (mut w, mut r) = connect(server);
    recv(&mut r);
    send(
        &mut w,
        1,
        Op::FramesSubscribe {
            target: Target::APP,
            max_fps: 30.0,
            max: None,
            cursor: false,
        },
    );
    recv(&mut r);
    let (h1, _) = frame(&mut r);
    for win in &mut sc.lock().unwrap().windows {
        win.frame.x += 10.0;
    }
    let (h2, _) = frame(&mut r);
    assert_eq!(h2.seq, h1.seq + 1);
    assert_eq!(h2.geometry.origin.x, h1.geometry.origin.x + 10.0);
}

#[test]
fn input_shares_the_connection_with_a_subscription() {
    let (server, st, _) = fake::server(fake::scene());
    let (mut w, mut r) = connect(server);
    recv(&mut r);
    send(
        &mut w,
        1,
        Op::FramesSubscribe {
            target: Target::DESKTOP,
            max_fps: 5.0,
            max: None,
            cursor: true,
        },
    );
    recv(&mut r);
    send(
        &mut w,
        2,
        Op::Input {
            target: Target::DESKTOP,
            action: Input::Move { at: pt(5.0, 5.0) },
        },
    );
    loop {
        match recv(&mut r).0 {
            Msg::Frame(_) => continue,
            Msg::Reply {
                id: 2,
                outcome:
                    Outcome::Ok {
                        data: Some(Data::Input(d)),
                        ..
                    },
            } => {
                assert_eq!(d.events, 1);
                break;
            }
            m => panic!("{m:?}"),
        }
    }
    assert_eq!(st.lock().unwrap().posted.len(), 1);
}

#[test]
fn a_subscription_waits_out_a_failed_capture() {
    let (server, st, sc) = fake::server(fake::scene());
    sc.lock().unwrap().windows.clear();
    let (mut w, mut r) = connect(server);
    recv(&mut r);
    // No window now: the subscribe itself says so.
    send(
        &mut w,
        1,
        Op::FramesSubscribe {
            target: Target::APP,
            max_fps: 30.0,
            max: None,
            cursor: false,
        },
    );
    match recv(&mut r).0 {
        Msg::Reply {
            outcome: Outcome::Error(f),
            ..
        } => assert_eq!(f.code, Code::NoWindow),
        m => panic!("{m:?}"),
    }
    *sc.lock().unwrap() = fake::scene();
    send(
        &mut w,
        2,
        Op::FramesSubscribe {
            target: Target::APP,
            max_fps: 30.0,
            max: None,
            cursor: false,
        },
    );
    recv(&mut r);
    frame(&mut r);
    // The window goes, then comes back: the stream goes on.
    st.lock().unwrap().capture_error = Some(Failure::new(Code::Capture, "busy"));
    std::thread::sleep(Duration::from_millis(100));
    st.lock().unwrap().capture_error = None;
    st.lock().unwrap().pixels = vec![4];
    let (h, _) = frame(&mut r);
    assert_eq!(h.seq, 2);
}

#[test]
fn a_long_line_ends_the_connection_with_an_error() {
    let (server, _, _) = fake::server(fake::scene());
    let (mut w, mut r) = connect(server);
    recv(&mut r);
    let long = vec![b'x'; cu::LINE_MAX + 10];
    let _ = w.write_all(&long);
    match recv(&mut r).0 {
        Msg::Reply {
            id: 0,
            outcome: Outcome::Error(f),
        } => assert_eq!(f.code, Code::BadRequest),
        m => panic!("{m:?}"),
    }
}

#[test]
fn the_published_scene_is_the_one_read() {
    let sc = fake::scene();
    publish(sc.clone());
    assert_eq!(scene(), sc);
}

#[test]
fn the_scene_of_the_follow_has_pids_only_while_the_app_lives() {
    let mut f = crate::guest::windows::Follow::default();
    let app = "com.example.notes".to_string();
    let s = scene_of(Some(&app), &f, &[]);
    assert!(s.pids.is_empty() && s.root.is_none());
    f.root = Some(500);
    f.procs.insert(500, 1);
    let s = scene_of(Some(&app), &f, &fake::scene().windows);
    assert_eq!(s.pids, [500].into_iter().collect());
    assert_eq!(s.windows.len(), 2);
}

fn combo(mods: &[Mod], key: &str) -> Combo {
    Combo {
        mods: mods.to_vec(),
        key: key.into(),
    }
}

#[test]
fn system_shortcuts_are_refused_for_the_app_windows() {
    let (server, st, _) = fake::server(fake::scene());
    for c in [
        combo(&[Mod::Cmd], "space"),
        combo(&[Mod::Cmd], "tab"),
        combo(&[Mod::Cmd, Mod::Option], "escape"),
        combo(&[Mod::Cmd, Mod::Ctrl], "q"),
        combo(&[Mod::Ctrl], "up"),
        combo(&[Mod::Ctrl], "f3"),
        combo(&[Mod::Fn], ""),
    ] {
        // A harmless combo first: nothing of the action is sent.
        let k = Input::Key {
            combos: vec![combo(&[Mod::Cmd], "s"), c.clone()],
        };
        let e = server.input(Target::APP, &k).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{c:?}");
        assert!(e.text.contains("shortcut of the system"), "{}", e.text);
        assert!(st.lock().unwrap().posted.is_empty(), "{c:?}");
        // The whole display of the session is the user's grant: it goes.
        assert!(server.input(Target::DESKTOP, &k).is_ok(), "{c:?}");
        st.lock().unwrap().posted.clear();
    }
    // The shortcuts of the app still go.
    let k = Input::Key {
        combos: vec![
            combo(&[Mod::Cmd], "s"),
            combo(&[Mod::Cmd, Mod::Shift], "t"),
            combo(&[Mod::Cmd], "grave"),
            combo(&[Mod::Ctrl], "tab"),
            combo(&[], "space"),
        ],
    };
    assert!(server.input(Target::APP, &k).is_ok());
}

#[test]
fn keys_stop_when_the_app_leaves_the_front() {
    let (server, st, _) = fake::server(fake::scene());
    // The first combo (cmd down, s down, s up, cmd up) brings another
    // app to the front.
    st.lock().unwrap().front_after = Some((4, 900));
    let k = Input::Key {
        combos: vec![combo(&[Mod::Cmd], "o"), combo(&[], "return")],
    };
    let e = server.input(Target::APP, &k).unwrap_err();
    assert_eq!(e.code, Code::NotFront);
    assert!(e.text.contains("after 1 of 2 steps"), "{}", e.text);
    assert_eq!(st.lock().unwrap().posted.len(), 4, "the return never went");
    // Text: each chunk checks again.
    st.lock().unwrap().front = Some(500);
    st.lock().unwrap().posted.clear();
    st.lock().unwrap().front_after = Some((1, 900));
    let t = Input::Type {
        text: format!("{}\nrm -rf ~", "x".repeat(25)),
    };
    let e = server.input(Target::APP, &t).unwrap_err();
    assert_eq!(e.code, Code::NotFront);
    let posted = st.lock().unwrap().posted.clone();
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(matches!(&posted[0], Low::Text(t) if t.chars().count() == 20));
}

#[test]
fn system_shortcuts_are_refused_by_an_alias_of_their_key() {
    let (server, st, _) = fake::server(fake::scene());
    for c in [
        combo(&[Mod::Cmd], "s_pace"),
        combo(&[Mod::Cmd], "t_ab"),
        combo(&[Mod::Ctrl], "le_ft"),
        combo(&[Mod::Ctrl], "f_3"),
        combo(&[Mod::Cmd, Mod::Option], "es_c"),
        combo(&[Mod::Cmd], "SPACE"),
    ] {
        let k = Input::Key {
            combos: vec![c.clone(), combo(&[], "return")],
        };
        let e = server.input(Target::APP, &k).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{c:?}");
        assert!(e.text.contains("shortcut of the system"), "{}", e.text);
        assert!(st.lock().unwrap().posted.is_empty(), "{c:?}");
    }
}

#[test]
fn the_help_menu_shortcut_is_refused_for_the_app_windows() {
    let (server, st, _) = fake::server(fake::scene());
    for c in [
        combo(&[Mod::Cmd, Mod::Shift], "/"),
        combo(&[Mod::Shift, Mod::Cmd], "slash"),
        combo(&[Mod::Cmd, Mod::Shift], "S_LASH"),
        combo(&[Mod::Ctrl], "f2"),
        combo(&[Mod::Ctrl], "F_2"),
    ] {
        let k = Input::Key {
            combos: vec![c.clone(), combo(&[], "left"), combo(&[], "return")],
        };
        let e = server.input(Target::APP, &k).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{c:?}");
        assert!(st.lock().unwrap().posted.is_empty(), "{c:?}");
    }
    // cmd+/ without shift is a shortcut of many apps: it goes.
    let k = Input::Key {
        combos: vec![combo(&[Mod::Cmd], "/")],
    };
    assert!(server.input(Target::APP, &k).is_ok());
}

/// An open menu of the app, as the window server lists it.
fn menu_window() -> RawWindow {
    let mut w = raw(77, 500, "Notes", Frame::new(120.0, 24.0, 220.0, 300.0));
    w.layer = MENU_LAYER;
    w
}

#[test]
fn no_keys_or_clicks_go_while_a_menu_is_open() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().on_screen = vec![
        menu_window(),
        raw(10, 500, "Notes", Frame::new(100.0, 100.0, 800.0, 600.0)),
    ];
    let keys = |names: &[&str]| Input::Key {
        combos: names.iter().map(|n| combo(&[], n)).collect(),
    };
    let on_menu = Point { x: 200.0, y: 150.0 };
    let on_app = Point { x: 600.0, y: 500.0 };
    for action in [
        keys(&["left"]),
        keys(&["down", "down", "return"]),
        keys(&["return"]),
        keys(&["escape", "return"]),
        Input::Type {
            text: "lock".into(),
        },
        Input::Click {
            at: on_menu,
            button: Button::Left,
            count: 1,
            mods: vec![],
        },
        Input::Click {
            at: on_app,
            button: Button::Left,
            count: 1,
            mods: vec![],
        },
    ] {
        let e = server.input(Target::APP, &action).unwrap_err();
        assert_eq!(e.code, Code::Covered, "{action:?}");
        assert!(e.text.contains("a menu is open"), "{}", e.text);
        assert!(st.lock().unwrap().posted.is_empty(), "{action:?}");
    }
    // Escape closes the menu: it goes.
    assert!(server.input(Target::APP, &keys(&["escape"])).is_ok());
    assert!(server.input(Target::APP, &keys(&["ESC", "esc"])).is_ok());
    st.lock().unwrap().posted.clear();
    // The menu closed: keys and clicks go again.
    st.lock().unwrap().on_screen.remove(0);
    assert!(server
        .input(Target::APP, &keys(&["down", "return"]))
        .is_ok());
    let click = Input::Click {
        at: on_app,
        button: Button::Left,
        count: 1,
        mods: vec![],
    };
    assert!(server.input(Target::APP, &click).is_ok());
    assert!(!st.lock().unwrap().posted.is_empty());
}

#[test]
fn keys_stop_when_a_combo_opens_a_menu() {
    let (server, st, _) = fake::server(fake::scene());
    // The first combo (cmd down, o down, o up, cmd up) opens a menu.
    st.lock().unwrap().menu_after = Some((4, menu_window()));
    let k = Input::Key {
        combos: vec![
            combo(&[Mod::Cmd], "o"),
            combo(&[], "left"),
            combo(&[], "return"),
        ],
    };
    let e = server.input(Target::APP, &k).unwrap_err();
    assert_eq!(e.code, Code::Covered);
    assert!(e.text.contains("after 1 of 3 steps"), "{}", e.text);
    assert_eq!(
        st.lock().unwrap().posted.len(),
        4,
        "left and return never went"
    );
}

#[test]
fn the_menu_bar_takes_no_clicks_for_the_app_windows() {
    let mut sc = fake::scene();
    // A window at the top of the display, under the menu bar.
    sc.windows
        .push(fake::window(12, 500, Frame::new(0.0, 0.0, 1440.0, 900.0)));
    let (server, st, _) = fake::server(sc);
    let mut bar = raw(3, 90, "Window Server", Frame::new(0.0, 0.0, 1440.0, 30.0));
    bar.layer = 24;
    st.lock().unwrap().on_screen = vec![
        bar,
        raw(12, 500, "Notes", Frame::new(0.0, 0.0, 1440.0, 900.0)),
    ];
    for (x, y) in [(10.0, 5.0), (60.0, 12.0), (700.0, 27.0)] {
        let click = Input::Click {
            at: Point { x, y },
            button: Button::Left,
            count: 1,
            mods: vec![],
        };
        let e = server.input(Target::APP, &click).unwrap_err();
        assert_eq!(e.code, Code::Outside, "{x},{y}");
        assert!(e.text.contains("menu bar"), "{}", e.text);
        assert!(server.input(Target::DESKTOP, &click).is_ok());
    }
    let click = Input::Click {
        at: Point { x: 700.0, y: 400.0 },
        button: Button::Left,
        count: 1,
        mods: vec![],
    };
    assert!(server.input(Target::APP, &click).is_ok());
}

#[test]
fn a_failed_post_releases_the_modifiers_of_the_combo() {
    let (server, st, _) = fake::server(fake::scene());
    // cmd down, shift down, then the key down fails.
    st.lock().unwrap().fail_post = Some(3);
    let k = Input::Key {
        combos: vec![combo(&[Mod::Cmd, Mod::Shift], "k")],
    };
    let e = server.input(Target::DESKTOP, &k).unwrap_err();
    assert_eq!(e.code, Code::Input);
    let posted = st.lock().unwrap().posted.clone();
    let ups: Vec<(u16, u64)> = posted
        .iter()
        .filter_map(|l| match l {
            Low::Modifier {
                code,
                down: false,
                flags,
            } => Some((*code, *flags)),
            _ => None,
        })
        .collect();
    let (cmd, shift) = (ci::mod_code(Mod::Cmd), ci::mod_code(Mod::Shift));
    assert_eq!(
        ups,
        vec![(shift, ci::mod_flag(Mod::Cmd)), (cmd, 0)],
        "{posted:?}"
    );
    // A failure after the key down also releases the key.
    st.lock().unwrap().posted.clear();
    st.lock().unwrap().tries = 0;
    st.lock().unwrap().fail_post = Some(4);
    let e = server.input(Target::DESKTOP, &k).unwrap_err();
    assert_eq!(e.code, Code::Input);
    let posted = st.lock().unwrap().posted.clone();
    assert!(
        posted
            .iter()
            .any(|l| matches!(l, Low::Key { down: false, .. })),
        "{posted:?}"
    );
    assert!(matches!(
        posted.last(),
        Some(Low::Modifier {
            down: false,
            flags: 0,
            ..
        })
    ));
}

fn drag(button: Button, mods: Vec<Mod>) -> Input {
    Input::Drag {
        from: Point { x: 200.0, y: 200.0 },
        to: Point { x: 260.0, y: 320.0 },
        button,
        mods,
    }
}

#[test]
fn a_drag_that_opens_a_context_menu_is_refused_for_the_app_windows() {
    let (server, st, _) = fake::server(fake::scene());
    for action in [
        drag(Button::Right, vec![]),
        drag(Button::Left, vec![Mod::Ctrl]),
        drag(Button::Middle, vec![Mod::Ctrl, Mod::Shift]),
    ] {
        let e = server.input(Target::APP, &action).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{action:?}");
        assert!(e.text.contains("context menu"), "{}", e.text);
        assert!(st.lock().unwrap().posted.is_empty(), "{action:?}");
    }
    // A left drag goes, and in desktop scope a right drag goes.
    assert!(server
        .input(Target::APP, &drag(Button::Left, vec![]))
        .is_ok());
    assert!(server
        .input(Target::DESKTOP, &drag(Button::Right, vec![]))
        .is_ok());
}

#[test]
fn a_menu_that_the_press_of_a_drag_opens_closes_before_the_let_go() {
    let (server, st, _) = fake::server(fake::scene());
    // The press (move, down) opens a menu, as a pop-up button does.
    st.lock().unwrap().menu_after = Some((2, menu_window()));
    let e = server
        .input(Target::APP, &drag(Button::Left, vec![]))
        .unwrap_err();
    assert_eq!(e.code, Code::Covered);
    assert!(e.text.contains("after 1 of 2 steps"), "{}", e.text);
    let posted = st.lock().unwrap().posted.clone();
    assert!(
        !posted.iter().any(|l| matches!(l, Low::Drag { .. })),
        "the pointer never moved with the button down: {posted:?}"
    );
    // Escape, then the let-go at the press point.
    let esc = posted
        .iter()
        .position(|l| {
            matches!(
                l,
                Low::Key {
                    code: 53,
                    down: true,
                    ..
                }
            )
        })
        .expect("escape went");
    let up = posted
        .iter()
        .position(|l| matches!(l, Low::Up { .. }))
        .expect("the button went up");
    assert!(esc < up, "{posted:?}");
    assert!(
        matches!(posted[up], Low::Up { at, .. } if at == Point { x: 200.0, y: 200.0 }),
        "{posted:?}"
    );
}

#[test]
fn a_failed_post_in_a_drag_lets_go_of_the_button() {
    let (server, st, _) = fake::server(fake::scene());
    // Move, down, two moves with the button down, then a post fails.
    st.lock().unwrap().fail_post = Some(5);
    let e = server
        .input(Target::DESKTOP, &drag(Button::Left, vec![]))
        .unwrap_err();
    assert_eq!(e.code, Code::Input);
    let posted = st.lock().unwrap().posted.clone();
    let last_drag = posted
        .iter()
        .rev()
        .find_map(|l| match l {
            Low::Drag { at, .. } => Some(*at),
            _ => None,
        })
        .expect("a drag event");
    assert!(
        matches!(posted.last(), Some(Low::Up { at, button: Button::Left, .. }) if *at == last_drag),
        "{posted:?}"
    );
}

#[test]
fn the_help_menu_shortcut_is_refused_on_the_layout_of_the_guest() {
    let (server, st, _) = fake::server(fake::scene());
    // German: `?` is shift and the key right of 0 (`-` on the US layout).
    st.lock().unwrap().layout = Some(vec![(27, true, '?'), (27, false, 'ß'), (44, true, '_')]);
    let k = Input::Key {
        combos: vec![combo(&[Mod::Cmd, Mod::Shift], "-"), combo(&[], "left")],
    };
    let e = server.input(Target::APP, &k).unwrap_err();
    assert_eq!(e.code, Code::BadRequest);
    assert!(st.lock().unwrap().posted.is_empty());
    // The same key without cmd, and cmd+ß, go.
    for c in [combo(&[Mod::Shift], "-"), combo(&[Mod::Cmd], "-")] {
        let k = Input::Key { combos: vec![c] };
        assert!(server.input(Target::APP, &k).is_ok());
    }
    // A layout that cannot say: the keys of `?` on the common layouts.
    st.lock().unwrap().layout = None;
    for key in ["-", "m"] {
        let k = Input::Key {
            combos: vec![combo(&[Mod::Cmd, Mod::Shift], key)],
        };
        assert_eq!(
            server.input(Target::APP, &k).unwrap_err().code,
            Code::BadRequest,
            "{key}"
        );
    }
}

#[test]
fn press_is_refused_while_a_menu_is_open() {
    let (server, st, _) = fake::server(fake::scene());
    st.lock().unwrap().elements = vec![el("10:0.2", "AXButton")];
    st.lock().unwrap().on_screen = vec![
        menu_window(),
        raw(10, 500, "Notes", Frame::new(100.0, 100.0, 800.0, 600.0)),
    ];
    let e = server.press(Target::APP, "10:0.2", None).unwrap_err();
    assert_eq!(e.code, Code::Covered);
    assert!(st.lock().unwrap().pressed.is_empty());
    // The desktop scope reaches the menus anyway.
    assert!(server.press(Target::DESKTOP, "10:0.2", None).is_ok());
}
