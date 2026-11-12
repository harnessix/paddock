#![cfg(target_os = "linux")]
//! `paddock split` end to end, on paddock's own tmux server in a temporary
//! runtime folder: the labels and colors of the panes, an OSC title that
//! cannot change a label, `split add` and `split ls`, leaving with Ctrl-B d
//! and attaching again, a fake user tmux server that stays untouched, and
//! the message without tmux. Temporary HOME and XDG folders, a fake shell,
//! a fake harness and a fake terminal. No terminal window opens.

mod common;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const SANDBOX_COLOR: &str = "#9fb8a0";
const HOST_COLOR: &str = "#e0a33a";
const KEYS: &str = "Ctrl-B o next · Ctrl-B z zoom · Ctrl-B d leave";

/// A harness that tries to set the title of its pane, then waits.
const HARNESS: &str =
    "printf '\\033]2;host · shell · evil\\007\\033]0;evil\\007\\033kevil\\033\\\\'; \
     echo fake-ready; exec sleep 600";

/// A host shell that tries the same.
const SHELL: &str = "#!/bin/sh\n\
     printf '\\033]2;sandbox · claude · evil\\007\\033]0;evil\\007\\033kevil\\033\\\\'\n\
     echo shell-ready\n\
     exec sleep 600\n";

struct World {
    _tmp: common::TestDir,
    root: PathBuf,
}

