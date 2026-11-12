//! `paddock split`: several terminals in one window, on paddock's own tmux
//! server (view.rs, `<runtime>/paddock/view.sock`), never on the user's
//! tmux server or with the user's tmux configuration.
//!
//! The split is the tmux session `split`. Each pane runs `paddock
//! split-pane JOB`: the job file holds the program, its folder, the
//! caller's environment and the label of the pane. The helper labels its
//! pane and then starts the program. A label is a set of pane options
//! (`@paddock_side`, `@paddock_title`, `@paddock_color`, and for a sandbox
//! pane `@paddock_badge`: running, waits for you, ended) that only paddock
//! sets. The border of each pane shows the label, the side always as a
//! word, not only as a color. A program in a pane
//! cannot change it: an OSC title changes `pane_title`, which no format of
//! the split reads, and renames are off. A pane without a label, for
//! example one that the user opened with Ctrl-B %, shows as a host shell.
//!
//! The runtime folder is hidden in every sandbox, so no sandbox reaches the
//! socket or the job files.

use super::Tmux;
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use title::on_off;

/// The tmux session of the split. Sandbox ids always have two `-`, so no
/// sandbox has this name.
pub const SESSION: &str = "split";
/// The target of the session, exact.
pub const TARGET: &str = "=split";
/// The border color of a sandbox pane: the paddock accent (sage).
pub const SANDBOX_COLOR: &str = "#9fb8a0";
/// The border color of a host pane: the warning tone (amber).
pub const HOST_COLOR: &str = "#e0a33a";
/// The key line of the status bar.
pub const KEYS: &str = "Ctrl-B o next · Ctrl-B z zoom · Ctrl-B d leave";
/// The label of a pane that paddock did not label.
pub const UNLABELED: &str = "host · shell";
/// A title keeps at most this many characters.
const TITLE_CHARS: usize = 96;
/// The kind or harness in a title keeps at most this many characters.
const WHAT_CHARS: usize = 24;
/// A job file is at most this large.
const JOB_MAX: u64 = 4 << 20;
/// How long `paddock split` waits for the panes to get their labels.
const LABEL_WAIT: Duration = Duration::from_secs(10);
/// A window at least this wide shows a board pane on the left.
pub const WIDE: u32 = 140;
/// The board pane on the left takes this share of the width, at least
/// BOARD_MIN columns.
const BOARD_SHARE: u32 = 30;
const BOARD_MIN: u32 = 36;

/// Whether a pane runs in a sandbox or on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Sandbox,
    Host,
}

impl Side {
    pub fn word(self) -> &'static str {
        match self {
            Side::Sandbox => "sandbox",
            Side::Host => "host",
        }
    }

    pub fn color(self) -> &'static str {
        match self {
            Side::Sandbox => SANDBOX_COLOR,
            Side::Host => HOST_COLOR,
        }
    }
}

/// The kinds of pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `run HARNESS [ARGS]`: a new sandbox run, as `paddock HARNESS`.
    Run,
    /// `view ID [--write]`: an existing run, read-only unless --write.
    View,
    /// `shell`: a host shell in the current folder.
    Shell,
    /// `board`: `paddock board --watch`.
    Board,
    /// `mac [APP]`: `paddock mac view`, macOS only.
    Mac,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Run => "run",
            Kind::View => "view",
            Kind::Shell => "shell",
            Kind::Board => "board",
            Kind::Mac => "mac",
        }
    }
}

/// One pane as the user wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub kind: Kind,
    pub args: Vec<String>,
}

/// The word that separates the panes on the command line.
pub const SEPARATOR: &str = "+";

const KINDS: &str = "run HARNESS [ARGS], view ID [--write], shell, board or mac [APP]";

/// The panes of `paddock split PANES...`, separated by a lone `+`.
pub fn parse(words: &[String]) -> Result<Vec<Pane>> {
    words
        .split(|w| w == SEPARATOR)
        .map(parse_one)
        .collect::<Result<Vec<_>>>()
}

