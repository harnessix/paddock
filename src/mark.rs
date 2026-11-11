//! The mark of protected windows on macOS (`paddock mark`).
//!
//! Each window of an app sandbox gets a thin frame in a trust color, a
//! badge on demand, and an entry in a menu bar item. The frame is a separate
//! click-through window above the app window (mark/macos.rs). This file
//! holds the parts that need no screen: colors, badge text, window
//! matching and geometry. They are tested on every platform.

use crate::env::HostEnv;
use crate::state::Running;
use std::collections::{BTreeMap, BTreeSet};
pub use vm::VmFacts;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub mod procs;

/// The width of the frame stroke, in points.
pub const FRAME_WIDTH: f64 = 2.0;
/// The corner radius of a standard window, in points.
pub const WINDOW_RADIUS: f64 = 12.0;
/// Frame opacity of a window that does not have focus: 0x88, as on Omarchy.
pub const DIM_ALPHA: f64 = 136.0 / 255.0;
/// The badge shows this long after its window gets focus, in seconds.
pub const BADGE_FOCUS_SECS: f64 = 4.0;
/// The badge shows while the pointer is this close to it, in points.
pub const BADGE_HOVER_MARGIN: f64 = 18.0;
/// Space between the badge and the window corner, in points.
pub const BADGE_INSET: f64 = 12.0;
/// Windows smaller than this are menus, tooltips or helpers: no mark.
pub const MIN_WINDOW: f64 = 60.0;

/// What the color of a mark says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Traffic goes only through paddock's proxy, or there is no network.
    Sage,
    /// A wider network.
    Amber,
}

impl Tone {
    /// The tone of a network mode, as `Running::network` names it. A mode
    /// that paddock does not know gets amber.
    pub fn of(network: &str) -> Tone {
        match network {
            // On macOS, mode allowlist goes through paddock's proxy too, and
            // mode ask exists only in the macOS guest, whose every
            // connection goes through paddock's proxy.
            "none" | "api" | "egress" | "allowlist" | "ask" => Tone::Sage,
            _ => Tone::Amber,
        }
    }

    /// sRGB components from 0 to 1.
    pub fn rgb(self) -> (f64, f64, f64) {
        match self {
            // #9FB8A0, the Omarchy base color.
            Tone::Sage => (159.0 / 255.0, 184.0 / 255.0, 160.0 / 255.0),
            // #E0A33A, the Omarchy base color.
            Tone::Amber => (224.0 / 255.0, 163.0 / 255.0, 58.0 / 255.0),
        }
    }

    /// The word of the tone, for `paddock windows --json`.
    pub fn word(self) -> &'static str {
        match self {
            Tone::Sage => "sage",
            Tone::Amber => "amber",
        }
    }

    /// What the tone means, for the details.
    pub fn meaning(self) -> &'static str {
        match self {
            Tone::Sage => "only through paddock's proxy",
            Tone::Amber => "a wider network",
        }
    }
}

