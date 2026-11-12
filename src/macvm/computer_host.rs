//! The commands of computer use on the host, for `paddock mac`, `paddock
//! mcp mac` and the broker: which session a command targets, the image
//! coordinates of the last screenshot, and each command as JSON.
//!
//! Coordinates are pixels of the last screenshot of the same target. The
//! host keeps its [`Geometry`] per session and scope (a small file under
//! the run folder of paddock), so a click at the pixel that the agent saw
//! lands on the guest point that the pixel showed. When the windows of
//! the target moved since, the pixels follow them; when the target
//! changed size, the command refuses with `stale` and asks for a new
//! screenshot. Before the first screenshot the coordinates are those of
//! a screenshot with the default size.
//!
//! Each client keeps its own geometry: the CLI, each `paddock mcp mac`
//! and each sandbox. A screenshot that the user takes at another size
//! never changes where the clicks of an agent land.

use super::computer::{
    Button, Code, Combo, Element, Failure, Geometry, Input, MaxSize, Mod, Scope, Target,
};
use super::computer_client::Conn;
use super::host::{self, Session};
use super::layout::Layout;
use super::proto::Point;
use super::text;
use crate::env::HostEnv;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The target as the user names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    /// The app of the session: `--app NAME`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Its instance: `--instance N`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<u32>,
    /// The whole guest display of the session, not only the app.
    #[serde(default)]
    pub desktop: bool,
}

/// The session and the target of a command.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub session: Session,
    pub target: Target,
}

impl Chosen {
    /// The name of the session for the output: `notes`, `notes 2`.
    pub fn name(&self) -> String {
        self.session.label()
    }
}

/// The session of a pick among the running ones. With `--app` the scope
/// is the app unless `--desktop`; without it the only session, and its
/// whole display.
pub fn choose(running: &[Session], pick: &Pick) -> Result<Chosen, Failure> {
    let scope = match (&pick.app, pick.desktop) {
        (Some(_), false) => Scope::App,
        _ => Scope::Desktop,
    };
    let session = match &pick.app {
        Some(app) => {
            let n = pick.instance.unwrap_or(super::instance::FIRST);
            running
                .iter()
                .find(|s| &s.app == app && s.instance() == n)
                .cloned()
                .ok_or_else(|| {
                    Failure::new(
                        Code::NoApp,
                        text::cu_session_not_running(
                            &super::instance::arg(app, n),
                            &names(running),
                        ),
                    )
                })?
        }
        None => match running {
            [] => return Err(Failure::new(Code::NoApp, text::cu_no_session())),
            [one] => {
                if pick.instance.is_some() {
                    return Err(Failure::new(
                        Code::BadRequest,
                        text::cu_instance_needs_app(),
                    ));
                }
                one.clone()
            }
            many => {
                return Err(Failure::new(
                    Code::BadRequest,
                    text::cu_sessions_many(&names(many)),
                ))
            }
        },
    };
    Ok(Chosen {
        session,
        target: Target { scope },
    })
}

/// The session of `chosen` among the running ones, when it is still the
/// same one: the same name, the same shim and the same start. A session
/// that stopped and started again is another session.
pub fn still_running(running: &[Session], chosen: &Chosen) -> Result<Chosen, Failure> {
    let was = &chosen.session;
    running
        .iter()
        .find(|s| s.key() == was.key() && s.pid == was.pid && s.started == was.started)
        .map(|s| Chosen {
            session: s.clone(),
            target: chosen.target,
        })
        .ok_or_else(|| Failure::new(Code::NoApp, text::cu_session_changed(&was.arg())))
}

fn names(running: &[Session]) -> Vec<String> {
    running.iter().map(|s| s.arg()).collect()
}

/// The running sessions, sorted by name.
pub fn running(layout: &Layout) -> Vec<Session> {
    let mut v: Vec<Session> = host::sessions(&layout.sessions)
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| host::alive(s.pid))
        .collect();
    v.sort_by_key(|s| s.key());
    v
}

/// The geometry of the last screenshot of one session and scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    /// The shim of the session: a new session starts without one.
    pub pid: u32,
    pub geometry: Geometry,
}