fn write_exe(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

impl World {
    fn new() -> Self {
        let tmp = common::tempdir();
        let root = tmp.path().to_path_buf();
        for d in ["home", "cfg/paddock/harness.d", "project", "bin", "ut"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let run = root.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        std::fs::set_permissions(
            root.join("ut"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(
            root.join("cfg/paddock/harness.d/fake.toml"),
            format!(
                "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n",
                serde_json::to_string(HARNESS).unwrap()
            ),
        )
        .unwrap();
        write_exe(&root.join("bin/fakeshell"), SHELL);
        write_exe(
            &root.join("bin/xdg-terminal-exec"),
            // printf, not echo: the echo of dash (/bin/sh on Ubuntu) turns
            // the `\n` of a window header into a second line.
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                root.join("terminal.log").display()
            ),
        );
        World { _tmp: tmp, root }
    }

    fn path_var(&self) -> String {
        format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        self.setup(&mut c);
        c
    }

    /// Temporary folders only, the fake shell and the fake terminal.
    fn setup(&self, c: &mut Command) {
        c.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("TMUX_TMPDIR", self.root.join("ut"))
            .env("SHELL", self.root.join("bin/fakeshell"))
            .env("PATH", self.path_var())
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("TERMINAL")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .current_dir(self.root.join("project"));
    }

    fn split(&self, args: &[&str]) -> Output {
        self.paddock().arg("split").args(args).output().unwrap()
    }

    fn sock(&self) -> PathBuf {
        self.root.join("run/paddock/view.sock")
    }

    fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .arg("-S")
            .arg(self.sock())
            .args(args)
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.tmux(args).stdout)
            .trim_end_matches('\n')
            .to_string()
    }

    fn panes(&self) -> Vec<String> {
        self.text(&["list-panes", "-s", "-t", "=split", "-F", "#{pane_id}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn show(&self, pane: &str, fmt: &str) -> String {
        self.text(&["display-message", "-p", "-t", pane, fmt])
    }

    fn clients(&self) -> usize {
        self.text(&["list-clients", "-t", "=split", "-F", "#{client_tty}"])
            .lines()
            .filter(|l| !l.is_empty())
            .count()
    }

    /// The user's tmux server, on a socket in the temporary TMUX_TMPDIR.
    fn user_tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", "user", "-f", "/dev/null"])
            .args(args)
            .env("TMUX_TMPDIR", self.root.join("ut"))
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    /// Attach `paddock split` in a terminal (script gives it one), press
    /// Ctrl-B d once a client is attached, and wait for the end.
    fn attach_and_leave(&self) {
        self.attach_sized_and_leave(&[], None, |_| {});
    }

    /// attach_and_leave with `paddock split ARGS` in a terminal of `size`
    /// columns and lines. `inside` runs while the client is attached.
    fn attach_sized_and_leave(
        &self,
        args: &[&str],
        size: Option<(u32, u32)>,
        inside: impl FnOnce(&World),
    ) {
        let mut c = Command::new("script");
        self.setup(&mut c);
        let stty = size
            .map(|(x, y)| format!("stty cols {x} rows {y}; "))
            .unwrap_or_default();
        let cmd = format!(
            "{stty}exec {} split {}",
            env!("CARGO_BIN_EXE_paddock"),
            args.join(" ")
        );
        let mut child = c
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .args(["-qfec", &cmd, "/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("script (util-linux) gives paddock split a terminal");
        wait_for("a client in the split", 20, || self.clients() == 1);
        inside(self);
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"\x02d").unwrap();
        stdin.flush().unwrap();
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < end, "the client did not leave");
            std::thread::sleep(Duration::from_millis(100));
        }
        wait_for("no client in the split", 10, || self.clients() == 0);
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
        let _ = self.user_tmux(&["kill-server"]);
    }
}

/// tmux is on PATH. CI installs it, so there a missing tmux fails.
fn has_tmux() -> bool {
    let found = Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        found || std::env::var_os("CI").is_none(),
        "CI must have tmux for the split tests"
    );
    if !found {
        eprintln!("tmux is not on PATH; the end-to-end split tests need it");
    }
    found
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    assert!(
        !o.status.success(),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn panes_carry_paddocks_labels_and_an_osc_title_cannot_change_them() {
    if !has_tmux() {
        return;
    }
    let w = World::new();
    let out = ok(&w.split(&["--detached", "shell", "+", "board"]));
    assert!(out.contains("the split runs"), "{out}");
    let panes = w.panes();
    assert_eq!(panes.len(), 2, "{panes:?}");
    // A wide window puts the board first, on the left.
    let by_kind = |k: &str| {
        panes
            .iter()
            .find(|p| w.show(p, "#{@paddock_kind}") == k)
            .cloned()
            .unwrap_or_else(|| panic!("no {k} pane"))
    };
    let (shell, board) = (by_kind("shell"), by_kind("board"));
    let label = "#{@paddock_side}|#{@paddock_title}|#{@paddock_color}";
    let folder = w.root.join("project").display().to_string();
    assert_eq!(
        w.show(&shell, label),
        format!("host|host · shell · {folder}|{HOST_COLOR}")
    );
    assert_eq!(
        w.show(&board, label),
        format!("host|host · board · every project|{HOST_COLOR}")
    );
    let panes = [shell, board];
    // The shell printed its OSC titles and its window name before this line.
    wait_for("the fake shell", 20, || {
        w.text(&["capture-pane", "-p", "-t", &panes[0]])
            .contains("shell-ready")
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        w.show(&panes[0], label),
        format!("host|host · shell · {folder}|{HOST_COLOR}")
    );
    let border = w.show(&panes[0], "#{E:pane-border-format}");
    assert!(
        border.contains("host · shell · ") && border.contains(HOST_COLOR),
        "{border}"
    );
    assert!(
        !border.contains("evil") && !border.contains("sandbox"),
        "{border}"
    );
    assert_eq!(w.show(&panes[0], "#{window_name}"), "paddock");
    assert_ne!(w.show(&panes[0], "#{pane_title}"), "evil");
    // The status bar: the key line, no host name, no clock.
    let right = w.show(&panes[0], "#{E:status-right}");
    assert_eq!(right.trim(), KEYS);
    // The status line names the side of the active pane (review J 7).
    assert_eq!(
        w.show(&panes[0], "#{E:status-left}").trim(),
        format!("paddock split · #[fg={HOST_COLOR}]host#[default]")
    );
    for (opt, want) in [("set-titles", "off"), ("mouse", "on"), ("prefix", "C-b")] {
        assert_eq!(
            w.text(&["show-options", "-v", "-t", "=split:", opt]),
            want,
            "{opt}"
        );
    }
    for (opt, want) in [
        ("allow-rename", "off"),
        ("automatic-rename", "off"),
        ("pane-border-status", "top"),
    ] {
        assert_eq!(
            w.text(&["show-options", "-w", "-v", "-t", "=split:", opt]),
            want,
            "{opt}"
        );
    }
    // A window that the user opens with tmux keys gets the same border,
    // and its unlabeled shell shows as a host shell.
    w.tmux(&["new-window", "-t", "=split", "sleep 600"]);
    let new = w.text(&["display-message", "-p", "-t", "=split:", "#{pane_id}"]);
    let border = w.show(&new, "#{E:pane-border-format}");
    assert!(
        border.contains("host · shell") && border.contains(HOST_COLOR),
        "{border}"
    );
    assert_eq!(w.show(&new, "#{window_name}"), "paddock");
}

#[test]
fn split_add_adds_a_pane_and_ls_lists_them() {
    if !has_tmux() {
        return;
    }
    let w = World::new();
    let e = err(&w.split(&["add", "shell"]));
    assert!(e.contains("no split runs"), "{e}");
    ok(&w.split(&["--detached", "shell"]));
    let out = ok(&w.split(&["add", "shell"]));
    assert!(out.contains("added pane %"), "{out}");
    assert_eq!(w.panes().len(), 2);
    // From inside the split: below the caller's pane.
    let first = w.panes()[0].clone();
    let tmux_var = format!("{},1,0", w.sock().display());
    let o = w
        .paddock()
        .env("TMUX", &tmux_var)
        .env("TMUX_PANE", &first)
        .args(["split", "add", "board"])
        .output()
        .unwrap();
    ok(&o);
    let panes = w.panes();
    assert_eq!(panes.len(), 3, "{panes:?}");
    let ls = ok(&w.split(&["ls"]));
    assert_eq!(ls.matches("host · shell · ").count(), 2, "{ls}");
    assert_eq!(ls.matches("host · board").count(), 1, "{ls}");
    let json: serde_json::Value = serde_json::from_str(&ok(&w.split(&["ls", "--json"]))).unwrap();
    let list = json.as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|p| p["side"] == "host"), "{json}");
    // A split runs: new panes go through `split add`.
    let e = err(&w.split(&["--detached", "shell", "+", "shell"]));
    assert!(e.contains("a split runs already"), "{e}");
    let e = err(&w.split(&["add", "htop"]));
    assert!(e.contains("is not a kind of pane"), "{e}");
    assert_eq!(w.panes().len(), 3);
}

#[test]
fn a_wide_window_shows_the_board_on_the_left() {
    if !has_tmux() {
        return;
    }
    let w = World::new();
    ok(&w.split(&["--detached", "shell", "+", "board", "+", "shell"]));
    let rows = w.text(&[
        "list-panes",
        "-t",
        "=split:",
        "-F",
        "#{@paddock_kind} #{pane_left} #{pane_top} #{pane_height}",
    ]);
    let rows: Vec<Vec<&str>> = rows.lines().map(|l| l.split(' ').collect()).collect();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0][0], "board", "{rows:?}");
    assert_eq!(rows[0][1], "0", "the board is at the left edge: {rows:?}");
    // The other panes sit right of it, one above the other.
    assert!(rows[1][1] != "0" && rows[1][1] == rows[2][1], "{rows:?}");
    assert_ne!(rows[1][2], rows[2][2], "{rows:?}");
}