/// A short age: `<1m`, `42m`, `2h`, `3d`.
pub fn age(secs: u64) -> String {
    match secs {
        0..=59 => "<1m".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// The badge text, the same as on Omarchy: `paddock · vscode · egress · 2h`.
/// `name` is the pasture, or else the profile.
pub fn badge_text(name: &str, network: &str, age_secs: u64) -> String {
    format!("paddock · {name} · {network} · {}", age(age_secs))
}

/// The badge text of one run. A session of the macOS guest (`backend =
/// "macvm"`) gains the word `vm`, and the name of the guest input source
/// while it differs from the host's: `paddock · chatgpt-desktop · ask ·
/// vm · Pinyin - Simplified · 2h`. The note of the session (`waking`,
/// `display changed, restart for full size`) goes at the end.
pub fn run_badge(r: &Running, vm: Option<&VmFacts>, now: u64) -> String {
    let age_secs = now.saturating_sub(r.started);
    if !is_vm(r) {
        return badge_text(badge_name(r), &r.network, age_secs);
    }
    let mut parts = vec![
        "paddock".to_string(),
        badge_name(r).to_string(),
        r.network.clone(),
        "vm".to_string(),
    ];
    let facts = vm.cloned().unwrap_or_default();
    if let Some(input) = facts.guest_input.as_deref().filter(|s| !s.is_empty()) {
        parts.push(input.to_string());
    }
    parts.push(age(age_secs));
    if let Some(note) = facts.note.as_deref().filter(|s| !s.is_empty()) {
        parts.push(note.to_string());
    }
    parts.join(" · ")
}

/// A run of the macOS guest.
pub fn is_vm(r: &Running) -> bool {
    r.backend == crate::macvm::host::BACKEND
}

/// The name in the badge: the pasture the sandbox joined, or its profile.
pub fn badge_name(r: &Running) -> &str {
    r.pasture.as_deref().unwrap_or(&r.harness)
}

/// The last part of the project path.
pub fn project_name(r: &Running) -> String {
    r.project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| r.project.display().to_string())
}

/// The lines of the details that a click on the badge shows. `terminal` is
/// the name of the terminal app of a terminal run.
pub fn details(r: &Running, now: u64, terminal: Option<&str>) -> Vec<String> {
    let since = now.saturating_sub(r.started);
    let mut out = vec![
        badge_text(badge_name(r), &r.network, since),
        format!("Project: {}", r.project.display()),
        format!("Network: {}, {}", r.network, Tone::of(&r.network).meaning()),
        format!("Backend: {}", r.backend),
    ];
    if let Some(app) = terminal {
        out.push(format!("Terminal run in {app}"));
    }
    if !r.auth.is_empty() {
        out.push(format!("Login: {}", r.auth));
    }
    out.push(format!("Started: {} ago", age(since)));
    out.push(format!("Sandbox: {}", r.id));
    out.push(format!("Process: {}", r.pid));
    out
}

/// The menu bar entry of one window. `nth` counts the windows of one
/// sandbox from 1, and `of` is their number. `terminal` is the name of the
/// terminal app of a terminal run.
pub fn menu_title(r: &Running, now: u64, nth: usize, of: usize, terminal: Option<&str>) -> String {
    let mut s = format!(
        "{} · {} · {} · {}",
        r.harness,
        project_name(r),
        r.network,
        age(now.saturating_sub(r.started))
    );
    if of > 1 {
        s.push_str(&format!(" ({nth}/{of})"));
    }
    if let Some(app) = terminal {
        s.push_str(&format!(" ({app})"));
    }
    s
}

/// The menu entry of one window of a run. A session of the macOS guest is
/// named by its app (`ChatGPT (paddock)`) and gains `vm`, the other runs
/// keep [`menu_title`].
pub fn run_menu_title(
    r: &Running,
    vm: Option<&VmFacts>,
    now: u64,
    nth: usize,
    of: usize,
    terminal: Option<&str>,
) -> String {
    if !is_vm(r) {
        return menu_title(r, now, nth, of, terminal);
    }
    let name = vm
        .map(|f| f.display_name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(&r.harness);
    let mut s = format!(
        "{name} · {} · {} · vm · {}",
        project_name(r),
        r.network,
        age(now.saturating_sub(r.started))
    );
    if of > 1 {
        s.push_str(&format!(" ({nth}/{of})"));
    }
    s
}

/// The running sandboxes that get a mark: app runs, and terminal runs
/// whose terminal window paddock recorded at the start.
pub fn marked_runs(list: Vec<Running>) -> Vec<Running> {
    list.into_iter()
        .filter(|r| r.gui == "app" || (r.gui == "off" && r.terminal_window.is_some()))
        .collect()
}

/// Whether the windows of a run get a frame. On macOS no window gets
/// one: a frame window that follows its window by polling lags behind
/// every move. A run keeps its badge, its menu entry and its questions.
pub fn framed(r: &Running) -> bool {
    !cfg!(target_os = "macos") && !is_vm(r)
}

/// What tells the windows of one marked run.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// An app run: every window of a process in this process tree.
    Tree(BTreeSet<u32>),
    /// A terminal run: one window, by its window number and owner pid.
    Window(u32, u32),
}

impl Target {
    /// The target of a run. `procs` gives the process tree of an app run.
    pub fn of(r: &Running, procs: &ProcTable) -> Target {
        match (r.gui.as_str(), r.terminal_window, r.terminal_pid) {
            ("off", Some(id), Some(pid)) => Target::Window(id, pid),
            _ => Target::Tree(tree(procs, r.pid)),
        }
    }

    fn matches(&self, w: &Win) -> bool {
        match self {
            Target::Tree(t) => t.contains(&w.pid),
            Target::Window(id, pid) => w.id == *id && w.pid == *pid,
        }
    }
}

/// A rectangle in global screen coordinates with the origin at the top
/// left of the main screen and y down, as the window server reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Larger by `d` on each side.
    pub fn outset(self, d: f64) -> Self {
        Self::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    pub fn contains(self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    /// The same rectangle in AppKit coordinates: origin at the bottom left
    /// of the main screen, y up. `main_h` is the height of the main screen.
    pub fn flip(self, main_h: f64) -> Self {
        Self::new(self.x, main_h - self.y - self.h, self.w, self.h)
    }

    /// Close enough to count as the same window frame.
    pub fn near(self, o: Rect, tolerance: f64) -> bool {
        (self.x - o.x).abs() <= tolerance
            && (self.y - o.y).abs() <= tolerance
            && (self.w - o.w).abs() <= tolerance
            && (self.h - o.h).abs() <= tolerance
    }
}

/// The frame window of an app window: around it, outside its edge.
pub fn frame_rect(win: Rect) -> Rect {
    win.outset(FRAME_WIDTH)
}

/// Where the badge goes. Above the top right corner of the window, outside
/// it, when the screen has room there. Else just inside the bottom right
/// corner. `screens` are the visible areas of the screens (without the menu
/// bar and the Dock).
pub fn badge_rect(win: Rect, w: f64, h: f64, screens: &[Rect]) -> Rect {
    let x = win.x + win.w - w - BADGE_INSET;
    let above = Rect::new(x, win.y - FRAME_WIDTH - 4.0 - h, w, h);
    let top = screens
        .iter()
        .find(|s| s.contains(win.x + win.w / 2.0, win.y.max(s.y)))
        .map(|s| s.y)
        .unwrap_or(0.0);
    if above.y >= top {
        above
    } else {
        Rect::new(x, win.y + win.h - h - BADGE_INSET, w, h)
    }
}

/// Whether the badge shows: for a while after its window gets focus, and
/// while the pointer is near. `focused_for` is the time since the window got
/// focus, None without focus.
pub fn badge_visible(focused_for: Option<f64>, hover: bool) -> bool {
    hover || focused_for.is_some_and(|t| t < BADGE_FOCUS_SECS)
}

/// One window from the window server list.
#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    /// The window number of the window server.
    pub id: u32,
    pub pid: u32,
    /// 0 for normal windows. Menus, panels and the Dock use other layers.
    pub layer: i64,
    pub bounds: Rect,
    pub alpha: f64,
}

/// Each process and its parent.
pub type ProcTable = BTreeMap<u32, u32>;

/// A process and all below it.
pub fn tree(procs: &ProcTable, root: u32) -> BTreeSet<u32> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&pid, &ppid) in procs {
        if pid != ppid {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut out = BTreeSet::from([root]);
    let mut todo = vec![root];
    while let Some(p) = todo.pop() {
        for &c in children.get(&p).into_iter().flatten() {
            if out.insert(c) {
                todo.push(c);
            }
        }
    }
    out
}

/// A normal, visible window of a useful size: one that can get a mark.
fn markable(w: &Win) -> bool {
    w.layer == 0 && w.alpha > 0.0 && w.bounds.w >= MIN_WINDOW && w.bounds.h >= MIN_WINDOW
}

/// The windows that get a mark, in the order of the list (front first),
/// each with the index of its run in `targets`. Only normal, visible
/// windows of a useful size count. `own` is the marker process: it never
/// marks its own windows.
pub fn match_windows(wins: &[Win], targets: &[Target], own: u32) -> Vec<(Win, usize)> {
    wins.iter()
        .filter(|w| w.pid != own && markable(w))
        .filter_map(|w| {
            targets
                .iter()
                .position(|t| t.matches(w))
                .map(|i| (w.clone(), i))
        })
        .collect()
}

/// The window that has focus: the front normal window of the front app.
/// `wins` is the list front first.
pub fn focused_window(wins: &[Win], front_pid: Option<u32>) -> Option<u32> {
    let pid = front_pid?;
    wins.iter()
        .find(|w| w.pid == pid && w.layer == 0 && w.alpha > 0.0)
        .map(|w| w.id)
}

/// The parent chain of `pid`, from `pid` itself up, at most `max` steps.
/// It ends at launchd (1), at a process that is not in the table, or at a
/// loop.
pub fn ancestors(procs: &ProcTable, pid: u32, max: usize) -> Vec<u32> {
    let mut out = vec![pid];
    let mut p = pid;
    while out.len() <= max {
        match procs.get(&p) {
            Some(&q) if q > 1 && !out.contains(&q) => {
                out.push(q);
                p = q;
            }
            _ => break,
        }
    }
    out
}

/// The terminal window of a run, found at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalWindow {
    /// The window number of the window server.
    pub id: u32,
    /// The terminal app that owns the window.
    pub pid: u32,
}