/// The client of the command line.
pub const CLI: &str = "cli";

/// The file of the stored geometry of one client.
pub fn store_path(dir: &Path, chosen: &Chosen, client: &str) -> PathBuf {
    let name = super::layout::safe_name(&chosen.session.key());
    let scope = chosen.target.scope.word();
    if client == CLI {
        return dir.join(format!("{name}-{scope}.json")); // not user text
    }
    let client = super::layout::safe_name(client);
    dir.join(format!("{name}-{scope}-{client}.json")) // not user text
}

/// The folder of the stored geometries.
pub fn store_dir(env: &HostEnv) -> PathBuf {
    env.paddock_run().join("macvm-computer")
}

pub fn load(path: &Path, pid: u32) -> Option<Geometry> {
    let text = std::fs::read_to_string(path).ok()?;
    let s: Stored = serde_json::from_str(&text).ok()?;
    (s.pid == pid).then_some(s.geometry)
}

pub fn save(path: &Path, pid: u32, geometry: Geometry) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension("json.partial");
    let Ok(text) = serde_json::to_vec(&Stored { pid, geometry }) else {
        return;
    };
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The geometry that a pixel of the agent maps through: the stored one
/// when the target still has its size (moved with the windows when they
/// moved), else a refusal; the default size of the region when nothing
/// is stored.
pub fn through(stored: Option<Geometry>, now: Geometry) -> Result<Geometry, Failure> {
    let Some(s) = stored else {
        return Ok(Geometry::plan(
            now.region(),
            scale_of(&now),
            Some(MaxSize::AGENT),
        ));
    };
    let same_size =
        (s.points.w - now.points.w).abs() <= 0.5 && (s.points.h - now.points.h).abs() <= 0.5;
    if !same_size {
        return Err(Failure::new(Code::Stale, text::cu_stale()));
    }
    Ok(Geometry {
        origin: now.origin,
        ..s
    })
}

/// Pixels per point of a native geometry.
fn scale_of(g: &Geometry) -> f64 {
    if g.points.w > 0.0 {
        f64::from(g.width) / g.points.w
    } else {
        1.0
    }
}

/// One action in pixels of the image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Act {
    Click {
        x: f64,
        y: f64,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u32,
        #[serde(default)]
        mods: Vec<Mod>,
    },
    Move {
        x: f64,
        y: f64,
    },
    Drag {
        x: f64,
        y: f64,
        to_x: f64,
        to_y: f64,
        #[serde(default)]
        button: Button,
        #[serde(default)]
        mods: Vec<Mod>,
    },
    Scroll {
        x: f64,
        y: f64,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    Type {
        text: String,
    },
    Key {
        combos: Vec<Combo>,
    },
}

fn one() -> u32 {
    1
}

impl Act {
    /// The pixels of the action.
    pub fn pixels(&self) -> Vec<(f64, f64)> {
        match self {
            Act::Click { x, y, .. } | Act::Move { x, y } | Act::Scroll { x, y, .. } => {
                vec![(*x, *y)]
            }
            Act::Drag {
                x, y, to_x, to_y, ..
            } => vec![(*x, *y), (*to_x, *to_y)],
            Act::Type { .. } | Act::Key { .. } => Vec::new(),
        }
    }

    pub fn needs_point(&self) -> bool {
        !self.pixels().is_empty()
    }

    /// The action in guest points through `g`. A pixel outside the image
    /// is refused.
    pub fn to_input(&self, g: &Geometry) -> Result<Input, Failure> {
        for (x, y) in self.pixels() {
            if !(x.is_finite() && y.is_finite()) || !g.holds_pixel(x, y) {
                return Err(Failure::new(
                    Code::Outside,
                    text::cu_outside_image(x, y, g.width, g.height),
                ));
            }
        }
        let p = |x: f64, y: f64| -> Point { g.to_guest(x, y) };
        Ok(match self.clone() {
            Act::Click {
                x,
                y,
                button,
                count,
                mods,
            } => Input::Click {
                at: p(x, y),
                button,
                count,
                mods,
            },
            Act::Move { x, y } => Input::Move { at: p(x, y) },
            Act::Drag {
                x,
                y,
                to_x,
                to_y,
                button,
                mods,
            } => Input::Drag {
                from: p(x, y),
                to: p(to_x, to_y),
                button,
                mods,
            },
            Act::Scroll { x, y, dx, dy } => Input::Scroll {
                at: p(x, y),
                dx,
                dy,
            },
            Act::Type { text } => Input::Type { text },
            Act::Key { combos } => Input::Key { combos },
        })
    }
}

