//! Computer use from a sandbox: the `mac` capability of the host broker.
//! `paddock-host mac ...` and `paddock-host mcp mac` send one request per
//! tool: `{"tool": "mac_click", "args": {...}}`. The broker checks it, asks
//! the user (a session grant covers one target: the app session and its
//! scope), runs the tool on the host with [`run_tool`], and writes a
//! screenshot into the project inbox, so no image goes through the
//! response.
//!
//! The session is chosen once, before the user is asked: its name and its
//! pid go on the card and into the key of a session grant, and the tool
//! runs on that session or not at all. A grant never moves to a session
//! that started later.
//!
//! The audit keeps the tool, the target and the points, never typed text.

use super::computer::{Code, Failure};
use super::computer_host::{self as ch, Chosen, Driver, Pick};
use super::computer_mcp::{pick_of, run_tool, tools};
use super::host::Session;
use super::text;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The name of the capability.
pub const CAP: &str = "mac";
/// The most characters of one `mac_type` from a sandbox: the card shows
/// every one of them, so a grant never covers text that nobody saw.
pub const CARD_TYPE_MAX: usize = 1000;
/// The screenshots of the broker that stay in the inbox of a project.
pub const SHOTS_KEPT: usize = 20;

fn tool(args: &BTreeMap<String, Value>) -> &str {
    args.get("tool").and_then(Value::as_str).unwrap_or("")
}

fn tool_args(args: &BTreeMap<String, Value>) -> Value {
    args.get("args").cloned().unwrap_or_else(|| json!({}))
}

/// The target of a request.
pub fn pick(args: &BTreeMap<String, Value>) -> Pick {
    pick_of(&Pick::default(), &tool_args(args))
}

/// The checks before the user is asked.
pub fn validate(args: &BTreeMap<String, Value>) -> Result<(), String> {
    let t = tool(args);
    if !tools().iter().any(|x| x["name"] == t) {
        return Err(text::cu_tool_unknown(t));
    }
    if !tool_args(args).is_object() {
        return Err(text::cu_args_not_object());
    }
    if let Some(app) = pick(args).app {
        if !super::host::valid_name(&app) {
            return Err(text::bad_app_name(&app));
        }
    }
    if t == "mac_type" {
        let n = typed(&tool_args(args)).chars().count();
        if n > CARD_TYPE_MAX {
            return Err(text::cu_type_card_too_long(n, CARD_TYPE_MAX));
        }
    }
    Ok(())
}

fn typed(a: &Value) -> &str {
    a.get("text").and_then(Value::as_str).unwrap_or("")
}

/// The session and the scope of a request, chosen before the user is
/// asked.
#[derive(Debug, Clone)]
pub struct Planned {
    pub chosen: Chosen,
}

/// Choose the session of a request among the running ones.
pub fn plan(running: &[Session], args: &BTreeMap<String, Value>) -> Result<Planned, Failure> {
    Ok(Planned {
        chosen: ch::choose(running, &pick(args))?,
    })
}

/// Choose the session of a request among the sessions of the host.
pub fn plan_host(
    env: &crate::env::HostEnv,
    args: &BTreeMap<String, Value>,
) -> Result<Planned, Failure> {
    plan(&ch::running(&super::layout::Layout::new(env)), args)
}

/// The key of a session grant: the target of [`grant_key`] and the
/// session itself, its pid and its start, so a grant ends with its
/// session.
pub fn session_grant_key(args: &BTreeMap<String, Value>, p: &Planned) -> String {
    let s = &p.chosen.session;
    format!(
        "{}\0{}\0{}\0{}", // not user text
        grant_key(args),
        s.key(),
        s.pid,
        s.started
    )
}

/// The target in words: `notes`, `notes 2`, or the only session.
fn target_words(p: &Pick) -> (String, bool) {
    let name = match &p.app {
        Some(a) => super::instance::label(a, p.instance.unwrap_or(super::instance::FIRST)),
        None => String::new(),
    };
    let app_only = p.app.is_some() && !p.desktop;
    (name, app_only)
}

