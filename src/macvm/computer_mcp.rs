//! `paddock mcp mac`: computer use of a guest session as an MCP server
//! on stdio (newline-delimited JSON-RPC 2.0), with the tools of `paddock
//! mac`. The tools run through a [`Backend`]: on the host the session's
//! guest directly ([`HostBackend`]), in a sandbox the host broker
//! (`paddock-host mcp mac`), which runs the same [`run_tool`] on the host.
//!
//! A screenshot comes back as an MCP image with its geometry as text, so
//! the model sees the picture and the size that its coordinates refer to.
//! An action can ask for a screenshot after it (`screenshot_after`), which
//! saves one round trip of the observe, act, verify loop.

use super::computer::{Button, Code, Failure, Mod};
use super::computer_host::{self as ch, Act, Driver, Pick};
use super::computer_input::{parse_combo, parse_mod};
use super::text;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::time::Duration;

/// The MCP version that this server speaks when the client names none.
pub const PROTOCOL: &str = "2025-06-18";
/// The default pause before the screenshot after an action.
pub const SETTLE_MS: u64 = 400;
/// The longest settle pause.
pub const SETTLE_MAX_MS: u64 = 5000;

/// What a tool gives back: its JSON, and a PNG for a screenshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOut {
    pub json: Value,
    pub png: Option<Vec<u8>>,
}

/// Where the tools run.
pub trait Backend {
    fn call(&mut self, tool: &str, args: &Value) -> Result<ToolOut, Failure>;
}

/// The properties that every tool takes: the target.
fn target_props() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "app".into(),
        json!({"type": "string", "description": "The app session (its paddock app name). Without it: the only running session, or the default of the server."}),
    );
    m.insert(
        "instance".into(),
        json!({"type": "integer", "minimum": 1, "description": "The instance of the app session, 1 by default."}),
    );
    m.insert(
        "desktop".into(),
        json!({"type": "boolean", "description": "Work on the whole guest display, not only the app's windows."}),
    );
    m
}

fn schema(props: Value, required: &[&str]) -> Value {
    let mut all = target_props();
    if let Value::Object(p) = props {
        all.extend(p);
    }
    json!({"type": "object", "properties": all, "required": required, "additionalProperties": false})
}

fn after_props() -> Value {
    json!({
        "screenshot_after": {"type": "boolean", "description": "Take a screenshot after the action and return it, to verify the result."},
        "settle_ms": {"type": "integer", "minimum": 0, "maximum": SETTLE_MAX_MS, "description": "The wait before that screenshot, 400 ms by default."}
    })
}

fn merge(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Object(mut a), Value::Object(b)) => {
            a.extend(b);
            Value::Object(a)
        }
        (a, _) => a,
    }
}

fn point_props() -> Value {
    json!({
        "x": {"type": "number", "description": "Pixels from the left of the last screenshot."},
        "y": {"type": "number", "description": "Pixels from the top of the last screenshot."}
    })
}

fn mods_prop() -> Value {
    json!({"type": "array", "items": {"type": "string", "enum": ["cmd", "shift", "option", "ctrl", "fn"]}, "description": "Modifier keys held during the action."})
}

/// The tools with their descriptions and input schemas.
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "mac_status",
            "description": "The state of the macOS guest session: display size and scale, permissions, whether the screen is locked, the app, its windows, and the size of the last screenshot. Call it first.",
            "inputSchema": schema(json!({}), &[]),
        }),
        json!({
            "name": "mac_screenshot",
            "description": "A PNG of the target: the app's windows only (cropped to them), or the whole guest display with desktop=true. Coordinates of the other tools are pixels of the last screenshot of the same target. The default size fits a model's image limit.",
            "inputSchema": schema(json!({
                "max_size": {"type": "string", "description": "agent (default), full for the native pixels, or WIDTHxHEIGHT."},
                "cursor": {"type": "boolean", "description": "Draw the pointer."}
            }), &[]),
        }),
        json!({
            "name": "mac_click",
            "description": "Click at a pixel of the last screenshot. count 2 is a double click. In app scope a point outside the app's windows, or covered by another window, is refused.",
            "inputSchema": schema(merge(merge(point_props(), json!({
                "button": {"type": "string", "enum": ["left", "right", "middle"]},
                "count": {"type": "integer", "minimum": 1, "maximum": 3},
                "mods": mods_prop()
            })), after_props()), &["x", "y"]),
        }),
        json!({
            "name": "mac_move",
            "description": "Move the pointer to a pixel of the last screenshot (hover).",
            "inputSchema": schema(merge(point_props(), after_props()), &["x", "y"]),
        }),
        json!({
            "name": "mac_drag",
            "description": "Press at x,y, move to to_x,to_y, let go: pixels of the last screenshot.",
            "inputSchema": schema(merge(merge(point_props(), json!({
                "to_x": {"type": "number"},
                "to_y": {"type": "number"},
                "button": {"type": "string", "enum": ["left", "right", "middle"]},
                "mods": mods_prop()
            })), after_props()), &["x", "y", "to_x", "to_y"]),
        }),
        json!({
            "name": "mac_scroll",
            "description": "Scroll at a pixel of the last screenshot by lines.",
            "inputSchema": schema(merge(merge(point_props(), json!({
                "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                "amount": {"type": "integer", "minimum": 1, "maximum": 200, "description": "Lines, 3 by default."}
            })), after_props()), &["x", "y", "direction"]),
        }),
        json!({
            "name": "mac_type",
            "description": "Type text into the focused field, as it is, on any keyboard layout. A newline is the Return key. In app scope the app comes to the front first.",
            "inputSchema": schema(merge(json!({"text": {"type": "string"}}), after_props()), &["text"]),
        }),
        json!({
            "name": "mac_key",
            "description": "Press keys one after the other: cmd+s, shift+tab, return, escape, left, f5. In app scope the app comes to the front first.",
            "inputSchema": schema(merge(json!({"keys": {"type": "array", "items": {"type": "string"}, "minItems": 1}}), after_props()), &["keys"]),
        }),
        json!({
            "name": "mac_elements",
            "description": "The accessibility elements of the target: role, label, value, frame and center in pixels of the last screenshot, and an id for mac_press. Only useful elements unless all=true.",
            "inputSchema": schema(json!({
                "all": {"type": "boolean"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 40},
                "max_nodes": {"type": "integer", "minimum": 1, "maximum": 5000}
            }), &[]),
        }),
        json!({
            "name": "mac_press",
            "description": "Press an element by its id from mac_elements (the AXPress action), without moving the pointer. With role, only if the element still has that role.",
            "inputSchema": schema(merge(json!({
                "element": {"type": "string"},
                "role": {"type": "string"}
            }), after_props()), &["element"]),
        }),
    ]
}