/// The roles that an agent acts on, kept in a snapshot without a label.
const ACTIONABLE: &[&str] = &[
    "AXButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXTextField",
    "AXTextArea",
    "AXSearchField",
    "AXSecureTextField",
    "AXComboBox",
    "AXPopUpButton",
    "AXMenuButton",
    "AXMenuItem",
    "AXLink",
    "AXSlider",
    "AXTabGroup",
    "AXDisclosureTriangle",
    "AXIncrementor",
    "AXColorWell",
    "AXSwitch",
    "AXToggle",
];

/// True for an element that an agent can read or act on: a label, a
/// value, or an actionable role, and an area on the screen.
pub fn useful(e: &Element) -> bool {
    e.frame.w > 0.0
        && e.frame.h > 0.0
        && (!e.label.is_empty()
            || e.value.as_deref().is_some_and(|v| !v.is_empty())
            || ACTIONABLE.contains(&e.role.as_str())
            || e.depth == 0)
}

/// One element for the output: its frame and its center in pixels of the
/// image.
pub fn element_json(e: &Element, g: &Geometry) -> Value {
    let f = g.frame_to_image(e.frame);
    let r = |v: f64| (v * 10.0).round() / 10.0;
    let mut v = json!({
        "id": e.id,
        "role": e.role,
        "frame": {"x": r(f.x), "y": r(f.y), "w": r(f.w), "h": r(f.h)},
        "center": {"x": r(f.x + f.w / 2.0), "y": r(f.y + f.h / 2.0)},
        "depth": e.depth,
    });
    let o = v.as_object_mut().expect("an object");
    if !e.subrole.is_empty() {
        o.insert("subrole".into(), json!(e.subrole));
    }
    if !e.label.is_empty() {
        o.insert("label".into(), json!(e.label));
    }
    if let Some(val) = &e.value {
        o.insert("value".into(), json!(val));
    }
    if !e.enabled {
        o.insert("enabled".into(), json!(false));
    }
    if e.focused {
        o.insert("focused".into(), json!(true));
    }
    // Off the image: the agent scrolls or takes the desktop scope.
    let (cx, cy) = (f.x + f.w / 2.0, f.y + f.h / 2.0);
    if !g.holds_pixel(cx, cy) {
        o.insert("visible".into(), json!(false));
    }
    v
}

fn point_json(p: Point) -> Value {
    let r = |v: f64| (v * 100.0).round() / 100.0;
    json!({"x": r(p.x), "y": r(p.y)})
}

/// The JSON of a command for a terminal: serde escapes the control
/// characters below 0x20, and this also the others (DEL and the C1
/// controls, which a terminal can run as escapes), so no text of the
/// guest (a window title, a label) reaches the terminal raw.
pub fn json_text(v: &Value, pretty: bool) -> String {
    let text = if pretty {
        serde_json::to_string_pretty(v)
    } else {
        serde_json::to_string(v)
    }
    .unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && c != '\n' {
            out.push_str(&format!("\\u{:04x}", c as u32)); // not user text
        } else {
            out.push(c);
        }
    }
    out
}

/// The reason of a failure for a terminal: without the control
/// characters (escapes) that a guest could put in it.
pub fn failure_line(f: &Failure) -> String {
    crate::view::strip_controls(&f.text)
}

/// The JSON of a failure.
pub fn failure_json(f: &Failure) -> Value {
    json!({"ok": false, "code": f.code.word(), "error": f.text})
}

/// A connection to the target of one command, with the stored geometry.
pub struct Driver {
    pub chosen: Chosen,
    pub conn: Conn,
    store: PathBuf,
    dir: PathBuf,
}