/// The key of a session grant: one target, its scope included, so a grant
/// for an app's windows never covers the whole guest display.
pub fn grant_key(args: &BTreeMap<String, Value>) -> String {
    let p = pick(args);
    let scope = if p.app.is_some() && !p.desktop {
        "app"
    } else {
        "desktop"
    };
    format!(
        "{CAP}\0{}\0{}\0{scope}", // not user text
        p.app.unwrap_or_default(),
        p.instance.unwrap_or(super::instance::FIRST)
    )
}

/// What the action does, in a few words, without typed text.
fn what(t: &str, a: &Value) -> String {
    let n = |k: &str| a.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    match t {
        "mac_click" | "mac_move" | "mac_scroll" => {
            format!("{t} at {:.0},{:.0}", n("x"), n("y")) // not user text
        }
        "mac_drag" => {
            let from = format!("{:.0},{:.0}", n("x"), n("y"));
            let to = format!("{:.0},{:.0}", n("to_x"), n("to_y"));
            [t, "from", &from, "to", &to].join(" ")
        }
        "mac_type" => {
            let len = a
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, |s| s.chars().count());
            format!("{t} of {len} characters") // not user text
        }
        "mac_key" => {
            let keys: Vec<&str> = a
                .get("keys")
                .and_then(Value::as_array)
                .map(|v| v.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            format!("{t} {}", keys.join(" ")) // not user text
        }
        "mac_press" => {
            let id = a.get("element").and_then(Value::as_str).unwrap_or("");
            [t, id].join(" ")
        }
        other => other.to_string(),
    }
}

/// The typed text as the card shows it: every character visible, a
/// Return as \u{23CE} and a Tab as \u{21E5} at the end of their line,
/// every other control or invisible character as `\u{...}`, so the
/// card never hides a key that is typed.
pub fn visible(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\u{23CE}\n");
            }
            '\t' => out.push('\u{21E5}'),
            c if c.is_control() || invisible(c) => {
                out.push_str(&format!("\\u{{{:04X}}}", c as u32)); // not user text
            }
            c => out.push(c),
        }
    }
    out
}

/// The characters that `broker::approval::clean_text` removes from a
/// card: format and direction marks.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// The summary and the detail of the card, for the session that the
/// request runs on.
pub fn card(args: &BTreeMap<String, Value>, planned: &Planned) -> (String, String) {
    let p = pick(args);
    let (name, app_only) = target_words(&p);
    let a = tool_args(args);
    let t = tool(args);
    let s = &planned.chosen.session;
    let session = text::cu_card_session(&s.label(), s.pid);
    let mut detail = text::cu_card_detail(&name, &session, app_only, &what(t, &a));
    if t == "mac_type" {
        // validate() refused more than the card shows.
        let typed = typed(&a);
        detail.push_str(&text::cu_card_typed_count(typed.chars().count()));
        detail.push_str(&text::cu_card_typed(&visible(typed)));
    }
    if t == "mac_press" && a.get("role").and_then(Value::as_str).is_none() {
        detail.push_str(&text::cu_card_press_unchecked());
    }
    (
        text::cu_card_summary(&planned.chosen.name(), app_only),
        detail,
    )
}

/// The summary without a session, for the lists of requests.
pub fn summary(args: &BTreeMap<String, Value>) -> String {
    let (name, app_only) = target_words(&pick(args));
    text::cu_card_summary(&name, app_only)
}

/// The line of the audit log: the tool, the target and the points.
pub fn audit(args: &BTreeMap<String, Value>) -> String {
    let p = pick(args);
    let (name, app_only) = target_words(&p);
    let scope = if app_only { "app" } else { "desktop" };
    let who = if name.is_empty() { "-" } else { name.as_str() };
    format!("mac {who} {scope}: {}", what(tool(args), &tool_args(args))) // not user text
}

/// The client of a sandbox: its screenshots and clicks are its own.
pub fn client(sandbox: &str) -> String {
    format!("sandbox-{sandbox}") // not user text
}