/// One pane: the kind and its arguments.
pub fn parse_one(words: &[String]) -> Result<Pane> {
    let Some((kind, args)) = words.split_first() else {
        bail!("a pane is empty: write one of {KINDS}, and put a lone + between two panes");
    };
    let args = args.to_vec();
    let kind = match kind.as_str() {
        "run" => {
            match args.first() {
                Some(h) if !h.starts_with('-') && !h.is_empty() => {}
                _ => bail!("a run pane needs a harness first, for example: run claude"),
            }
            Kind::Run
        }
        "view" => {
            let ids: Vec<&String> = args.iter().filter(|a| *a != "--write").collect();
            if ids.len() != 1 || ids[0].starts_with('-') {
                bail!("a view pane needs one sandbox id, for example: view 1a2b3c");
            }
            Kind::View
        }
        "shell" | "board" => {
            if !args.is_empty() {
                bail!("a {kind} pane takes no arguments");
            }
            if kind == "shell" {
                Kind::Shell
            } else {
                Kind::Board
            }
        }
        "mac" => {
            if !cfg!(target_os = "macos") {
                bail!("a mac pane shows the macOS guest (paddock mac view), and it needs macOS");
            }
            if args.len() > 1 || args.first().is_some_and(|a| a.starts_with('-')) {
                bail!(
                    "a mac pane takes at most the name of an app session, for example: mac safari"
                );
            }
            Kind::Mac
        }
        other => bail!("{other} is not a kind of pane. The kinds: {KINDS}"),
    };
    Ok(Pane { kind, args })
}

/// Text that is safe in a border: no control characters, no `#` (tmux
/// reads `#[` in the expanded border as a style), and not too long. A long
/// text keeps its start, so a label keeps its side word.
pub fn clean(text: &str) -> String {
    let out: Vec<char> = safe(text).chars().collect();
    if out.len() <= TITLE_CHARS {
        return out.into_iter().collect();
    }
    let mut cut: String = out[..TITLE_CHARS - 1].iter().collect();
    cut.push('…');
    cut
}

/// No control characters and no `#`.
fn safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() || c == '#' { '?' } else { c })
        .collect()
}

/// `text` in at most `max` characters: a long text keeps its start and its
/// end, with `…` in the middle.
fn middle(text: &str, max: usize) -> String {
    let v: Vec<char> = text.chars().collect();
    if v.len() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let keep = max - 1;
    let head = keep / 3;
    let tail = keep - head;
    let mut out: String = v[..head].iter().collect();
    out.push('…');
    out.extend(&v[v.len() - tail..]);
    out
}

/// The label of a pane: `sandbox · claude · ID`, `host · shell · ~/src`.
/// The side word and the kind always stay: a long detail loses its middle.
pub fn title(side: Side, what: &str, detail: &str) -> String {
    let what = middle(&safe(what), WHAT_CHARS);
    let head = format!("{} · {what} · ", side.word());
    let room = TITLE_CHARS - head.chars().count();
    format!("{head}{}", middle(&safe(detail), room))
}

/// A folder for a title: `~/src/app` below the home.
pub fn folder(home: &Path, dir: &Path) -> String {
    match dir.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    }
}

/// The border of every pane. The active pane shows reversed. A pane
/// without a label shows as a host shell, in the host color.
pub fn border_format() -> String {
    format!(
        "#{{?pane_active,#[reverse],}}#[fg=#{{?@paddock_color,#{{@paddock_color}},{HOST_COLOR}}}] \
         #{{?@paddock_title,#{{@paddock_title}},{UNLABELED}}} \
         #{{?@paddock_badge,#[bold][#{{@paddock_badge}}]#[nobold] ,}}#[default]"
    )
}

/// The window options of the split, also for each new window.
fn window_options() -> Vec<(&'static str, String)> {
    vec![
        ("pane-border-status", "top".into()),
        ("pane-border-format", border_format()),
        ("allow-rename", "off".into()),
        ("automatic-rename", "off".into()),
        ("remain-on-exit", "off".into()),
        ("allow-set-title", "off".into()),
    ]
}