impl Driver {
    /// Choose the session and connect to its guest, for the command line.
    pub fn open(env: &HostEnv, pick: &Pick) -> Result<Driver, Failure> {
        Driver::open_as(env, pick, CLI)
    }

    /// Choose the session and connect to its guest, for `client`.
    pub fn open_as(env: &HostEnv, pick: &Pick, client: &str) -> Result<Driver, Failure> {
        if let Some(app) = &pick.app {
            if !host::valid_name(app) {
                return Err(Failure::new(Code::BadRequest, text::bad_app_name(app)));
            }
        }
        let chosen = choose(&running(&Layout::new(env)), pick)?;
        Driver::connect(env, chosen, client)
    }

    /// Connect to the session that was chosen before (the broker chose
    /// it before it asked the user), only when it is still the same one.
    pub fn reopen(env: &HostEnv, chosen: &Chosen, client: &str) -> Result<Driver, Failure> {
        let now = still_running(&running(&Layout::new(env)), chosen)?;
        Driver::connect(env, now, client)
    }

    fn connect(env: &HostEnv, chosen: Chosen, client: &str) -> Result<Driver, Failure> {
        if chosen.session.suspended {
            return Err(Failure::new(
                Code::Unreachable,
                text::cu_suspended(&chosen.session.arg()),
            ));
        }
        let conn = Conn::open(&chosen.session.control)?;
        Ok(Driver::with(chosen, conn, &store_dir(env)).client(client))
    }

    /// A driver on a connection, the geometries under `dir`, for the
    /// command line.
    pub fn with(chosen: Chosen, conn: Conn, dir: &Path) -> Driver {
        let store = store_path(dir, &chosen, CLI);
        Driver {
            chosen,
            conn,
            store,
            dir: dir.to_path_buf(),
        }
    }

    /// The same driver for another client, with its own geometry.
    pub fn client(mut self, client: &str) -> Driver {
        self.store = store_path(&self.dir, &self.chosen, client);
        self
    }

