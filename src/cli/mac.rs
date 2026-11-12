//! `paddock mac`: computer use of a macOS guest session (see
//! `paddock::macvm::computer_host`), and `paddock mcp mac`, the same tools
//! as an MCP server. Every command prints one JSON object. A failure
//! prints `{"ok": false, "code", "error"}`, the reason on stderr, and
//! exits 1. `paddock mac view` draws the guest inline in the terminal
//! (src/macvm/view) and prints no JSON.

use anyhow::Result;
use clap::{Args, Subcommand};
use paddock::env::HostEnv;
use paddock::macvm::computer::{Button, Failure, Mod};
use paddock::macvm::computer_host::{self as ch, Act, Driver, Pick};
use paddock::macvm::computer_input::{parse_combo, parse_mod};
use paddock::macvm::computer_mcp::{self, HostBackend};
use serde_json::Value;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

mod view;
pub(crate) use view::ViewArgs;

pub(crate) const MAC_HELP: &str = "\
Targets:
  --app NAME [--instance N]   the windows of that app session only
  --app NAME --desktop        the whole guest display of that session
  (none)                      the whole display of the only running session

Coordinates are pixels of the last screenshot of the same target.
Loop: screenshot, act, screenshot again to verify. `paddock help computer-use`.

Examples:
  paddock mac status --app notes
  paddock mac screenshot --app notes --out notes.png
  paddock mac click 412 230 --app notes
  paddock mac type 'Hello' --app notes
  paddock mac key cmd+s --app notes
  paddock mac ax --app notes
  paddock mac view --app notes";

/// The target of a command, on every subcommand.
#[derive(Args, Debug, Clone, Default)]
pub(crate) struct PickArgs {
    /// The app session: only its windows, unless --desktop.
    #[arg(long, global = true, value_name = "NAME")]
    pub app: Option<String>,
    /// The instance of the app session.
    #[arg(long, global = true, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=99))]
    pub instance: Option<u32>,
    /// The whole guest display of the session, every window on it.
    #[arg(long, global = true)]
    pub desktop: bool,
}

impl PickArgs {
    pub(crate) fn pick(&self) -> Pick {
        Pick {
            app: self.app.clone(),
            instance: self.instance,
            desktop: self.desktop,
        }
    }
}

