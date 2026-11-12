//! The host commands of computer use: the choice of the session, the
//! stored geometry, the pixels of an action, and a driver on a fake guest.

use super::*;
use crate::guest::computer::serve;
use crate::guest::computer_fake::{self as fake, PNG};
use crate::macvm::proto::Frame;
use std::os::unix::net::UnixStream;

fn session(app: &str, instance: u32, pid: u32) -> Session {
    serde_json::from_value(json!({
        "pid": pid,
        "app": app,
        "home": "default",
        "profile": format!("/p/{app}"),
        "started": 1,
        "control": format!("/run/{app}.sock"),
        "instance": instance,
    }))
    .unwrap()
}

fn pick(app: Option<&str>, instance: Option<u32>, desktop: bool) -> Pick {
    Pick {
        app: app.map(str::to_string),
        instance,
        desktop,
    }
}

#[test]
fn the_only_session_is_the_desktop_without_app() {
    let run = vec![session("notes", 1, 10)];
    let c = choose(&run, &Pick::default()).unwrap();
    assert_eq!(c.session.app, "notes");
    assert_eq!(c.target.scope, Scope::Desktop);
    assert_eq!(choose(&[], &Pick::default()).unwrap_err().code, Code::NoApp);
    let e = choose(&run, &pick(None, Some(2), false)).unwrap_err();
    assert!(e.text.contains("--instance needs --app"), "{}", e.text);
}

#[test]
fn several_sessions_need_a_name() {
    let run = vec![session("mail", 1, 10), session("notes", 2, 11)];
    let e = choose(&run, &Pick::default()).unwrap_err();
    assert_eq!(e.code, Code::BadRequest);
    assert!(e.text.contains("mail, notes --instance 2"), "{}", e.text);
}

#[test]
fn app_picks_its_instance_and_the_app_scope() {
    let run = vec![session("notes", 1, 10), session("notes", 2, 11)];
    let c = choose(&run, &pick(Some("notes"), None, false)).unwrap();
    assert_eq!((c.session.pid, c.target.scope), (10, Scope::App));
    assert_eq!(c.name(), "notes");
    let c = choose(&run, &pick(Some("notes"), Some(2), true)).unwrap();
    assert_eq!((c.session.pid, c.target.scope), (11, Scope::Desktop));
    assert_eq!(c.name(), "notes 2");
    let e = choose(&run, &pick(Some("notes"), Some(3), false)).unwrap_err();
    assert_eq!(e.code, Code::NoApp);
    assert!(
        e.text.contains("notes --instance 3 does not run"),
        "{}",
        e.text
    );
}

fn native(x: f64, y: f64, w: f64, h: f64) -> Geometry {
    Geometry::plan(Frame::new(x, y, w, h), 2.0, None)
}

#[test]
fn without_a_screenshot_the_default_size_maps() {
    let g = through(None, native(0.0, 0.0, 2560.0, 1600.0)).unwrap();
    assert_eq!(
        g,
        Geometry::plan(
            Frame::new(0.0, 0.0, 2560.0, 1600.0),
            2.0,
            Some(MaxSize::AGENT)
        )
    );
}

#[test]
fn the_stored_geometry_follows_moved_windows_and_refuses_a_new_size() {
    let shot = Geometry::plan(
        Frame::new(100.0, 100.0, 800.0, 600.0),
        2.0,
        Some(MaxSize::of(400, 400)),
    );
    let g = through(Some(shot), native(100.0, 100.0, 800.0, 600.0)).unwrap();
    assert_eq!(g, shot);
    let g = through(Some(shot), native(150.0, 90.0, 800.0, 600.0)).unwrap();
    assert_eq!(g.origin, Point { x: 150.0, y: 90.0 });
    assert_eq!((g.width, g.height), (shot.width, shot.height));
    let e = through(Some(shot), native(100.0, 100.0, 900.0, 600.0)).unwrap_err();
    assert_eq!(e.code, Code::Stale);
}

#[test]
fn an_action_maps_its_pixels_and_refuses_pixels_off_the_image() {
    let g = Geometry::plan(
        Frame::new(100.0, 100.0, 800.0, 600.0),
        2.0,
        Some(MaxSize::of(400, 400)),
    );
    let act = Act::Click {
        x: 200.0,
        y: 150.0,
        button: Button::Left,
        count: 1,
        mods: vec![],
    };
    match act.to_input(&g).unwrap() {
        Input::Click { at, .. } => assert_eq!(at, Point { x: 500.0, y: 400.0 }),
        i => panic!("{i:?}"),
    }
    let off = Act::Move { x: 400.0, y: 10.0 };
    assert_eq!(off.to_input(&g).unwrap_err().code, Code::Outside);
    let drag = Act::Drag {
        x: 0.0,
        y: 0.0,
        to_x: 399.0,
        to_y: 299.0,
        button: Button::Left,
        mods: vec![],
    };
    assert_eq!(drag.to_input(&g).unwrap().points().len(), 2);
    let keys = Act::Key {
        combos: vec![parse_combo_ok("cmd+s")],
    };
    assert!(!keys.needs_point());
    assert!(keys.to_input(&Geometry::default()).unwrap().is_keyboard());
}