fn bad(text: String) -> Failure {
    Failure::new(Code::BadRequest, text)
}

fn num(args: &Value, k: &str) -> Result<f64, Failure> {
    args.get(k)
        .and_then(Value::as_f64)
        .ok_or_else(|| bad(text::cu_arg_missing(k)))
}

fn opt_str<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn button(args: &Value) -> Result<Button, Failure> {
    match opt_str(args, "button") {
        None => Ok(Button::Left),
        Some(b) => serde_json::from_value(json!(b)).map_err(|_| bad(text::cu_button_invalid(b))),
    }
}

fn mods(args: &Value) -> Result<Vec<Mod>, Failure> {
    let Some(list) = args.get("mods").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for m in list {
        let s = m.as_str().unwrap_or("");
        let m = parse_mod(s).ok_or_else(|| bad(text::cu_mod_unknown(s, s)))?;
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out.sort();
    Ok(out)
}

/// The action of a tool call, or None for a tool that is not an action.
pub fn act_of(tool: &str, args: &Value) -> Result<Option<Act>, Failure> {
    Ok(Some(match tool {
        "mac_click" => Act::Click {
            x: num(args, "x")?,
            y: num(args, "y")?,
            button: button(args)?,
            count: args.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
            mods: mods(args)?,
        },
        "mac_move" => Act::Move {
            x: num(args, "x")?,
            y: num(args, "y")?,
        },
        "mac_drag" => Act::Drag {
            x: num(args, "x")?,
            y: num(args, "y")?,
            to_x: num(args, "to_x")?,
            to_y: num(args, "to_y")?,
            button: button(args)?,
            mods: mods(args)?,
        },
        "mac_scroll" => {
            let dir =
                opt_str(args, "direction").ok_or_else(|| bad(text::cu_arg_missing("direction")))?;
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(3) as i32;
            let (dx, dy) = ch::scroll_lines(dir, amount)?;
            Act::Scroll {
                x: num(args, "x")?,
                y: num(args, "y")?,
                dx,
                dy,
            }
        }
        "mac_type" => Act::Type {
            text: opt_str(args, "text")
                .ok_or_else(|| bad(text::cu_arg_missing("text")))?
                .to_string(),
        },
        "mac_key" => {
            let keys = args
                .get("keys")
                .and_then(Value::as_array)
                .ok_or_else(|| bad(text::cu_arg_missing("keys")))?;
            let mut combos = Vec::new();
            for k in keys {
                combos.push(parse_combo(k.as_str().unwrap_or("")).map_err(bad)?);
            }
            Act::Key { combos }
        }
        _ => return Ok(None),
    }))
}

/// The target of a call: the default of the server, with the call's
/// `app`, `instance` and `desktop` over it.
pub fn pick_of(default: &Pick, args: &Value) -> Pick {
    let mut p = default.clone();
    if let Some(app) = opt_str(args, "app") {
        p.app = Some(app.to_string());
        p.instance = None;
    }
    if let Some(n) = args.get("instance").and_then(Value::as_u64) {
        p.instance = Some(n as u32);
    }
    if let Some(d) = args.get("desktop").and_then(Value::as_bool) {
        p.desktop = d;
    }
    p
}

/// One tool on a driver.
pub fn run_tool(d: &mut Driver, tool: &str, args: &Value) -> Result<ToolOut, Failure> {
    let after = args
        .get("screenshot_after")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let settle = args
        .get("settle_ms")
        .and_then(Value::as_u64)
        .unwrap_or(SETTLE_MS)
        .min(SETTLE_MAX_MS);
    let mut out = match tool {
        "mac_status" => ToolOut {
            json: d.status()?,
            png: None,
        },
        "mac_screenshot" => {
            let max = ch::parse_max(opt_str(args, "max_size").unwrap_or("agent"))?;
            let cursor = args.get("cursor").and_then(Value::as_bool).unwrap_or(false);
            let (json, png) = d.screenshot(max, cursor)?;
            return Ok(ToolOut {
                json,
                png: Some(png),
            });
        }
        "mac_elements" => ToolOut {
            json: d.ax(
                args.get("all").and_then(Value::as_bool).unwrap_or(false),
                args.get("depth").and_then(Value::as_u64).map(|v| v as u32),
                args.get("max_nodes")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32),
            )?,
            png: None,
        },
        "mac_press" => ToolOut {
            json: d.press(
                opt_str(args, "element").ok_or_else(|| bad(text::cu_arg_missing("element")))?,
                opt_str(args, "role"),
            )?,
            png: None,
        },
        other => match act_of(other, args)? {
            Some(act) => ToolOut {
                json: d.act(&act)?,
                png: None,
            },
            None => return Err(bad(text::cu_tool_unknown(other))),
        },
    };
    if after {
        // The action ran: a failed screenshot after it is part of the
        // result, never a failure of the action, which the model would
        // then repeat.
        std::thread::sleep(Duration::from_millis(settle));
        let shot = d.screenshot(Some(super::computer::MaxSize::AGENT), false);
        if let Value::Object(o) = &mut out.json {
            match shot {
                Ok((shot, png)) => {
                    o.insert("screenshot".into(), shot);
                    out.png = Some(png);
                }
                Err(f) => {
                    o.insert("screenshot_error".into(), ch::failure_json(&f));
                }
            }
        }
    }
    Ok(out)
}