/// Why a terminal run gets no mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoMark {
    /// No parent process owns a normal window: tmux, screen, ssh, a script.
    NoTerminal,
    /// The terminal app was not in front, so its front window is not
    /// surely the run's window.
    NotFront { terminal: u32, front: Option<u32> },
}

impl NoMark {
    /// The log line. `name` gives the name of an app from its pid.
    pub fn reason(&self, name: impl Fn(u32) -> String) -> String {
        match self {
            NoMark::NoTerminal => {
                "no window mark: no parent process of paddock owns a terminal window".into()
            }
            NoMark::NotFront { terminal, front } => {
                let front = match front {
                    Some(p) => format!(", {} is", name(*p)),
                    None => String::new(),
                };
                format!(
                    "no window mark: {} is not the front app at the start{front}",
                    name(*terminal)
                )
            }
        }
    }
}

/// The ancestor of the run at the most steps up that a terminal can be.
pub const TERMINAL_DEPTH: usize = 32;

/// The terminal window of a run that starts now. The terminal app is the
/// nearest ancestor of `me` that owns a normal window. It must be the front
/// app `front`, and its front window is the run's window: the user typed
/// the command there. `wins` is the window list, front first.
pub fn terminal_window(
    procs: &ProcTable,
    me: u32,
    wins: &[Win],
    front: Option<u32>,
) -> Result<TerminalWindow, NoMark> {
    let owns = |pid: u32| wins.iter().find(|w| w.pid == pid && markable(w));
    let pid = ancestors(procs, me, TERMINAL_DEPTH)
        .into_iter()
        .find(|&p| owns(p).is_some())
        .ok_or(NoMark::NoTerminal)?;
    if front != Some(pid) {
        return Err(NoMark::NotFront {
            terminal: pid,
            front,
        });
    }
    let w = owns(pid).ok_or(NoMark::NoTerminal)?;
    Ok(TerminalWindow { id: w.id, pid })
}