/// The status line names the side of the active pane, from its label: a
/// program in a pane can draw a fake border in its own pane, but not this.
pub fn status_left() -> String {
    format!(
        " paddock split · #[fg=#{{?@paddock_color,#{{@paddock_color}},{HOST_COLOR}}}]\
         #{{?@paddock_side,#{{@paddock_side}},host}}#[default] "
    )
}

/// The session options: the key line, no host name, no clock, no titles.
fn session_options() -> Vec<(&'static str, String)> {
    vec![
        ("status", "on".into()),
        ("status-left", status_left()),
        ("status-left-length", "40".into()),
        ("status-right", format!("{KEYS} ")),
        ("status-right-length", "80".into()),
        ("status-style", "default".into()),
        ("window-status-format", " #I ".into()),
        ("window-status-current-format", "[#I]".into()),
        ("set-titles", "off".into()),
        ("mouse", "on".into()),
        ("prefix", "C-b".into()),
    ]
}

/// One tmux invocation with commands separated by `;`.
fn chain(cmds: Vec<Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for (i, c) in cmds.into_iter().enumerate() {
        if i > 0 {
            out.push(";".into());
        }
        out.extend(c);
    }
    out
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// The commands that style the session `split` and its windows. `-q`:
/// an older tmux without an option skips it. The target is a pane
/// (`=split:`, the active window): set-option and set-hook take a pane,
/// and the session of that pane gets the session options. `layout`: the
/// shell command that lays out the split again when a terminal attaches or
/// changes its size (layout_hook).
pub fn style_args(layout: Option<&str>) -> Vec<String> {
    let mut cmds = Vec::new();
    let win = format!("{TARGET}:");
    for (k, v) in session_options() {
        cmds.push(vec![
            "set-option".into(),
            "-q".into(),
            "-t".into(),
            win.clone(),
            k.into(),
            v,
        ]);
    }
    for (k, v) in window_options() {
        cmds.push(vec![
            "set-option".into(),
            "-q".into(),
            "-w".into(),
            "-t".into(),
            win.clone(),
            k.into(),
            v,
        ]);
    }
    cmds.push(vec![
        "rename-window".into(),
        "-t".into(),
        win.clone(),
        "paddock".into(),
    ]);
    // A window that the user opens (Ctrl-B c) gets the same border.
    let hook: Vec<String> = window_options()
        .into_iter()
        .map(|(k, v)| format!("set-option -q -w {k} '{v}'"))
        .chain(["rename-window paddock".to_string()])
        .collect();
    cmds.push(vec![
        "set-hook".into(),
        "-t".into(),
        win.clone(),
        "after-new-window".into(),
        hook.join(" ; "),
    ]);
    cmds.push(title::session_args(&win));
    if let Some(cmd) = layout {
        for h in ["client-attached", "client-resized"] {
            cmds.push(vec![
                "set-hook".into(),
                "-t".into(),
                win.clone(),
                h.into(),
                format!("run-shell -b '{cmd}'"),
            ]);
        }
    }
    chain(cmds)
}

/// The shell command of the layout hook: `paddock split-layout TMUX SOCK`.
/// None when a path holds a character that needs quoting: tmux expands
/// `#` in run-shell and the shell reads quotes, so only plain paths go in.
/// Then the layout stays as paddock made it.
pub fn layout_hook(me: &Path, t: &Tmux) -> Option<String> {
    let plain = |p: &Path| {
        let s = p.to_str()?;
        (p.is_absolute()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._-+@%:,".contains(&b)))
        .then(|| s.to_string())
    };
    // tmux shows the output of a hook in the pane: none, also when an
    // update moved paddock.
    Some(format!(
        "{} split-layout {} {} >/dev/null 2>&1 || true",
        plain(me)?,
        plain(&t.exe)?,
        plain(&t.sock)?
    ))
}

/// The commands that label pane `pane`.
pub fn label_args(pane: &str, job: &Job) -> Vec<String> {
    let set = |k: &str, v: &str| s(&["set-option", "-q", "-p", "-t", pane, k, v]);
    chain(vec![
        set("@paddock_side", job.side.word()),
        set("@paddock_color", job.side.color()),
        set("@paddock_title", &clean(&job.title)),
        set("remain-on-exit", on_off(job.side == Side::Sandbox)),
        set("allow-set-title", "off"),
        set("allow-rename", "off"),
        set("allow-passthrough", on_off(job.passthrough)),
    ])
}

/// The command that reads back the side of pane `pane`: the helper of a
/// sandbox pane starts its program only once it reads `sandbox`.
pub fn side_args(pane: &str) -> Vec<String> {
    s(&["display-message", "-p", "-t", pane, "#{@paddock_side}"])
}

/// The badge of a sandbox pane from the state of its run on the board.
/// None: the run does not run (any more).
pub fn badge(state: Option<crate::board::State>) -> &'static str {
    use crate::board::State as S;
    match state {
        None => "ended",
        Some(S::Working | S::Open) => "running",
        Some(S::Waiting | S::Blocked | S::Stuck | S::Idle | S::Done) => "waits for you",
        Some(S::Ended) => "ended",
    }
}

/// The command that sets the badge of pane `pane`.
pub fn badge_args(pane: &str, badge: &str) -> Vec<String> {
    s(&[
        "set-option",
        "-q",
        "-p",
        "-t",
        pane,
        "@paddock_badge",
        badge,
    ])
}

/// The commands that give a run pane the id of its sandbox.
pub fn id_args(pane: &str, harness: &str, id: &str) -> Vec<String> {
    let set = |k: &str, v: &str| s(&["set-option", "-q", "-p", "-t", pane, k, v]);
    chain(vec![
        set("@paddock_id", id),
        set("@paddock_title", &title(Side::Sandbox, harness, id)),
    ])
}

/// What one pane runs: the job file of `paddock split-pane`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Job {
    pub side: Side,
    pub title: String,
    /// The program and its arguments, no shell in between.
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// The caller's environment, without TERM, TMUX and TMUX_PANE.
    pub env: Vec<(String, String)>,
    /// The program gets TMUX and TMUX_PANE of the pane: a host program
    /// that can run `paddock split add`. Not a view: its tmux client
    /// refuses to start inside tmux.
    pub keep_tmux: bool,
    pub kind: Kind,
    /// A sandbox pane: the helper stays, keeps the badge current and, for a
    /// run, puts the sandbox id in the label once the sandbox runs.
    pub watch: Option<Watch>,
    /// The mac view draws images through tmux to the terminal.
    pub passthrough: bool,
    /// paddock's tmux server, so the helper can label the pane.
    pub tmux: PathBuf,
    pub sock: PathBuf,
}