fn parse_combo_ok(t: &str) -> Combo {
    super::super::computer_input::parse_combo(t).unwrap()
}

#[test]
fn acts_read_from_json_with_defaults() {
    let a: Act = serde_json::from_value(json!({"action": "click", "x": 1, "y": 2})).unwrap();
    assert_eq!(
        a,
        Act::Click {
            x: 1.0,
            y: 2.0,
            button: Button::Left,
            count: 1,
            mods: vec![]
        }
    );
    let a: Act =
        serde_json::from_value(json!({"action": "scroll", "x": 1, "y": 2, "dy": 3})).unwrap();
    assert!(matches!(a, Act::Scroll { dy: 3, dx: 0, .. }));
}

#[test]
fn the_store_is_per_session_and_scope_and_forgets_another_pid() {
    let t = tempfile::tempdir().unwrap();
    let c = choose(
        &[session("notes", 2, 10)],
        &pick(Some("notes"), Some(2), false),
    )
    .unwrap();
    let p = store_path(t.path(), &c, CLI);
    assert!(p.ends_with("notes--2-app.json"), "{}", p.display());
    let g = native(1.0, 2.0, 3.0, 4.0);
    assert_eq!(load(&p, 10), None);
    save(&p, 10, g);
    assert_eq!(load(&p, 10), Some(g));
    assert_eq!(load(&p, 11), None);
}

fn el(role: &str, label: &str, w: f64, depth: u32) -> Element {
    Element {
        id: "10:0".into(),
        role: role.into(),
        subrole: String::new(),
        label: label.into(),
        value: None,
        frame: Frame::new(110.0, 120.0, w, 20.0),
        enabled: true,
        focused: false,
        depth,
    }
}

#[test]
fn useful_keeps_labels_values_and_actions_with_an_area() {
    assert!(useful(&el("AXStaticText", "Hello", 10.0, 2)));
    assert!(useful(&el("AXButton", "", 10.0, 2)));
    assert!(!useful(&el("AXGroup", "", 10.0, 2)));
    assert!(!useful(&el("AXButton", "OK", 0.0, 2)));
    assert!(useful(&el("AXWindow", "", 10.0, 0)));
    let mut v = el("AXGroup", "", 10.0, 2);
    v.value = Some("3".into());
    assert!(useful(&v));
}

#[test]
fn an_element_in_pixels_of_the_image() {
    let g = Geometry::plan(
        Frame::new(100.0, 100.0, 800.0, 600.0),
        2.0,
        Some(MaxSize::of(400, 400)),
    );
    let mut e = el("AXButton", "Save", 40.0, 1);
    e.enabled = false;
    let v = element_json(&e, &g);
    assert_eq!(
        v["frame"],
        json!({"x": 5.0, "y": 10.0, "w": 20.0, "h": 10.0})
    );
    assert_eq!(v["center"], json!({"x": 15.0, "y": 15.0}));
    assert_eq!(v["label"], "Save");
    assert_eq!(v["enabled"], false);
    assert!(v.get("visible").is_none());
    let mut off = e.clone();
    off.frame.x = 2000.0;
    assert_eq!(element_json(&off, &g)["visible"], false);
}

#[test]
fn scroll_words_and_max_sizes() {
    assert_eq!(scroll_lines("down", 3).unwrap(), (0, 3));
    assert_eq!(scroll_lines("UP", 0).unwrap(), (0, -1));
    assert_eq!(scroll_lines("left", 2).unwrap(), (-2, 0));
    assert_eq!(
        scroll_lines("sideways", 2).unwrap_err().code,
        Code::BadRequest
    );
    assert_eq!(parse_max("full").unwrap(), None);
    assert_eq!(parse_max("agent").unwrap(), Some(MaxSize::AGENT));
    assert_eq!(parse_max("1280x800").unwrap(), Some(MaxSize::of(1280, 800)));
    assert_eq!(parse_max("1024").unwrap(), Some(MaxSize::of(1024, 1024)));
    for bad in ["0", "12x", "big", "99999x10"] {
        assert!(parse_max(bad).is_err(), "{bad}");
    }
}

fn driver(
    scope: Scope,
) -> (
    Driver,
    std::sync::Arc<std::sync::Mutex<fake::FakeState>>,
    tempfile::TempDir,
) {
    let (server, st, _) = fake::server(fake::scene());
    let (a, b) = UnixStream::pair().unwrap();
    std::thread::spawn(move || serve(server, b));
    let conn = Conn::over(a).unwrap();
    let chosen = Chosen {
        session: session("notes", 1, std::process::id()),
        target: Target { scope },
    };
    let t = tempfile::tempdir().unwrap();
    (Driver::with(chosen, conn, t.path()), st, t)
}

