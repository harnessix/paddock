//! `paddock-host mac` and `paddock-host mcp mac`: computer use of an app
//! session in paddock's macOS guest from a sandbox. Each command is one
//! `mac` request to the host (`macvm::computer_broker`), which asks the
//! user unless a session grant covers the target. A screenshot lands in
//! the project inbox.

use crate::broker::{Request, Status, PROTOCOL};
use crate::macvm::computer::{Code, Failure};
use crate::macvm::computer_mcp::{self, Backend, ToolOut};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const HELP: &str = r#"paddock-host mac: drive an app session of paddock's macOS guest, never the host.

USAGE
  paddock-host mac status       [TARGET]
  paddock-host mac screenshot   [TARGET] [--max-size agent|full|WxH] [--cursor]
  paddock-host mac click X Y    [TARGET] [--button left|right|middle] [--count N] [--mods cmd,shift]
  paddock-host mac move X Y     [TARGET]
  paddock-host mac drag X Y TO_X TO_Y [TARGET]
  paddock-host mac scroll X Y up|down|left|right [LINES] [TARGET]
  paddock-host mac type TEXT    [TARGET]
  paddock-host mac key COMBO... [TARGET]      (cmd+s, shift+tab, return, escape)
  paddock-host mac ax           [TARGET] [--all]
  paddock-host mac press ID     [TARGET] [--role ROLE]
  paddock-host mcp mac          [TARGET]      (the same tools as an MCP server on stdio)

TARGET
  --app NAME [--instance N]   only the windows of that app session
  --desktop                   the whole guest display of the session
  (none)                      the only running session, its whole display

Every command prints JSON. Coordinates are pixels of the last screenshot of
the same target. Work in a loop: screenshot, one action, screenshot again to
check the result. A refusal names its reason (covered, outside, stale,
locked, denied): follow it instead of repeating the call.

The first request asks the user on the host; "allow for this session" covers
every later request on the same target of the same running session. One type
has at most 1000 characters. In app scope, the known shortcuts of the system
(cmd+space, cmd+tab, ...) are refused, not one that the user changed. The
menu bar takes no clicks, a drag or a double or triple click with the right
button or ctrl is refused, and while a menu is open only escape goes. A
screenshot is written to .paddock/inbox/ as paddock-mac-SECONDS.png (the
newest 20 stay) and its path is in the output.
"#;

/// Send one tool to the host: its JSON, and the PNG of a screenshot.
pub fn call(tool: &str, args: &Value, reason: &str) -> Result<ToolOut, Failure> {
    let mut a = BTreeMap::new();
    a.insert("tool".to_string(), json!(tool));
    a.insert("args".to_string(), args.clone());
    let req = Request {
        v: PROTOCOL,
        cap: "mac".into(),
        args: a,
        reason: reason.to_string(),
    };
    let resp = super::send(&req).map_err(|e| Failure::new(Code::Unreachable, e))?;
    match resp.status {
        Status::Done => {
            let png = match resp.result.get("png").and_then(Value::as_str) {
                Some(p) => Some(std::fs::read(p).map_err(|e| {
                    Failure::new(Code::Internal, format!("{p}: {e}")) // not user text
                })?),
                None => None,
            };
            let mut json = resp.result.get("json").cloned().unwrap_or(Value::Null);
            if let (Some(p), Value::Object(o)) = (resp.result.get("png"), &mut json) {
                o.insert("path".into(), p.clone());
            }
            Ok(ToolOut { json, png })
        }
        Status::Failed if resp.result.get("code").is_some() => {
            let code: Code =
                serde_json::from_value(resp.result["code"].clone()).unwrap_or(Code::Internal);
            let text = resp.result["error"].as_str().unwrap_or(&resp.message);
            Err(Failure::new(code, text))
        }
        Status::Denied | Status::Disabled | Status::Timeout => Err(Failure::new(
            Code::Denied,
            crate::macvm::text::cu_denied(&resp.message),
        )),
        _ => Err(Failure::new(Code::Internal, resp.message)),
    }
}

/// The tools through the host broker.
pub struct BrokerBackend {
    pub default: Map<String, Value>,
}

impl Backend for BrokerBackend {
    fn call(&mut self, tool: &str, args: &Value) -> Result<ToolOut, Failure> {
        let mut a = self.default.clone();
        if let Value::Object(o) = args {
            for (k, v) in o {
                a.insert(k.clone(), v.clone());
            }
        }
        call(tool, &Value::Object(a), "")
    }
}

/// The target flags and the other flags of a command line.
struct Line {
    target: Map<String, Value>,
    opts: BTreeMap<String, String>,
    flags: Vec<String>,
    words: Vec<String>,
    reason: String,
}