/// The audit line with the session that the card named.
pub fn audit_on(args: &BTreeMap<String, Value>, planned: &Planned) -> String {
    let s = &planned.chosen.session;
    format!("{} (session {} pid {})", audit(args), s.label(), s.pid) // not user text
}

/// Run the tool on the host, on the session of the card. A screenshot
/// goes to `save` (the project inbox), which gives the path for the
/// sandbox. Ok holds the JSON for the sandbox; Err the failure with its
/// code.
pub fn perform(
    env: &crate::env::HostEnv,
    sandbox: &str,
    args: &BTreeMap<String, Value>,
    planned: &Planned,
    save: &mut dyn FnMut(&[u8]) -> Result<String, String>,
) -> Result<Value, Failure> {
    let mut d = Driver::reopen(env, &planned.chosen, &client(sandbox))?;
    let out = run_tool(&mut d, tool(args), &tool_args(args))?;
    let mut v = json!({"json": out.json});
    if let Some(png) = out.png {
        let path = save(&png).map_err(|e| Failure::new(Code::Internal, e))?;
        v["png"] = json!(path);
    }
    Ok(v)
}

/// The name prefix of the screenshots that the broker writes. Only paddock
/// writes it: a file of the user in the inbox (`mac-1.png`) never goes.
pub const SHOT_PREFIX: &str = "paddock-mac-";

/// The screenshot files of the broker in an inbox
/// (`paddock-mac-SECONDS.png`, `paddock-mac-SECONDS-N.png`) that go,
/// oldest first, so the newest `keep` stay. Other files are never named.
pub fn old_shots(names: &[String], keep: usize) -> Vec<String> {
    let mut shots: Vec<((u64, u32), &String)> = names
        .iter()
        .filter_map(|n| {
            let stem = n.strip_prefix(SHOT_PREFIX)?.strip_suffix(".png")?;
            let (secs, nth) = match stem.split_once('-') {
                Some((s, i)) => (s, i.parse::<u32>().ok()?),
                None => (stem, 0),
            };
            if secs.is_empty() || !secs.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some(((secs.parse::<u64>().ok()?, nth), n))
        })
        .collect();
    shots.sort();
    let drop = shots.len().saturating_sub(keep);
    shots[..drop].iter().map(|(_, n)| (*n).clone()).collect()
}

/// The response of the broker: the tool runs here, on the host, and a
/// screenshot goes into the inbox of `project`, where only the newest
/// [`SHOTS_KEPT`] screenshots stay. A failure keeps its code in the
/// result.
pub fn respond(
    env: &crate::env::HostEnv,
    project: &std::path::Path,
    sandbox: &str,
    args: &BTreeMap<String, Value>,
    planned: &Planned,
) -> crate::broker::Response {
    use crate::broker::{Response, Status};
    use std::io::Write;
    let mut save = |png: &[u8]| -> Result<String, String> {
        let inbox = crate::broker::inbox_for(env, project).map_err(|e| format!("{e:#}"))?;
        let name = format!("{SHOT_PREFIX}{}.png", crate::state::now()); // not user text
        let (mut f, name) = inbox.new_file(&name, 0o644).map_err(|e| format!("{e:#}"))?;
        f.write_all(png).map_err(|e| e.to_string())?;
        if let Ok(all) = inbox.file_names() {
            let all: Vec<String> = all
                .into_iter()
                .filter_map(|n| n.into_string().ok())
                .collect();
            for old in old_shots(&all, SHOTS_KEPT) {
                let _ = inbox.remove(std::ffi::OsStr::new(&old));
            }
        }
        Ok(inbox.path().join(name).display().to_string())
    };
    match perform(env, sandbox, args, planned, &mut save) {
        Ok(v) => Response {
            status: Status::Done,
            message: String::new(),
            result: v,
        },
        Err(f) => Response {
            status: Status::Failed,
            message: f.text.clone(),
            result: ch::failure_json(&f),
        },
    }
}

#[cfg(test)]
#[path = "computer_broker_tests.rs"]
mod tests;