#[test]
fn a_driver_clicks_where_the_screenshot_showed() {
    let (mut d, st, _t) = driver(Scope::App);
    let (v, png) = d.screenshot(Some(MaxSize::of(400, 400)), false).unwrap();
    assert!(png.starts_with(PNG));
    assert_eq!(
        (v["width"].as_u64(), v["height"].as_u64()),
        (Some(400), Some(300))
    );
    assert_eq!(v["session"], "notes");
    assert_eq!(v["scope"], "app");
    let v = d
        .act(&Act::Click {
            x: 200.0,
            y: 150.0,
            button: Button::Left,
            count: 1,
            mods: vec![],
        })
        .unwrap();
    assert_eq!(v["points"], json!([{"x": 500.0, "y": 400.0}]));
    assert_eq!(v["action"], "click");
    let posted = st.lock().unwrap().posted.clone();
    assert_eq!(
        posted[0],
        super::super::computer_input::Low::Move(Point { x: 500.0, y: 400.0 })
    );
    // A pixel off the screenshot: refused before the guest.
    let e = d.act(&Act::Move { x: 401.0, y: 1.0 }).unwrap_err();
    assert_eq!(e.code, Code::Outside);
}

#[test]
fn a_driver_status_carries_hints() {
    let (mut d, st, _t) = driver(Scope::App);
    st.lock().unwrap().screen_recording = false;
    st.lock().unwrap().locked = true;
    let v = d.status().unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["app_running"], true);
    let hints = v["hints"].as_array().unwrap();
    assert_eq!(hints.len(), 2, "{hints:?}");
    let e = d.screenshot(None, false).unwrap_err();
    assert_eq!(e.code, Code::ScreenRecording);
    assert_eq!(failure_json(&e)["code"], "screen_recording");
}

#[test]
fn a_driver_lists_useful_elements_in_pixels() {
    let (mut d, st, _t) = driver(Scope::App);
    st.lock().unwrap().elements = vec![el("AXButton", "Save", 40.0, 1), el("AXGroup", "", 40.0, 1)];
    let v = d.ax(false, None, None).unwrap();
    assert_eq!(v["count"], 1);
    assert_eq!(v["read"], 2);
    // No screenshot yet: the default size of 800x600 points at scale 2.
    assert_eq!(v["image"], json!({"width": 1238, "height": 929}));
    let v = d.ax(true, None, None).unwrap();
    assert_eq!(v["count"], 2);
    let v = d.press("10:0", Some("AXButton")).unwrap();
    assert_eq!(v["label"], "Save");
}

#[test]
fn each_client_maps_through_its_own_screenshot() {
    let (server, _st, _) = fake::server(fake::scene());
    let t = tempfile::tempdir().unwrap();
    let open = |client: &str| {
        let (a, b) = UnixStream::pair().unwrap();
        let s = server.clone();
        std::thread::spawn(move || serve(s, b));
        let chosen = Chosen {
            session: session("notes", 1, std::process::id()),
            target: Target { scope: Scope::App },
        };
        Driver::with(chosen, Conn::over(a).unwrap(), t.path()).client(client)
    };
    let mut agent = open("sandbox-a1");
    let mut user = open(CLI);
    agent
        .screenshot(Some(MaxSize::of(400, 400)), false)
        .unwrap();
    // The user takes a full-size screenshot of the same target meanwhile.
    let (v, _) = user.screenshot(None, false).unwrap();
    assert_eq!(v["width"], 1600);
    // The agent's pixel still lands where its own screenshot showed it.
    let v = agent
        .act(&Act::Click {
            x: 200.0,
            y: 150.0,
            button: Button::Left,
            count: 1,
            mods: vec![],
        })
        .unwrap();
    assert_eq!(v["points"], json!([{"x": 500.0, "y": 400.0}]));
    let v = user.act(&Act::Move { x: 800.0, y: 600.0 }).unwrap();
    assert_eq!(v["points"], json!([{"x": 500.0, "y": 400.0}]));
}

#[test]
fn guest_text_reaches_a_terminal_without_escapes() {
    let v = json!({"title": "a\u{9b}2Jb\u{7f}", "line": "x\ny"});
    let text = json_text(&v, false);
    assert!(!text.chars().any(char::is_control), "{text:?}");
    let back: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back, v, "the JSON keeps its values");
    assert!(json_text(&v, true).contains('\n'));
    let f = Failure::new(
        Code::Covered,
        "a window of \u{1b}]0;x\u{7}\u{1b}[2Jevil covers it",
    );
    assert_eq!(failure_line(&f), "a window of evil covers it");
}
