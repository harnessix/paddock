//! The wire of computer use: the geometry, the messages on the wire, the
//! element ids and the change detection of the frame stream.

use super::*;

fn region() -> Frame {
    Frame::new(100.0, 50.0, 800.0, 600.0)
}

#[test]
fn a_plan_at_the_display_scale_has_the_native_pixels() {
    let g = Geometry::plan(region(), 2.0, None);
    assert_eq!((g.width, g.height), (1600, 1200));
    assert_eq!(g.origin, Point { x: 100.0, y: 50.0 });
    assert_eq!(g.region(), region());
}

#[test]
fn a_max_size_scales_down_and_keeps_the_aspect() {
    let g = Geometry::plan(region(), 2.0, Some(MaxSize::of(1000, 1000)));
    assert_eq!((g.width, g.height), (1000, 750));
    let g = Geometry::plan(region(), 2.0, Some(MaxSize::of(4000, 300)));
    assert_eq!((g.width, g.height), (400, 300));
    // Never above the native pixels.
    let g = Geometry::plan(region(), 1.0, Some(MaxSize::of(4000, 4000)));
    assert_eq!((g.width, g.height), (800, 600));
}

#[test]
fn the_agent_size_fits_the_long_side_and_the_pixel_budget() {
    let full = Frame::new(0.0, 0.0, 2560.0, 1600.0);
    let g = Geometry::plan(full, 2.0, Some(MaxSize::AGENT));
    assert!(g.width <= 1568 && g.height <= 1568, "{g:?}");
    let px = u64::from(g.width) * u64::from(g.height);
    assert!(px <= 1_152_000, "{px}");
    assert!((f64::from(g.width) / f64::from(g.height) - 1.6).abs() < 0.01);
    // A small window keeps its native pixels.
    let g = Geometry::plan(
        Frame::new(0.0, 0.0, 400.0, 300.0),
        2.0,
        Some(MaxSize::AGENT),
    );
    assert_eq!((g.width, g.height), (800, 600));
}

#[test]
fn a_bad_scale_or_an_empty_region_still_gives_an_image() {
    let g = Geometry::plan(Frame::new(0.0, 0.0, 0.0, 0.0), f64::NAN, None);
    assert_eq!((g.width, g.height), (1, 1));
    let g = Geometry::plan(region(), -3.0, None);
    assert_eq!((g.width, g.height), (800, 600));
}

#[test]
fn pixels_map_to_guest_points_and_back() {
    let g = Geometry::plan(region(), 2.0, Some(MaxSize::of(1000, 1000)));
    let p = g.to_guest(500.0, 375.0);
    assert_eq!(p, Point { x: 500.0, y: 350.0 });
    assert_eq!(g.to_image(p), (500.0, 375.0));
    let f = g.frame_to_image(Frame::new(100.0, 50.0, 80.0, 40.0));
    assert_eq!(f, Frame::new(0.0, 0.0, 100.0, 50.0));
    assert!(g.holds_pixel(0.0, 0.0) && g.holds_pixel(999.9, 749.9));
    assert!(!g.holds_pixel(1000.0, 10.0) && !g.holds_pixel(-0.1, 10.0));
}

#[test]
fn same_compares_region_and_size() {
    let a = Geometry::plan(region(), 2.0, None);
    let mut b = a;
    b.origin.x += 0.4;
    assert!(a.same(&b));
    b.origin.x += 1.0;
    assert!(!a.same(&b));
    let c = Geometry::plan(region(), 1.0, None);
    assert!(!a.same(&c));
}

#[test]
fn union_and_clip() {
    assert_eq!(union(Vec::<Frame>::new()), None);
    let u = union([
        Frame::new(10.0, 10.0, 10.0, 10.0),
        Frame::new(0.0, 15.0, 5.0, 20.0),
    ])
    .unwrap();
    assert_eq!(u, Frame::new(0.0, 10.0, 20.0, 25.0));
    let d = Display { w: 100, h: 50 };
    assert_eq!(
        clip(Frame::new(-10.0, 40.0, 50.0, 50.0), d),
        Some(Frame::new(0.0, 40.0, 40.0, 10.0))
    );
    assert_eq!(clip(Frame::new(200.0, 0.0, 5.0, 5.0), d), None);
    assert!(holds(
        &Frame::new(0.0, 0.0, 10.0, 10.0),
        Point { x: 0.0, y: 9.9 }
    ));
    assert!(!holds(
        &Frame::new(0.0, 0.0, 10.0, 10.0),
        Point { x: 10.0, y: 1.0 }
    ));
}