/// On macOS: the terminal window of a run that starts now, when `own`
/// (the run is watched in the caller's own terminal), and the marker for
/// it. None and one log line when there is no such window. None on other
/// platforms, which mark terminal windows their own way.
pub fn own_terminal_window(env: &HostEnv, own: bool) -> Option<TerminalWindow> {
    #[cfg(target_os = "macos")]
    if own {
        let w = macos::terminal_window_now();
        if w.is_some() {
            // The marker waits for the run record (GRACE).
            spawn(env);
        }
        return w;
    }
    let _ = (env, own);
    None
}

/// macOS: the windows on screen, front first, and the process table.
#[cfg(target_os = "macos")]
pub fn screen() -> (Vec<Win>, ProcTable) {
    (macos::windows(), procs::proc_table())
}

/// The variable that names the paddock binary for a process that is not
/// paddock itself (the session helper of the macOS guest): `spawn` runs
/// `paddock mark` from it.
pub const EXE_VAR: &str = "PADDOCK_EXE";

/// Start the marker in the background, unless one runs or the user turned
/// it off with `PADDOCK_MARK=0`. Never inside a sandbox. macOS only: other
/// platforms have their own window marks.
pub fn spawn(env: &HostEnv) {
    let off = env.var("PADDOCK").is_some() || env.var("PADDOCK_MARK") == Some("0");
    #[cfg(target_os = "macos")]
    if !off {
        let exe = env
            .var(EXE_VAR)
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_exe().ok());
        if let Some(exe) = exe {
            macos::spawn(env, &exe);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = off;
}

// ---- The firewall panel -------------------------------------------------

/// The panel of one `connect` question, as the marker reports it in
/// `panels.json` of the run folder, so `paddock windows --json` lists it
/// with `kind = approval`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PanelRow {
    /// The question id in the approvals store.
    pub question: String,
    /// The window number of the panel.
    pub window: u32,
    /// The marker process that owns the panel.
    pub pid: u32,
    /// The sandbox of the question (`macvm-<app>`).
    pub sandbox: String,
    pub harness: String,
}