#[derive(Args, Debug, Clone)]
pub(crate) struct MacArgs {
    #[command(subcommand)]
    pub cmd: MacCmd,
    #[command(flatten)]
    pub pick: PickArgs,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum MacCmd {
    /// Show the guest desktop, or the windows of one app, inline in this
    /// terminal (kitty graphics, iTerm2 images or sixel). Keys and the
    /// mouse go to the guest. Ctrl-] opens a menu of Cmd shortcuts and
    /// the way out; leaving the view keeps the session running.
    View(ViewArgs),
    /// The display, the permissions, the lock, the app and its windows.
    Status,
    /// Capture the target as a PNG.
    Screenshot {
        /// The file to write (default: a file under the paddock run folder).
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// agent (fits a model's image limit, the default), full (the
        /// native pixels), or WIDTHxHEIGHT.
        #[arg(long, value_name = "SIZE", default_value = "agent")]
        max_size: String,
        /// Draw the pointer into the image.
        #[arg(long)]
        cursor: bool,
    },
    /// Click at a pixel of the last screenshot.
    Click {
        x: f64,
        y: f64,
        /// left, right or middle.
        #[arg(long, default_value = "left")]
        button: String,
        /// The number of clicks: 2 is a double click.
        #[arg(long, default_value_t = 1, conflicts_with = "double")]
        count: u32,
        #[arg(long)]
        double: bool,
        /// Modifier keys held during the click: cmd,shift,option,ctrl.
        #[arg(long, value_delimiter = ',', value_name = "KEYS")]
        mods: Vec<String>,
    },
    /// Move the pointer to a pixel of the last screenshot.
    Move { x: f64, y: f64 },
    /// Press at X Y, move to TO_X TO_Y, let go.
    Drag {
        x: f64,
        y: f64,
        to_x: f64,
        to_y: f64,
        #[arg(long, default_value = "left")]
        button: String,
        #[arg(long, value_delimiter = ',', value_name = "KEYS")]
        mods: Vec<String>,
    },
    /// Scroll at a pixel: up, down, left or right, by lines (3).
    Scroll {
        x: f64,
        y: f64,
        direction: String,
        #[arg(default_value_t = 3)]
        amount: i32,
    },
    /// Type text as it is, on any keyboard layout of the guest.
    Type {
        /// The text; with --stdin the text comes from stdin.
        #[arg(required_unless_present = "stdin")]
        text: Option<String>,
        #[arg(long)]
        stdin: bool,
    },
    /// Press keys one after the other: cmd+s, shift+tab, return.
    Key {
        #[arg(required = true, value_name = "COMBO")]
        combos: Vec<String>,
    },
    /// The accessibility elements of the target, in pixels of the last
    /// screenshot.
    Ax {
        /// Every element, also those without a label, a value or an action.
        #[arg(long)]
        all: bool,
        #[arg(long, value_name = "N")]
        depth: Option<u32>,
        #[arg(long, value_name = "N")]
        max_nodes: Option<u32>,
    },
    /// Press an element by its id from `paddock mac ax`.
    Press {
        element: String,
        /// Press only when the element still has this role.
        #[arg(long)]
        role: Option<String>,
    },
}

/// Print the JSON: pretty on a terminal, one line otherwise.
fn print(v: &Value) {
    let text = ch::json_text(v, std::io::stdout().is_terminal());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

fn fail(f: &Failure) -> i32 {
    print(&ch::failure_json(f));
    eprintln!("paddock mac: {}", ch::failure_line(f));
    1
}

fn button(s: &str) -> Result<Button, Failure> {
    serde_json::from_value(Value::String(s.to_ascii_lowercase())).map_err(|_| {
        Failure::new(
            paddock::macvm::computer::Code::BadRequest,
            paddock::macvm::text::cu_button_invalid(s),
        )
    })
}

fn mods(list: &[String]) -> Result<Vec<Mod>, Failure> {
    let mut out = Vec::new();
    for m in list {
        let p = parse_mod(m.trim()).ok_or_else(|| {
            Failure::new(
                paddock::macvm::computer::Code::BadRequest,
                paddock::macvm::text::cu_mod_unknown(m, m),
            )
        })?;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn bad(text: String) -> Failure {
    Failure::new(paddock::macvm::computer::Code::BadRequest, text)
}

/// The action of a command, None for the commands that are not actions.
fn act_of(cmd: &MacCmd) -> Result<Option<Act>, Failure> {
    Ok(Some(match cmd {
        MacCmd::Click {
            x,
            y,
            button: b,
            count,
            double,
            mods: m,
        } => Act::Click {
            x: *x,
            y: *y,
            button: button(b)?,
            count: if *double { 2 } else { *count },
            mods: mods(m)?,
        },
        MacCmd::Move { x, y } => Act::Move { x: *x, y: *y },
        MacCmd::Drag {
            x,
            y,
            to_x,
            to_y,
            button: b,
            mods: m,
        } => Act::Drag {
            x: *x,
            y: *y,
            to_x: *to_x,
            to_y: *to_y,
            button: button(b)?,
            mods: mods(m)?,
        },
        MacCmd::Scroll {
            x,
            y,
            direction,
            amount,
        } => {
            let (dx, dy) = ch::scroll_lines(direction, *amount)?;
            Act::Scroll {
                x: *x,
                y: *y,
                dx,
                dy,
            }
        }
        MacCmd::Type { text, stdin } => {
            let text = if *stdin {
                let mut s = String::new();
                std::io::stdin()
                    .read_to_string(&mut s)
                    .map_err(|e| bad(e.to_string()))?;
                s
            } else {
                text.clone().unwrap_or_default()
            };
            Act::Type { text }
        }
        MacCmd::Key { combos } => {
            let mut out = Vec::new();
            for c in combos {
                out.push(parse_combo(c).map_err(bad)?);
            }
            Act::Key { combos: out }
        }
        _ => return Ok(None),
    }))
}

/// The file of a screenshot without --out: under the run folder, the
/// last 20 kept.
fn shot_path(env: &HostEnv, d: &Driver) -> PathBuf {
    let dir = ch::store_dir(env).join("shots");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        let n = files.len();
        for f in files.into_iter().take(n.saturating_sub(19)) {
            let _ = std::fs::remove_file(f);
        }
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let key = paddock::macvm::layout::safe_name(&d.chosen.session.key());
    dir.join(format!("{ms}-{key}-{}.png", d.chosen.target.scope.word()))
}

fn exec(env: &HostEnv, a: &MacArgs) -> Result<Value, Failure> {
    // The values first: a bad key never opens a connection.
    let act = act_of(&a.cmd)?;
    if let MacCmd::Screenshot { max_size, .. } = &a.cmd {
        ch::parse_max(max_size)?;
    }
    let mut d = Driver::open(env, &a.pick.pick())?;
    if let Some(act) = act {
        return d.act(&act);
    }
    match &a.cmd {
        MacCmd::Status => d.status(),
        MacCmd::Screenshot {
            out,
            max_size,
            cursor,
        } => {
            let (mut v, png) = d.screenshot(ch::parse_max(max_size)?, *cursor)?;
            let path = match out {
                Some(p) => p.clone(),
                None => shot_path(env, &d),
            };
            std::fs::write(&path, &png).map_err(|e| {
                Failure::new(
                    paddock::macvm::computer::Code::Internal,
                    format!("{}: {e}", paddock::macvm::text::cannot_write(&path)),
                )
            })?;
            let abs = std::fs::canonicalize(&path).unwrap_or(path);
            if let Value::Object(o) = &mut v {
                o.insert("path".into(), Value::String(abs.display().to_string()));
            }
            Ok(v)
        }
        MacCmd::Ax {
            all,
            depth,
            max_nodes,
        } => d.ax(*all, *depth, *max_nodes),
        MacCmd::Press { element, role } => d.press(element, role.as_deref()),
        MacCmd::View(_) => unreachable!("run draws the view"),
        _ => unreachable!("an action"),
    }
}

pub(crate) fn run(env: &HostEnv, a: MacArgs) -> Result<i32> {
    if let MacCmd::View(v) = &a.cmd {
        return view::view(&a.pick, v);
    }
    match exec(env, &a) {
        Ok(v) => {
            print(&v);
            Ok(0)
        }
        Err(f) => Ok(fail(&f)),
    }
}

/// `paddock mcp mac`: the MCP server on stdio.
pub(crate) fn mcp(env: &HostEnv, pick: &PickArgs) -> Result<i32> {
    let mut backend = HostBackend::new(env.clone(), pick.pick());
    let stdin = std::io::stdin();
    computer_mcp::serve(&mut backend, stdin.lock(), std::io::stdout())?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(flatten)]
        a: MacArgs,
    }

    fn parse(args: &[&str]) -> MacArgs {
        let mut v = vec!["t"];
        v.extend_from_slice(args);
        T::try_parse_from(v).unwrap().a
    }

    #[test]
    fn the_target_flags_go_before_or_after_the_command() {
        let a = parse(&["click", "1", "2", "--app", "notes", "--instance", "2"]);
        assert_eq!(a.pick.pick().app.as_deref(), Some("notes"));
        assert_eq!(a.pick.instance, Some(2));
        let a = parse(&["--app", "notes", "--desktop", "status"]);
        assert!(a.pick.desktop);
        assert!(T::try_parse_from(["t", "status", "--instance", "0"]).is_err());
    }

    #[test]
    fn actions_from_the_command_line() {
        let a = parse(&[
            "click",
            "10",
            "20",
            "--double",
            "--mods",
            "cmd,shift",
            "--button",
            "Right",
        ]);
        assert_eq!(
            act_of(&a.cmd).unwrap(),
            Some(Act::Click {
                x: 10.0,
                y: 20.0,
                button: Button::Right,
                count: 2,
                mods: vec![Mod::Cmd, Mod::Shift]
            })
        );
        let a = parse(&["scroll", "1", "2", "down"]);
        assert!(matches!(
            act_of(&a.cmd).unwrap(),
            Some(Act::Scroll { dy: 3, dx: 0, .. })
        ));
        let a = parse(&["key", "cmd+s", "return"]);
        match act_of(&a.cmd).unwrap() {
            Some(Act::Key { combos }) => assert_eq!(combos.len(), 2),
            other => panic!("{other:?}"),
        }
        let a = parse(&["key", "cmd+nokey"]);
        assert!(act_of(&a.cmd).is_err());
        let a = parse(&["click", "1", "2", "--mods", "hyper"]);
        assert!(act_of(&a.cmd).is_err());
        assert_eq!(act_of(&parse(&["status"]).cmd).unwrap(), None);
        assert!(T::try_parse_from(["t", "type"]).is_err());
    }
}