#[test]
fn the_calls_on_the_wire() {
    let c = Call {
        id: 3,
        op: Op::FramesSubscribe {
            target: Target::APP,
            max_fps: 10.0,
            max: None,
            cursor: true,
        },
    };
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"id": 3, "op": "frames.subscribe", "target": {"scope": "app"}, "max_fps": 10.0, "cursor": true})
    );
    // The spec form without the optional fields reads.
    let c: Call = serde_json::from_str(
        r#"{"id":4,"op":"frames.subscribe","target":{"scope":"desktop"},"max_fps":2}"#,
    )
    .unwrap();
    assert_eq!(
        c.op,
        Op::FramesSubscribe {
            target: Target::DESKTOP,
            max_fps: 2.0,
            max: None,
            cursor: true
        }
    );
    let c: Call = serde_json::from_str(r#"{"id":5,"op":"frames.stop"}"#).unwrap();
    assert_eq!(c.op, Op::FramesStop);
    let c: Call = serde_json::from_str(
        r#"{"id":6,"op":"input","target":{"scope":"app"},"action":{"kind":"click","at":{"x":1,"y":2}}}"#,
    )
    .unwrap();
    assert_eq!(
        c.op,
        Op::Input {
            target: Target::APP,
            action: Input::Click {
                at: Point { x: 1.0, y: 2.0 },
                button: Button::Left,
                count: 1,
                mods: vec![]
            }
        }
    );
    let c: Call = serde_json::from_str(r#"{"id":7,"op":"status"}"#).unwrap();
    assert_eq!(
        c.op,
        Op::Status {
            target: Target::DESKTOP
        }
    );
}

#[test]
fn the_messages_on_the_wire() {
    let f = Msg::Frame(FrameHeader {
        seq: 9,
        width: 10,
        height: 20,
        geometry: Geometry::default(),
        bytes: 123,
    });
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(v["ev"], "frame");
    assert_eq!(v["seq"], 9);
    assert_eq!(
        (v["width"].as_u64(), v["height"].as_u64()),
        (Some(10), Some(20))
    );
    assert_eq!(f.bytes(), 123);
    let r = Msg::Reply {
        id: 2,
        outcome: Outcome::Error(Failure::new(Code::Covered, "a\nb")),
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"ev": "reply", "id": 2, "result": "error", "code": "covered", "text": "a b"})
    );
    assert_eq!(r.bytes(), 0);
    let ok = Msg::Reply {
        id: 1,
        outcome: Outcome::Ok {
            data: Some(Data::Stopped { last_seq: 4 }),
            bytes: None,
        },
    };
    let text = serde_json::to_string(&ok).unwrap();
    assert_eq!(
        text,
        r#"{"ev":"reply","id":1,"result":"ok","data":{"stopped":{"last_seq":4}}}"#
    );
    let back: Msg = serde_json::from_str(&text).unwrap();
    assert_eq!(back, ok);
}

#[test]
fn every_code_has_its_wire_word() {
    for c in [
        Code::NoApp,
        Code::NoWindow,
        Code::Outside,
        Code::Covered,
        Code::NotFront,
        Code::Locked,
        Code::ScreenRecording,
        Code::Accessibility,
        Code::NoElement,
        Code::BadRequest,
        Code::Capture,
        Code::Input,
        Code::Timeout,
        Code::Unsupported,
        Code::Stale,
        Code::Unreachable,
        Code::Denied,
        Code::Internal,
    ] {
        assert_eq!(serde_json::to_value(c).unwrap(), c.word());
    }
}

#[test]
fn element_ids_round_trip() {
    assert_eq!(element_id(4512, &[0, 3, 1]), "4512:0.3.1");
    assert_eq!(parse_element_id("4512:0.3.1"), Some((4512, vec![0, 3, 1])));
    assert_eq!(parse_element_id(" 7: "), Some((7, vec![])));
    assert_eq!(element_id(7, &[]), "7:");
    for bad in ["", "x:1", "1:a", "1:0..1", "1"] {
        assert_eq!(parse_element_id(bad), None, "{bad}");
    }
}

#[test]
fn the_interval_follows_max_fps_within_its_bounds() {
    assert_eq!(frame_interval(10.0), Duration::from_millis(100));
    assert_eq!(
        frame_interval(1000.0),
        Duration::from_secs_f64(1.0 / FPS_MAX)
    );
    assert_eq!(frame_interval(0.0), Duration::from_secs(5));
    assert_eq!(
        frame_interval(f64::NAN),
        Duration::from_secs_f64(1.0 / FPS_MAX)
    );
}

#[test]
fn the_differ_numbers_only_changed_frames() {
    let g = Geometry::plan(region(), 1.0, None);
    let mut d = Differ::default();
    assert_eq!(d.last_seq(), 0);
    let a = Differ::digest(Differ::pixels(b"aaa"), &g);
    assert!(d.changed(a));
    assert_eq!(d.next(a), Some(1));
    assert!(!d.changed(a));
    assert_eq!(d.next(a), None);
    let b = Differ::digest(Differ::pixels(b"bbb"), &g);
    assert_eq!(d.next(b), Some(2));
    // The same pixels at another place are another frame.
    let mut moved = g;
    moved.origin.x += 1.0;
    let c = Differ::digest(Differ::pixels(b"bbb"), &moved);
    assert_eq!(d.next(c), Some(3));
    assert_eq!(d.last_seq(), 3);
}

#[test]
fn input_points_and_words() {
    let p = Point { x: 1.0, y: 2.0 };
    let q = Point { x: 3.0, y: 4.0 };
    let drag = Input::Drag {
        from: p,
        to: q,
        button: Button::Left,
        mods: vec![],
    };
    assert_eq!(drag.points(), vec![p, q]);
    assert_eq!(drag.point(), Some(p));
    assert!(!drag.is_keyboard());
    let t = Input::Type { text: "x".into() };
    assert!(t.points().is_empty() && t.is_keyboard());
    assert_eq!(t.word(), "type");
}

#[test]
fn timeouts_grow_with_the_text() {
    let short = Op::Input {
        target: Target::DESKTOP,
        action: Input::Type { text: "x".into() },
    };
    let long = Op::Input {
        target: Target::DESKTOP,
        action: Input::Type {
            text: "x".repeat(TYPE_MAX),
        },
    };
    assert!(long.timeout() > short.timeout());
    assert!(long.timeout() < Duration::from_secs(60));
    assert_eq!(Op::FramesStop.word(), "frames.stop");
}