/// The file with the panels of the marker.
pub fn panels_file(env: &HostEnv) -> std::path::PathBuf {
    env.paddock_run().join("mark-panels.json")
}

/// Write the panels of the marker. An empty list removes the file.
pub fn write_panels(env: &HostEnv, rows: &[PanelRow]) {
    let f = panels_file(env);
    if rows.is_empty() {
        let _ = std::fs::remove_file(&f);
        return;
    }
    let _ = crate::privfs::write_atomic(&f, serde_json::to_vec(rows).unwrap_or_default());
}

/// The panels of a marker that still runs.
pub fn read_panels(env: &HostEnv) -> Vec<PanelRow> {
    let rows: Vec<PanelRow> = std::fs::read(panels_file(env))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    rows.into_iter()
        .filter(|r| crate::state::alive(r.pid))
        .collect()
}

/// The width and height of the firewall panel.
pub const PANEL_SIZE: (f64, f64) = (360.0, 132.0);

/// Where the panel of a question goes: under the top-right corner of
/// the window of its session, where the badge sits, inside the screen
/// of that window. Without a window: the top-right of the main screen.
pub fn panel_rect(win: Option<Rect>, size: (f64, f64), screens: &[Rect]) -> Rect {
    let (w, h) = size;
    let main = screens
        .first()
        .copied()
        .unwrap_or(Rect::new(0.0, 0.0, w, h));
    let Some(win) = win else {
        return Rect::new(
            main.x + main.w - w - BADGE_INSET,
            main.y + BADGE_INSET,
            w,
            h,
        );
    };
    let screen = screens
        .iter()
        .find(|s| s.contains(win.x + win.w / 2.0, win.y.max(s.y)))
        .copied()
        .unwrap_or(main);
    let x = (win.x + win.w - w - BADGE_INSET)
        .min(screen.x + screen.w - w)
        .max(screen.x);
    let y = (win.y + BADGE_INSET + 22.0)
        .min(screen.y + screen.h - h)
        .max(screen.y);
    Rect::new(x, y, w, h)
}

/// `paddock mark`: draw the marks until no app sandbox runs.
pub fn main(env: &HostEnv) -> anyhow::Result<i32> {
    #[cfg(target_os = "macos")]
    {
        macos::main(env)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = env;
        anyhow::bail!("paddock mark draws window marks on macOS only")
    }
}

pub mod panels;
pub mod vm;

#[cfg(test)]
mod tests;