    fn head(&self) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        m.insert("ok".into(), json!(true));
        m.insert("session".into(), json!(self.chosen.name()));
        m.insert("scope".into(), json!(self.chosen.target.scope.word()));
        m
    }

    fn stored(&self) -> Option<Geometry> {
        load(&self.store, self.chosen.session.pid)
    }

    /// The display, the permissions, the app and its windows.
    pub fn status(&mut self) -> Result<Value, Failure> {
        let st = self.conn.status(self.chosen.target)?;
        let mut m = self.head();
        let v =
            serde_json::to_value(&st).map_err(|e| Failure::new(Code::Internal, e.to_string()))?;
        if let Value::Object(o) = v {
            m.extend(o);
        }
        if let Some(g) = self.stored() {
            m.insert("last_screenshot".into(), json!(g));
        }
        let mut hints = Vec::new();
        if !st.screen_recording {
            hints.push(text::cu_hint_screen_recording());
        }
        if !st.accessibility {
            hints.push(text::cu_hint_accessibility());
        }
        if st.locked {
            hints.push(text::cu_hint_locked());
        }
        if self.chosen.target.scope == Scope::App && !st.app_running {
            hints.push(text::cu_no_app());
        }
        if !hints.is_empty() {
            m.insert("hints".into(), json!(hints));
        }
        Ok(Value::Object(m))
    }

    /// One PNG of the target; its geometry becomes the one that the next
    /// coordinates map through.
    pub fn screenshot(
        &mut self,
        max: Option<MaxSize>,
        cursor: bool,
    ) -> Result<(Value, Vec<u8>), Failure> {
        let (g, png) = self.conn.screenshot(self.chosen.target, max, cursor)?;
        save(&self.store, self.chosen.session.pid, g);
        let mut m = self.head();
        m.insert("width".into(), json!(g.width));
        m.insert("height".into(), json!(g.height));
        m.insert("geometry".into(), json!(g));
        m.insert("bytes".into(), json!(png.len()));
        Ok((Value::Object(m), png))
    }

    /// The geometry of the target now, at its native size.
    fn now(&mut self) -> Result<Geometry, Failure> {
        let st = self.conn.status(self.chosen.target)?;
        if self.chosen.target.scope == Scope::App && !st.app_running {
            return Err(Failure::new(Code::NoApp, text::cu_no_app()));
        }
        st.geometry
            .ok_or_else(|| Failure::new(Code::NoWindow, text::cu_no_window()))
    }

    /// One action in pixels of the last screenshot.
    pub fn act(&mut self, act: &Act) -> Result<Value, Failure> {
        let mut m = self.head();
        let input = if act.needs_point() {
            let g = through(self.stored(), self.now()?)?;
            let input = act.to_input(&g)?;
            let pts: Vec<Value> = input.points().into_iter().map(point_json).collect();
            m.insert("points".into(), json!(pts));
            input
        } else {
            act.to_input(&Geometry::default())?
        };
        m.insert("action".into(), json!(input.word()));
        let done = self.conn.input(self.chosen.target, input)?;
        m.insert("events".into(), json!(done.events));
        if done.activated {
            m.insert("activated".into(), json!(true));
        }
        Ok(Value::Object(m))
    }

    /// The accessibility snapshot, frames in pixels of the last
    /// screenshot.
    pub fn ax(
        &mut self,
        all: bool,
        depth: Option<u32>,
        max_nodes: Option<u32>,
    ) -> Result<Value, Failure> {
        let snap = self.conn.ax(self.chosen.target, depth, max_nodes)?;
        let g = through(self.stored(), snap.geometry)?;
        let list: Vec<Value> = snap
            .elements
            .iter()
            .filter(|e| all || useful(e))
            .map(|e| element_json(e, &g))
            .collect();
        let mut m = self.head();
        m.insert("count".into(), json!(list.len()));
        m.insert("read".into(), json!(snap.elements.len()));
        if snap.cut {
            m.insert("cut".into(), json!(true));
        }
        m.insert(
            "image".into(),
            json!({"width": g.width, "height": g.height}),
        );
        m.insert("elements".into(), Value::Array(list));
        Ok(Value::Object(m))
    }

    /// `AXPress` on an element of the snapshot.
    pub fn press(&mut self, element: &str, role: Option<&str>) -> Result<Value, Failure> {
        let (role, label) = self.conn.press(self.chosen.target, element, role)?;
        let mut m = self.head();
        m.insert("action".into(), json!("press"));
        m.insert("element".into(), json!(element));
        m.insert("role".into(), json!(role));
        if !label.is_empty() {
            m.insert("label".into(), json!(label));
        }
        Ok(Value::Object(m))
    }
}

/// The scroll lines of a direction word and an amount: `down 3`.
pub fn scroll_lines(direction: &str, amount: i32) -> Result<(i32, i32), Failure> {
    let n = amount.max(1);
    Ok(match direction.to_ascii_lowercase().as_str() {
        "down" => (0, n),
        "up" => (0, -n),
        "right" => (n, 0),
        "left" => (-n, 0),
        other => {
            return Err(Failure::new(
                Code::BadRequest,
                text::cu_scroll_direction(other),
            ))
        }
    })
}

/// A size of `--max-size`: `1280x800`, `1568` for a square, `full` for
/// the native pixels, `agent` for [`MaxSize::AGENT`].
pub fn parse_max(text: &str) -> Result<Option<MaxSize>, Failure> {
    let t = text.trim().to_ascii_lowercase();
    let bad = || Failure::new(Code::BadRequest, text::cu_max_size_invalid(text));
    match t.as_str() {
        "full" | "native" => return Ok(None),
        "agent" | "" => return Ok(Some(MaxSize::AGENT)),
        _ => {}
    }
    let (w, h) = match t.split_once('x') {
        Some((w, h)) => (w.parse::<u32>(), h.parse::<u32>()),
        None => (t.parse::<u32>(), t.parse::<u32>()),
    };
    match (w, h) {
        (Ok(w), Ok(h)) if (16..=16384).contains(&w) && (16..=16384).contains(&h) => {
            Ok(Some(MaxSize::of(w, h)))
        }
        _ => Err(bad()),
    }
}

#[cfg(test)]
#[path = "computer_host_tests.rs"]
mod tests;
