//! The MCP server of computer use: the tool schemas, the JSON-RPC
//! answers, and the tools on a driver over a fake guest.

use super::*;
use crate::guest::computer::serve as guest_serve;
use crate::guest::computer_fake::{self as fake, PNG};
use crate::macvm::computer::{Scope, Target};
use crate::macvm::computer_client::Conn;
use crate::macvm::computer_host::Chosen;
use std::os::unix::net::UnixStream;

#[test]
fn every_tool_has_a_valid_schema() {
    let tools = tools();
    assert_eq!(tools.len(), 10);
    let mut names = std::collections::BTreeSet::new();
    for t in &tools {
        let name = t["name"].as_str().unwrap();
        assert!(name.starts_with("mac_"), "{name}");
        assert!(names.insert(name.to_string()), "{name} twice");
        assert!(t["description"].as_str().unwrap().len() > 20, "{name}");
        let s = &t["inputSchema"];
        assert_eq!(s["type"], "object", "{name}");
        let props = s["properties"].as_object().unwrap();
        for k in ["app", "instance", "desktop"] {
            assert!(props.contains_key(k), "{name} {k}");
        }
        for r in s["required"].as_array().unwrap() {
            assert!(props.contains_key(r.as_str().unwrap()), "{name} {r}");
        }
        for (k, v) in props {
            assert!(v.get("type").is_some(), "{name} {k}");
        }
    }
}

/// A backend that records the calls and answers with their arguments.
#[derive(Default)]
struct Echo {
    calls: Vec<(String, Value)>,
}

impl Backend for Echo {
    fn call(&mut self, tool: &str, args: &Value) -> Result<ToolOut, Failure> {
        self.calls.push((tool.to_string(), args.clone()));
        if tool == "mac_key" {
            return Err(Failure::new(Code::NotFront, "not front"));
        }
        Ok(ToolOut {
            json: json!({"ok": true, "tool": tool}),
            png: (tool == "mac_screenshot").then(|| b"PNG".to_vec()),
        })
    }
}

#[test]
fn initialize_list_and_call() {
    let mut b = Echo::default();
    let r = handle(&mut b, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "paddock-mac");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    let r = handle(
        &mut b,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .unwrap();
    assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 10);
    let r = handle(&mut b, &json!({"jsonrpc": "2.0", "id": "s", "method": "tools/call", "params": {"name": "mac_screenshot", "arguments": {"app": "notes"}}})).unwrap();
    assert_eq!(r["id"], "s");
    let content = r["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert_eq!(content[1]["data"], "UE5H");
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(b.calls[0].1["app"], "notes");
}

#[test]
fn a_failed_tool_is_an_error_result_with_its_code() {
    let mut b = Echo::default();
    let r = handle(&mut b, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "mac_key", "arguments": {"keys": ["a"]}}})).unwrap();
    assert_eq!(r["result"]["isError"], true);
    let text: Value =
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["code"], "not_front");
    assert_eq!(text["ok"], false);
}

#[test]
fn unknown_tools_methods_and_notifications() {
    let mut b = Echo::default();
    let r = handle(
        &mut b,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "mac_fly"}}),
    )
    .unwrap();
    assert_eq!(r["error"]["code"], -32602);
    let r = handle(
        &mut b,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"}),
    )
    .unwrap();
    assert_eq!(r["error"]["code"], -32601);
    assert!(handle(
        &mut b,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .is_none());
    let r = handle(
        &mut b,
        &json!({"jsonrpc": "2.0", "id": 6, "method": "ping"}),
    )
    .unwrap();
    assert_eq!(r["result"], json!({}));
    assert!(b.calls.is_empty());
}

#[test]
fn serve_answers_line_by_line() {
    let mut b = Echo::default();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
        "\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "not json\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"mac_status\"}}\n",
    );
    let mut out = Vec::new();
    serve(&mut b, input.as_bytes(), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(lines[1]["error"]["code"], -32700);
    assert_eq!(lines[2]["id"], 2);
    assert_eq!(b.calls[0].1, json!({}));
}