fn parse(args: &[String]) -> Result<Line, String> {
    let with_value = [
        "app",
        "instance",
        "max-size",
        "button",
        "count",
        "mods",
        "role",
        "depth",
        "max-nodes",
        "reason",
    ];
    let mut l = Line {
        target: Map::new(),
        opts: BTreeMap::new(),
        flags: Vec::new(),
        words: Vec::new(),
        reason: String::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            l.words.extend(args[i + 1..].iter().cloned());
            break;
        }
        match a.strip_prefix("--") {
            Some(name) if with_value.contains(&name) => {
                let v = args.get(i + 1).ok_or(format!("{a} needs a value"))?.clone();
                match name {
                    "app" => {
                        l.target.insert("app".into(), json!(v));
                    }
                    "instance" => {
                        let n: u64 = v
                            .parse()
                            .map_err(|_| format!("--instance {v}: a number from 1 to 99"))?;
                        l.target.insert("instance".into(), json!(n));
                    }
                    "reason" => l.reason = v,
                    _ => {
                        l.opts.insert(name.to_string(), v);
                    }
                }
                i += 1;
            }
            Some("desktop") => {
                l.target.insert("desktop".into(), json!(true));
            }
            Some(name) if ["all", "cursor", "double", "json"].contains(&name) => {
                l.flags.push(name.to_string())
            }
            Some(_) if a.len() > 2 && !a[2..].starts_with(|c: char| c.is_ascii_digit()) => {
                return Err(format!("unknown option {a}"))
            }
            _ => l.words.push(a.clone()),
        }
        i += 1;
    }
    Ok(l)
}

fn number(words: &[String], i: usize, what: &str) -> Result<f64, String> {
    words
        .get(i)
        .ok_or(format!("{what} is missing"))?
        .parse::<f64>()
        .map_err(|_| format!("{what} must be a number"))
}

/// The tool and its arguments of a command line.
fn tool_of(cmd: &str, l: &Line) -> Result<(&'static str, Value), String> {
    let mut a = l.target.clone();
    let w = &l.words;
    let mods = |a: &mut Map<String, Value>| {
        if let Some(m) = l.opts.get("mods") {
            let list: Vec<&str> = m.split(',').map(str::trim).collect();
            a.insert("mods".into(), json!(list));
        }
        if let Some(b) = l.opts.get("button") {
            a.insert("button".into(), json!(b));
        }
    };
    let tool = match cmd {
        "status" => "mac_status",
        "screenshot" => {
            if let Some(m) = l.opts.get("max-size") {
                a.insert("max_size".into(), json!(m));
            }
            if l.flags.iter().any(|f| f == "cursor") {
                a.insert("cursor".into(), json!(true));
            }
            "mac_screenshot"
        }
        "click" => {
            a.insert("x".into(), json!(number(w, 0, "X")?));
            a.insert("y".into(), json!(number(w, 1, "Y")?));
            let count = if l.flags.iter().any(|f| f == "double") {
                2
            } else {
                l.opts
                    .get("count")
                    .map(|c| {
                        c.parse::<u64>()
                            .map_err(|_| "--count must be a number".to_string())
                    })
                    .transpose()?
                    .unwrap_or(1)
            };
            a.insert("count".into(), json!(count));
            mods(&mut a);
            "mac_click"
        }
        "move" => {
            a.insert("x".into(), json!(number(w, 0, "X")?));
            a.insert("y".into(), json!(number(w, 1, "Y")?));
            "mac_move"
        }
        "drag" => {
            for (i, k) in ["x", "y", "to_x", "to_y"].iter().enumerate() {
                a.insert((*k).into(), json!(number(w, i, &k.to_uppercase())?));
            }
            mods(&mut a);
            "mac_drag"
        }
        "scroll" => {
            a.insert("x".into(), json!(number(w, 0, "X")?));
            a.insert("y".into(), json!(number(w, 1, "Y")?));
            a.insert(
                "direction".into(),
                json!(w
                    .get(2)
                    .ok_or("the direction (up, down, left, right) is missing")?),
            );
            if let Some(n) = w.get(3) {
                let n: i64 = n
                    .parse()
                    .map_err(|_| "LINES must be a number".to_string())?;
                a.insert("amount".into(), json!(n));
            }
            "mac_scroll"
        }
        "type" => {
            if w.is_empty() {
                return Err("type needs the text".into());
            }
            a.insert("text".into(), json!(w.join(" ")));
            "mac_type"
        }
        "key" => {
            if w.is_empty() {
                return Err("key needs at least one key, for example cmd+s".into());
            }
            a.insert("keys".into(), json!(w));
            "mac_key"
        }
        "ax" | "elements" => {
            if l.flags.iter().any(|f| f == "all") {
                a.insert("all".into(), json!(true));
            }
            for (o, k) in [("depth", "depth"), ("max-nodes", "max_nodes")] {
                if let Some(v) = l.opts.get(o) {
                    let n: u64 = v.parse().map_err(|_| format!("--{o} must be a number"))?;
                    a.insert(k.into(), json!(n));
                }
            }
            "mac_elements"
        }
        "press" => {
            a.insert(
                "element".into(),
                json!(w
                    .first()
                    .ok_or("press needs an element id from paddock-host mac ax")?),
            );
            if let Some(r) = l.opts.get("role") {
                a.insert("role".into(), json!(r));
            }
            "mac_press"
        }
        other => return Err(format!("unknown mac command {other}")),
    };
    Ok((tool, Value::Object(a)))
}