#[test]
fn leaving_keeps_the_split_and_split_attaches_again() {
    if !has_tmux() {
        return;
    }
    let w = World::new();
    ok(&w.split(&["--detached", "shell", "+", "shell"]));
    let before = w.panes();
    w.attach_and_leave();
    assert_eq!(w.panes(), before);
    for p in &before {
        assert_eq!(w.show(p, "#{pane_dead}"), "0");
    }
    w.attach_and_leave();
    assert_eq!(w.panes(), before);
    let sessions = w.text(&["list-sessions", "-F", "#{session_name}"]);
    assert_eq!(sessions.lines().filter(|s| *s == "split").count(), 1);
}

#[test]
fn a_sandbox_pane_shows_its_id_and_keeps_running_after_leaving() {
    // CI runs this test in a step that allows bwrap and sets
    // PADDOCK_TEST_BWRAP (review J 2): there a skip fails.
    let bwrap = common::bwrap_works();
    assert!(
        bwrap || std::env::var_os("PADDOCK_TEST_BWRAP").is_none(),
        "PADDOCK_TEST_BWRAP is set, and bwrap does not work"
    );
    if !bwrap || !has_tmux() {
        eprintln!("bwrap or tmux does not work here; skipped");
        return;
    }
    let w = World::new();
    let project = w.root.join("project").display().to_string();
    ok(&w.split(&[
        "--detached",
        "run",
        "fake",
        "--backend",
        "bwrap",
        "--network",
        "none",
        "--env",
        "clean",
        "-y",
        &project,
        "+",
        "shell",
    ]));
    let panes = w.panes();
    assert_eq!(panes.len(), 2);
    let run = &panes[0];
    wait_for("the sandbox id in the label", 90, || {
        !w.show(run, "#{@paddock_id}").is_empty()
    });
    let id = w.show(run, "#{@paddock_id}");
    assert!(id.starts_with("fake-"), "{id}");
    let label = "#{@paddock_side}|#{@paddock_title}|#{@paddock_color}";
    let want = format!("sandbox|sandbox · fake · {id}|{SANDBOX_COLOR}");
    assert_eq!(w.show(run, label), want);
    wait_for("the fake harness", 30, || {
        w.text(&["capture-pane", "-p", "-t", run])
            .contains("fake-ready")
    });
    std::thread::sleep(Duration::from_millis(1500));
    // The harness printed OSC titles: the label stays paddock's.
    assert_eq!(w.show(run, label), want);
    let badge = w.show(run, "#{@paddock_badge}");
    assert!(badge == "running" || badge == "waits for you", "{badge}");
    let border = w.show(run, "#{E:pane-border-format}");
    assert!(
        border.contains(&format!("sandbox · fake · {id}")),
        "{border}"
    );
    // The status line names the side of the pane too (review J 7).
    assert_eq!(
        w.show(run, "#{E:status-left}").trim(),
        format!("paddock split · #[fg={SANDBOX_COLOR}]sandbox#[default]")
    );
    assert!(
        border.contains(SANDBOX_COLOR) && !border.contains("evil"),
        "{border}"
    );
    let ps = |w: &World| ok(&w.paddock().args(["ps", "--json"]).output().unwrap());
    assert!(ps(&w).contains(&id));
    w.attach_and_leave();
    assert!(
        ps(&w).contains(&id),
        "the sandbox ended when the client left"
    );
    assert_eq!(w.show(run, "#{pane_dead}"), "0");
    w.attach_and_leave();
    assert!(ps(&w).contains(&id));
    let ls = ok(&w.split(&["ls"]));
    assert!(ls.contains(&format!("sandbox · fake · {id}")), "{ls}");
}