#[test]
fn tool_arguments_become_actions() {
    let a = act_of(
        "mac_click",
        &json!({"x": 1, "y": 2, "count": 2, "button": "right", "mods": ["shift", "cmd"]}),
    )
    .unwrap();
    assert_eq!(
        a,
        Some(Act::Click {
            x: 1.0,
            y: 2.0,
            button: Button::Right,
            count: 2,
            mods: vec![Mod::Cmd, Mod::Shift]
        })
    );
    let a = act_of("mac_scroll", &json!({"x": 1, "y": 2, "direction": "up"})).unwrap();
    assert!(matches!(a, Some(Act::Scroll { dy: -3, .. })));
    let a = act_of("mac_key", &json!({"keys": ["cmd+a", "delete"]})).unwrap();
    assert!(matches!(a, Some(Act::Key { combos }) if combos.len() == 2));
    assert!(act_of("mac_click", &json!({"x": 1})).is_err());
    assert!(act_of("mac_click", &json!({"x": 1, "y": 1, "button": "huge"})).is_err());
    assert!(act_of("mac_key", &json!({"keys": ["cmd+nokey"]})).is_err());
    assert!(act_of("mac_type", &json!({})).is_err());
    assert_eq!(act_of("mac_status", &json!({})).unwrap(), None);
}

#[test]
fn the_call_target_overrides_the_default() {
    let d = Pick {
        app: Some("notes".into()),
        instance: Some(2),
        desktop: false,
    };
    assert_eq!(pick_of(&d, &json!({})), d);
    let p = pick_of(&d, &json!({"app": "mail"}));
    assert_eq!((p.app.as_deref(), p.instance), (Some("mail"), None));
    let p = pick_of(&d, &json!({"desktop": true, "instance": 3}));
    assert!(p.desktop && p.instance == Some(3));
}

fn driver() -> (
    Driver,
    std::sync::Arc<std::sync::Mutex<fake::FakeState>>,
    tempfile::TempDir,
) {
    let (server, st, _) = fake::server(fake::scene());
    let (a, b) = UnixStream::pair().unwrap();
    std::thread::spawn(move || guest_serve(server, b));
    let session: crate::macvm::host::Session = serde_json::from_value(json!({
        "pid": std::process::id(), "app": "notes", "home": "default", "profile": "/p",
        "started": 1, "control": "/run/c.sock"
    }))
    .unwrap();
    let chosen = Chosen {
        session,
        target: Target { scope: Scope::App },
    };
    let t = tempfile::tempdir().unwrap();
    (
        Driver::with(chosen, Conn::over(a).unwrap(), t.path()),
        st,
        t,
    )
}

#[test]
fn run_tool_screenshots_and_acts_with_a_screenshot_after() {
    let (mut d, st, _t) = driver();
    let out = run_tool(&mut d, "mac_screenshot", &json!({"max_size": "400x400"})).unwrap();
    assert!(out.png.unwrap().starts_with(PNG));
    assert_eq!(out.json["width"], 400);
    let out = run_tool(
        &mut d,
        "mac_click",
        &json!({"x": 200, "y": 150, "screenshot_after": true, "settle_ms": 0}),
    )
    .unwrap();
    assert_eq!(out.json["points"], json!([{"x": 500.0, "y": 400.0}]));
    assert!(out.json["screenshot"]["width"].as_u64().unwrap() > 0);
    assert!(out.png.is_some());
    assert!(!st.lock().unwrap().posted.is_empty());
    let e = run_tool(&mut d, "mac_press", &json!({})).unwrap_err();
    assert_eq!(e.code, Code::BadRequest);
    let e = run_tool(&mut d, "mac_fly", &json!({})).unwrap_err();
    assert_eq!(e.code, Code::BadRequest);
}

#[test]
fn a_failed_screenshot_after_an_action_keeps_the_result_of_the_action() {
    let (mut d, st, _t) = driver();
    st.lock().unwrap().capture_error = Some(Failure::new(Code::Capture, "no frame"));
    let out = run_tool(
        &mut d,
        "mac_click",
        &json!({"x": 200, "y": 150, "screenshot_after": true, "settle_ms": 0}),
    )
    .unwrap();
    // The click ran: the model must not repeat it.
    assert_eq!(out.json["ok"], true);
    assert_eq!(out.json["action"], "click");
    assert!(!st.lock().unwrap().posted.is_empty());
    assert_eq!(out.json["screenshot_error"]["code"], "capture");
    assert!(out.png.is_none());
    assert!(!result_of(Ok(out))["isError"].as_bool().unwrap());
}