fn print(v: &Value) {
    println!("{}", crate::macvm::computer_host::json_text(v, false));
}

/// `paddock-host mac ...`.
pub fn main(args: &[String]) -> i32 {
    let Some(cmd) = args.first() else {
        print!("{HELP}");
        return super::EXIT_USAGE;
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help")
        || args.iter().any(|a| a == "--help" || a == "-h")
    {
        print!("{HELP}");
        return 0;
    }
    let l = match parse(&args[1..]) {
        Ok(l) => l,
        Err(e) => return usage(&e),
    };
    let (tool, a) = match tool_of(cmd, &l) {
        Ok(t) => t,
        Err(e) => return usage(&e),
    };
    match call(tool, &a, &l.reason) {
        Ok(out) => {
            print(&out.json);
            0
        }
        Err(f) => {
            print(&crate::macvm::computer_host::failure_json(&f));
            eprintln!(
                "paddock-host mac: {}",
                crate::macvm::computer_host::failure_line(&f)
            );
            if f.code == Code::Denied {
                3
            } else {
                1
            }
        }
    }
}

fn usage(msg: &str) -> i32 {
    eprintln!("paddock-host mac: {msg}\nRun `paddock-host mac --help`.");
    super::EXIT_USAGE
}

/// `paddock-host mcp mac [TARGET]`: the MCP server through the host.
pub fn mcp_main(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("mac") => {}
        _ => {
            eprintln!(
                "paddock-host mcp: the only server is mac: paddock-host mcp mac [--app NAME]"
            );
            return super::EXIT_USAGE;
        }
    }
    let l = match parse(&args[1..]) {
        Ok(l) => l,
        Err(e) => return usage(&e),
    };
    let mut b = BrokerBackend { default: l.target };
    let stdin = std::io::stdin();
    match computer_mcp::serve(&mut b, stdin.lock(), std::io::stdout()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("paddock-host mcp mac: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> Line {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn commands_become_tools() {
        let (t, a) = tool_of(
            "click",
            &line(&[
                "10",
                "20",
                "--app",
                "notes",
                "--double",
                "--mods",
                "cmd,shift",
            ]),
        )
        .unwrap();
        assert_eq!(t, "mac_click");
        assert_eq!(
            a,
            json!({"x": 10.0, "y": 20.0, "app": "notes", "count": 2, "mods": ["cmd", "shift"]})
        );
        let (t, a) = tool_of("scroll", &line(&["1", "2", "down", "5", "--desktop"])).unwrap();
        assert_eq!(t, "mac_scroll");
        assert_eq!(a["amount"], 5);
        assert_eq!(a["desktop"], true);
        let (t, a) = tool_of("type", &line(&["hello", "world"])).unwrap();
        assert_eq!((t, a["text"].as_str()), ("mac_type", Some("hello world")));
        let (t, a) = tool_of("key", &line(&["cmd+s", "return", "--instance", "2"])).unwrap();
        assert_eq!(t, "mac_key");
        assert_eq!(a["keys"], json!(["cmd+s", "return"]));
        assert_eq!(a["instance"], 2);
        let (t, a) = tool_of("ax", &line(&["--all", "--depth", "5"])).unwrap();
        assert_eq!(
            (t, a["all"].as_bool(), a["depth"].as_u64()),
            ("mac_elements", Some(true), Some(5))
        );
        let (t, _) = tool_of("screenshot", &line(&["--max-size", "full"])).unwrap();
        assert_eq!(t, "mac_screenshot");
    }

    #[test]
    fn negative_numbers_are_words_and_bad_lines_are_refused() {
        let (_, a) = tool_of("move", &line(&["-5", "3"])).unwrap();
        assert_eq!(a["x"], -5.0);
        assert!(parse(&["--fly".to_string()]).is_err());
        assert!(tool_of("click", &line(&["1"])).is_err());
        assert!(tool_of("click", &line(&["a", "b"])).is_err());
        assert!(tool_of("type", &line(&[])).is_err());
        assert!(tool_of("teleport", &line(&[])).is_err());
        assert!(parse(&["--instance".to_string(), "x".to_string()]).is_err());
    }

    #[test]
    fn every_tool_of_a_command_is_a_tool_of_the_server() {
        let names: Vec<String> = computer_mcp::tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        for (cmd, words) in [
            ("status", vec![]),
            ("screenshot", vec![]),
            ("click", vec!["1", "2"]),
            ("move", vec!["1", "2"]),
            ("drag", vec!["1", "2", "3", "4"]),
            ("scroll", vec!["1", "2", "up"]),
            ("type", vec!["x"]),
            ("key", vec!["a"]),
            ("ax", vec![]),
            ("press", vec!["1:0"]),
        ] {
            let (t, _) = tool_of(cmd, &line(&words)).unwrap();
            assert!(names.contains(&t.to_string()), "{cmd}");
        }
    }
}