/// The tools on the host: a driver per target, kept while it works.
pub struct HostBackend {
    pub env: crate::env::HostEnv,
    pub default: Pick,
    cached: Option<(Pick, Driver)>,
    /// The client of this server: its screenshots are its own.
    client: String,
}

impl HostBackend {
    pub fn new(env: crate::env::HostEnv, default: Pick) -> Self {
        HostBackend {
            env,
            default,
            cached: None,
            client: format!("mcp-{}", std::process::id()), // not user text
        }
    }
}

impl Backend for HostBackend {
    fn call(&mut self, tool: &str, args: &Value) -> Result<ToolOut, Failure> {
        let pick = pick_of(&self.default, args);
        // A kept connection is checked first, so a broken one (the agent
        // restarted, the session resumed) is opened again before the tool,
        // and no action runs twice.
        let alive = match &mut self.cached {
            Some((p, d)) if *p == pick => d.conn.status(d.chosen.target).is_ok(),
            _ => false,
        };
        if !alive {
            self.cached = None;
            self.cached = Some((
                pick.clone(),
                Driver::open_as(&self.env, &pick, &self.client)?,
            ));
        }
        let (_, d) = self.cached.as_mut().expect("a driver");
        let r = run_tool(d, tool, args);
        if matches!(&r, Err(f) if matches!(f.code, Code::Unreachable | Code::Timeout)) {
            self.cached = None;
        }
        r
    }
}

/// The MCP result of a tool: the JSON as text, the PNG as an image, and
/// `isError` for a failure.
pub fn result_of(r: Result<ToolOut, Failure>) -> Value {
    match r {
        Ok(out) => {
            let mut content = vec![json!({"type": "text", "text": out.json.to_string()})];
            if let Some(png) = out.png {
                content.push(json!({
                    "type": "image",
                    "mimeType": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                }));
            }
            json!({"content": content, "isError": false})
        }
        Err(f) => json!({
            "content": [{"type": "text", "text": ch::failure_json(&f).to_string()}],
            "isError": true,
        }),
    }
}

/// The answer to one JSON-RPC message, None for a notification.
pub fn handle(backend: &mut dyn Backend, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?;
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
    let err = |code: i64, message: String| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
    Some(match method {
        "initialize" => {
            let version = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL);
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "paddock-mac", "version": super::version::paddock_version()},
                "instructions": text::cu_mcp_instructions(),
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !tools().iter().any(|t| t["name"] == name) {
                return Some(err(-32602, text::cu_tool_unknown(name)));
            }
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            ok(result_of(backend.call(name, &args)))
        }
        other => err(-32601, text::cu_method_unknown(other)),
    })
}

/// Serve MCP on `input` and `output` until the input ends.
pub fn serve(
    backend: &mut dyn Backend,
    input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(backend, &msg),
            Err(e) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}}),
            ),
        };
        if let Some(a) = answer {
            let mut text = serde_json::to_vec(&a).map_err(std::io::Error::other)?;
            text.push(b'\n');
            output.write_all(&text)?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "computer_mcp_tests.rs"]
mod tests;