#[test]
fn a_split_never_touches_the_users_tmux() {
    if !has_tmux() {
        return;
    }
    let w = World::new();
    let o = w.user_tmux(&["new-session", "-d", "-s", "mine", "sleep 600"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let user_sock = std::fs::read_dir(w.root.join("ut"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("user"))
        .find(|p| p.exists())
        .expect("the user's socket");
    let snapshot = |w: &World| {
        let mut parts = Vec::new();
        for args in [
            &["list-sessions", "-F", "#{session_name} #{session_windows}"][..],
            &[
                "list-panes",
                "-a",
                "-F",
                "#{pane_id} #{pane_title} #{pane_dead}",
            ],
            &["show-options", "-g"],
            &["show-options", "-g", "-w"],
            &["show-options", "-s"],
            &["show-hooks", "-g"],
            &["list-keys"],
        ] {
            parts.push(String::from_utf8_lossy(&w.user_tmux(args).stdout).into_owned());
        }
        let mut files: Vec<String> = walk(&w.root.join("ut"));
        files.sort();
        parts.push(files.join("\n"));
        parts
    };
    let before = snapshot(&w);
    // paddock runs in a pane of the user's tmux.
    let tmux_var = format!("{},4242,0", user_sock.display());
    let run = |args: &[&str]| {
        w.paddock()
            .env("TMUX", &tmux_var)
            .env("TMUX_PANE", "%0")
            .arg("split")
            .args(args)
            .output()
            .unwrap()
    };
    ok(&run(&["--detached", "shell"]));
    ok(&run(&["add", "shell"]));
    ok(&run(&["ls"]));
    // Attaching from the user's tmux opens a new terminal window, on
    // paddock's socket.
    let out = ok(&run(&[]));
    assert!(out.contains("new terminal window"), "{out}");
    // The fake terminal runs in the background.
    let read = || std::fs::read_to_string(w.root.join("terminal.log")).unwrap_or_default();
    wait_for("the terminal window", 10, || !read().is_empty());
    let log = read();
    assert!(log.contains(&w.sock().display().to_string()), "{log}");
    assert!(!log.contains(&user_sock.display().to_string()), "{log}");
    assert_eq!(w.panes().len(), 2);
    assert_eq!(snapshot(&w), before, "the user's tmux changed");
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        out.push(p.display().to_string());
        if p.is_dir() {
            out.extend(walk(&p));
        }
    }
    out
}

#[test]
fn without_tmux_the_message_says_how_to_install_it_and_windows_still_work() {
    let w = World::new();
    // PATH holds only the fake shell and the fake terminal.
    let only_bin = |c: &mut Command| {
        c.env("PATH", w.root.join("bin"));
    };
    let mut c = w.paddock();
    only_bin(&mut c);
    let e = err(&c.args(["split", "--detached", "shell"]).output().unwrap());
    assert!(
        e.contains("paddock split needs tmux, and tmux is not on PATH"),
        "{e}"
    );
    assert!(e.contains("Install it: ") && e.contains("tmux"), "{e}");
    assert!(e.contains("paddock split --windows"), "{e}");
    let mut c = w.paddock();
    only_bin(&mut c);
    let out = ok(&c
        .args(["split", "--windows", "shell", "+", "board"])
        .output()
        .unwrap());
    assert_eq!(out.matches("in a new terminal window").count(), 2, "{out}");
    // The fake terminals run in the background: wait for both, in any order.
    let read = || std::fs::read_to_string(w.root.join("terminal.log")).unwrap_or_default();
    wait_for("two terminal windows", 10, || read().lines().count() >= 2);
    let log = read();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines.iter().any(|l| l.ends_with("bin/fakeshell")), "{log}");
    assert!(lines.iter().any(|l| l.ends_with("board --watch")), "{log}");
}

/// The kind and the left edge of each pane of the split's first window.
fn kinds_left(w: &World) -> Vec<(String, String)> {
    w.text(&[
        "list-panes",
        "-t",
        "=split:",
        "-F",
        "#{@paddock_kind} #{pane_left}",
    ])
    .lines()
    .map(|l| {
        let (k, x) = l.split_once(' ').unwrap_or((l, ""));
        (k.to_string(), x.to_string())
    })
    .collect()
}

/// Review J 4: a split started in a wide window and attached from an 80x24
/// terminal lays out again for 80 columns, and a split started from an
/// 80x24 terminal starts at that size: the panes stack, so the header of
/// each pane has the full width.
#[test]
fn review_j_an_80_column_terminal_stacks_the_panes() {
    if !has_tmux() {
        return;
    }
    let w = World::new();
    ok(&w.split(&["--detached", "shell", "+", "board", "+", "shell"]));
    let wide = kinds_left(&w);
    assert_eq!(wide[0], ("board".into(), "0".into()), "{wide:?}");
    assert!(wide[1].1 != "0", "{wide:?}");
    w.attach_sized_and_leave(&[], Some((80, 24)), |w| {
        wait_for("the stacked layout in 80 columns", 20, || {
            w.show("=split:", "#{window_width}") == "80"
                && kinds_left(w).iter().all(|(_, x)| x == "0")
        });
    });
    let _ = w.tmux(&["kill-server"]);
    // A new split from an 80x24 terminal.
    w.attach_sized_and_leave(
        &["shell", "+", "board", "+", "shell"],
        Some((80, 24)),
        |w| {
            let panes = kinds_left(w);
            assert_eq!(panes.len(), 3, "{panes:?}");
            assert!(panes.iter().all(|(_, x)| x == "0"), "{panes:?}");
        },
    );
    // A wide terminal again: the board goes back to the left.
    w.attach_sized_and_leave(&[], Some((200, 50)), |w| {
        wait_for("the board on the left in 200 columns", 20, || {
            let p = kinds_left(w);
            p.first().is_some_and(|(k, x)| k == "board" && x == "0")
                && p.iter().skip(1).all(|(_, x)| x != "0")
        });
    });
}

/// A job file of `paddock split-pane`, private to this user.
fn write_job(w: &World, job: &serde_json::Value) -> PathBuf {
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = w.root.join("run/test.job");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .unwrap();
    f.write_all(job.to_string().as_bytes()).unwrap();
    file
}

/// Review J 5: when paddock cannot label a sandbox pane, the pane would
/// show as a host shell. The helper does not start the program then.
#[test]
fn review_j_a_sandbox_pane_without_its_label_starts_nothing() {
    let w = World::new();
    let marker = w.root.join("started");
    let job = |side: &str| {
        serde_json::json!({
            "side": side,
            "title": format!("{side} · fake · x"),
            "argv": ["/bin/sh", "-c", format!("touch '{}'", marker.display())],
            "cwd": w.root,
            "env": [],
            "keep_tmux": false,
            "kind": "run",
            "watch": null,
            "passthrough": false,
            // A tmux that fails every command: no label.
            "tmux": "/bin/false",
            "sock": w.root.join("run/none.sock"),
        })
    };
    let run = |file: &Path| {
        w.paddock()
            .env("TMUX_PANE", "%1")
            .arg("split-pane")
            .arg(file)
            .output()
            .unwrap()
    };
    let o = run(&write_job(&w, &job("sandbox")));
    let e = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{e}");
    assert!(e.contains("could not label this pane"), "{e}");
    assert!(!marker.exists(), "the program started without its label");
    // A host pane without a label shows as a host shell anyway: it starts.
    let o = run(&write_job(&w, &job("host")));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(marker.exists());
}

/// Review J 6: with --windows, no tmux border shows the label. Each window
/// prints it first, the side as a word.
#[test]
fn review_j_each_window_prints_its_label_first() {
    let w = World::new();
    let mut c = w.paddock();
    c.env("PATH", w.root.join("bin"));
    let out = ok(&c
        .args(["split", "--windows", "run", "fake", "+", "shell"])
        .output()
        .unwrap());
    assert_eq!(out.matches("in a new terminal window").count(), 2, "{out}");
    // The fake terminals run in the background, in any order.
    let read = || std::fs::read_to_string(w.root.join("terminal.log")).unwrap_or_default();
    wait_for("two terminal windows", 10, || read().lines().count() >= 2);
    let log = read();
    let line = |end: &str| {
        log.lines()
            .find(|l| l.ends_with(end))
            .unwrap_or_else(|| panic!("no window runs {end}: {log}"))
            .to_string()
    };
    let run = line("run fake");
    assert!(
        run.contains("paddock: sandbox · fake · starting") && run.contains("159;184;160"),
        "{log}"
    );
    let shell = line("bin/fakeshell");
    assert!(
        shell.contains("paddock: host · shell · ") && shell.contains("224;163;58"),
        "{log}"
    );
}