/// What the helper of a sandbox pane watches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Watch {
    pub harness: String,
    /// The sandbox id. None for a run until its sandbox starts, and for
    /// the mac view, which has no sandbox id: its badge says whether the
    /// view runs.
    pub id: Option<String>,
    /// A run: find the sandbox that the program starts.
    pub discover: bool,
}

/// The folder of the job files.
pub fn jobs_dir(env: &HostEnv) -> PathBuf {
    env.paddock_run().join("split")
}

/// Write a job file for one pane.
pub fn write_job(env: &HostEnv, job: &Job) -> Result<PathBuf> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = jobs_dir(env);
    crate::privfs::dir(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = dir.join(format!("{}-{nanos}-{n}.job", std::process::id()));
    crate::privfs::write(&file, serde_json::to_vec(job)?)
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(file)
}

/// Read and remove a job file. It must be a private file of this user.
pub fn take_job(file: &Path) -> Result<Job> {
    let m = std::fs::symlink_metadata(file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    if !m.is_file() || m.uid() != me || m.mode() & 0o077 != 0 || m.len() > JOB_MAX {
        bail!("{} is not a private file of this user", file.display());
    }
    let bytes = std::fs::read(file)?;
    let _ = std::fs::remove_file(file);
    Ok(serde_json::from_slice(&bytes)?)
}

impl Tmux {
    /// The output of a tmux command, when it succeeds.
    fn output(&self, args: &[String]) -> Option<String> {
        if !self.sock.exists() {
            return None;
        }
        let out = self.cmd().args(args).stdout(Stdio::piped()).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// The program that a pane runs: the helper with its job file.
fn pane_cmd(me: &Path, job: &Path) -> Vec<std::ffi::OsString> {
    vec![me.into(), "split-pane".into(), job.into()]
}

/// Start the split with `jobs`, the first pane on top. `me`: the paddock
/// executable. `size`: the columns and lines of the terminal that attaches,
/// so the layout fits it from the start. Waits until every pane has its
/// label and returns the panes that have none.
pub fn start(
    t: &Tmux,
    env: &HostEnv,
    me: &Path,
    jobs: &[Job],
    size: Option<(u32, u32)>,
) -> Result<Vec<String>> {
    let Some((first, rest)) = jobs.split_first() else {
        bail!("a split needs at least one pane");
    };
    if t.has(SESSION) {
        bail!("a split runs already");
    }
    title::check_version(t)?;
    let file = write_job(env, first)?;
    if let Err(e) = t.new_session_sized(SESSION, &pane_cmd(me, &file), size) {
        let _ = std::fs::remove_file(&file);
        return Err(e);
    }
    let window = format!("{TARGET}:");
    let _ = t.output(&style_args(layout_hook(me, t).as_deref()));
    // `-q` hides every error. Without the border and the status line, a
    // pane cannot be told from another: no split then.
    let read = |args: &[&str]| t.output(&s(args)).map(|o| o.trim_end().to_string());
    let border = read(&[
        "show-options",
        "-w",
        "-v",
        "-t",
        &window,
        "pane-border-format",
    ]);
    let status = read(&["show-options", "-v", "-t", &window, "status-right"]);
    let titles = read(&["show-options", "-v", "-t", &window, "set-titles"]);
    if border != Some(border_format())
        || status.as_deref().map(str::trim) != Some(KEYS)
        || titles.as_deref() != Some("off")
        || !title::took(t, &window)
    {
        let _ = t.output(&s(&["kill-session", "-t", TARGET]));
        bail!(
            "tmux did not take the pane labels of the split, so a sandbox pane could not be told from a host pane. paddock split needs tmux 3.2 or newer"
        );
    }
    let top = t
        .output(&s(&["display-message", "-p", "-t", &window, "#{pane_id}"]))
        .map(|o| o.trim().to_string())
        .unwrap_or_default();
    let _ = t.output(&kind_args(&top, first.kind));
    for job in rest {
        add_to(t, env, me, job, &window, false)?;
    }
    let _ = t.output(&s(&["select-pane", "-t", &top]));
    Ok(wait_labels(t))
}

/// Add a pane to the split, below `target` (a pane id), or else in the
/// active window. Returns the id of the new pane and the panes without a
/// label.
pub fn add(
    t: &Tmux,
    env: &HostEnv,
    me: &Path,
    job: &Job,
    target: Option<&str>,
) -> Result<(String, Vec<String>)> {
    if !t.has(SESSION) {
        bail!("no split runs. Start one with `paddock split`");
    }
    let target = match target {
        Some(p) if in_split(t, p) => p.to_string(),
        _ => format!("{TARGET}:"),
    };
    let pane = add_to(t, env, me, job, &target, true)?;
    Ok((pane, wait_labels(t)))
}

fn add_to(
    t: &Tmux,
    env: &HostEnv,
    me: &Path,
    job: &Job,
    target: &str,
    focus: bool,
) -> Result<String> {
    let file = write_job(env, job)?;
    let mut args = s(&["split-window", "-v", "-P", "-F", "#{pane_id}", "-t", target]);
    if !focus {
        args.push("-d".into());
    }
    args.extend(
        pane_cmd(me, &file)
            .iter()
            .map(|a| a.to_string_lossy().into_owned()),
    );
    let Some(out) = t.output(&args) else {
        let _ = std::fs::remove_file(&file);
        bail!("tmux could not add a pane. The window may be too small: zoom out (Ctrl-B z) or close a pane");
    };
    let pane = out.trim().to_string();
    let _ = t.output(&kind_args(&pane, job.kind));
    if let Some(w) = t.output(&s(&["display-message", "-p", "-t", &pane, "#{window_id}"])) {
        relayout(t, w.trim());
    }
    Ok(pane)
}

/// The kind of a pane, for the layout. paddock sets it when it adds the pane.
fn kind_args(pane: &str, kind: Kind) -> Vec<String> {
    s(&[
        "set-option",
        "-q",
        "-p",
        "-t",
        pane,
        "@paddock_kind",
        kind.word(),
    ])
}

/// How the panes of a window sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The board on the left, the other panes from top to bottom on the
    /// right. `board`: the index of the board pane now.
    BoardLeft { board: usize, width: u32 },
    /// From top to bottom.
    Stack,
    /// A grid, for many panes.
    Tiled,
}

/// The layout of a window with `n` panes, `width` columns wide, and a
/// board pane at index `board`.
pub fn layout(n: usize, width: u32, board: Option<usize>) -> Layout {
    match board {
        Some(b) if n >= 2 && width >= WIDE => Layout::BoardLeft {
            board: b,
            width: (width * BOARD_SHARE / 100).max(BOARD_MIN),
        },
        _ if n <= 3 => Layout::Stack,
        _ => Layout::Tiled,
    }
}

/// Lay out every window of the split again, for the size it has now: the
/// layout hook, when a terminal attaches or changes its size.
pub fn relayout_all(t: &Tmux) {
    let Some(out) = t.output(&s(&["list-windows", "-t", TARGET, "-F", "#{window_id}"])) else {
        return;
    };
    for w in out.lines().filter(|w| w.starts_with('@')) {
        relayout(t, w);
    }
}

/// Lay out the panes of window `window` again. One layout at a time: a
/// terminal that attaches starts two hooks at once (client-attached and
/// client-resized), and two swaps of the board would undo each other.
fn relayout(t: &Tmux, window: &str) {
    let _lock = layout_lock(t);
    let fmt = "#{pane_id}\t#{@paddock_kind}\t#{window_width}";
    let Some(out) = t.output(&s(&["list-panes", "-t", window, "-F", fmt])) else {
        return;
    };
    let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    let Some(first) = rows.first().filter(|r| r.len() == 3) else {
        return;
    };
    let width = first[2].parse().unwrap_or(0);
    let board = rows.iter().position(|r| r.get(1) == Some(&"board"));
    match layout(rows.len(), width, board) {
        Layout::BoardLeft { board, width } => {
            // main-vertical puts the first pane on the left.
            if board != 0 {
                let _ = t.output(&s(&[
                    "swap-pane",
                    "-d",
                    "-s",
                    rows[board][0],
                    "-t",
                    first[0],
                ]));
            }
            let w = width.to_string();
            let _ = t.output(&s(&[
                "set-option",
                "-q",
                "-w",
                "-t",
                window,
                "main-pane-width",
                &w,
            ]));
            let _ = t.output(&s(&["select-layout", "-t", window, "main-vertical"]));
        }
        Layout::Stack => {
            let _ = t.output(&s(&["select-layout", "-t", window, "even-vertical"]));
        }
        Layout::Tiled => {
            let _ = t.output(&s(&["select-layout", "-t", window, "tiled"]));
        }
    }
}

/// An exclusive lock beside the socket, held until the file closes. None
/// when the file cannot be opened: the layout goes on without it.
fn layout_lock(t: &Tmux) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(t.sock.with_file_name("split-layout.lock"))
        .ok()?;
    // SAFETY: flock only locks the open file.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0).then_some(file)
}

/// Whether pane `pane` belongs to the split.
fn in_split(t: &Tmux, pane: &str) -> bool {
    t.output(&s(&[
        "display-message",
        "-p",
        "-t",
        pane,
        "#{session_name}",
    ]))
    .is_some_and(|o| o.trim() == SESSION)
}

/// Wait until every pane that paddock added to the split has its label, at
/// most LABEL_WAIT. Returns the live ones that have none: they show as host
/// shells. The helper of a sandbox pane starts no program without its
/// label. A pane that the user opened with tmux keys has no label anyway.
fn wait_labels(t: &Tmux) -> Vec<String> {
    let end = Instant::now() + LABEL_WAIT;
    let fmt = "#{pane_id}\t#{@paddock_kind}\t#{@paddock_side}\t#{pane_dead}";
    loop {
        let missing: Vec<String> = t
            .output(&s(&["list-panes", "-s", "-t", TARGET, "-F", fmt]))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\t').collect::<Vec<_>>())
            .filter(|f| f.len() == 4 && !f[1].is_empty() && f[2].is_empty() && f[3] != "1")
            .map(|f| f[0].to_string())
            .collect();
        if missing.is_empty() || Instant::now() >= end {
            return missing;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One pane of `paddock split ls`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Listed {
    /// WINDOW.PANE, as tmux counts them.
    pub pane: String,
    /// The tmux pane id, for example `%3`.
    pub id: String,
    /// None: paddock did not label the pane. It shows as a host shell.
    pub side: Option<Side>,
    pub title: String,
    /// running, waits for you or ended, for a sandbox pane.
    pub badge: Option<String>,
    /// The sandbox id of a run pane, once it is known.
    pub sandbox: Option<String>,
    pub dead: bool,
    pub active: bool,
}

/// The panes of the split. Empty when no split runs.
pub fn list(t: &Tmux) -> Vec<Listed> {
    let fmt = "#{window_index}.#{pane_index}\t#{pane_id}\t#{@paddock_side}\t#{@paddock_title}\t#{@paddock_id}\t#{pane_dead}\t#{&&:#{pane_active},#{window_active}}\t#{@paddock_badge}";
    let Some(out) = t.output(&s(&["list-panes", "-s", "-t", TARGET, "-F", fmt])) else {
        return vec![];
    };
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() != 8 {
                return None;
            }
            let side = match f[2] {
                "sandbox" => Some(Side::Sandbox),
                "host" => Some(Side::Host),
                _ => None,
            };
            Some(Listed {
                pane: f[0].into(),
                id: f[1].into(),
                side,
                title: if f[3].is_empty() {
                    UNLABELED.into()
                } else {
                    f[3].into()
                },
                badge: (!f[7].is_empty()).then(|| f[7].into()),
                sandbox: (!f[4].is_empty()).then(|| f[4].into()),
                dead: f[5] == "1",
                active: f[6] == "1",
            })
        })
        .collect()
}

/// The pane of the caller, when it runs in a pane of paddock's tmux server:
/// `TMUX` names paddock's socket. Then `TMUX_PANE`.
pub fn caller_pane(t: &Tmux, tmux_var: Option<&str>, pane_var: Option<&str>) -> Option<String> {
    let sock = tmux_var?.split(',').next()?;
    (Path::new(sock) == t.sock)
        .then(|| pane_var.filter(|p| p.starts_with('%')).map(str::to_string))
        .flatten()
}

/// Whether the caller runs in a pane of paddock's tmux server.
pub fn in_paddock_tmux(t: &Tmux, tmux_var: Option<&str>) -> bool {
    tmux_var
        .and_then(|v| v.split(',').next())
        .is_some_and(|sock| Path::new(sock) == t.sock)
}

/// The command that attaches a terminal to the split.
pub fn attach_argv(t: &Tmux) -> Vec<std::ffi::OsString> {
    vec![
        t.exe.clone().into(),
        "-S".into(),
        t.sock.clone().into(),
        "attach-session".into(),
        "-t".into(),
        TARGET.into(),
    ]
}

/// Whether process `pid` is `ancestor` or one of its descendants.
pub fn descends(pid: u32, ancestor: u32) -> bool {
    let mut p = pid;
    for _ in 0..32 {
        if p == ancestor {
            return true;
        }
        if p <= 1 {
            return false;
        }
        match parent_pid(p) {
            Some(pp) => p = pp,
            None => return false,
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?; // not user text
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn parent_pid(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(test)]
#[path = "split_tests.rs"]
mod tests;
#[path = "split_tmux.rs"]
pub mod title;
