//! `paddock split`: several terminals in one window, on paddock's own tmux
//! server (src/view/split.rs). `paddock split-pane` is the helper that
//! each pane runs.

use anyhow::{bail, Context, Result};
use clap::Args;
use paddock::env::HostEnv;
use paddock::view::split::{self, Job, Kind, Pane, Side, Watch};
use paddock::view::Tmux;
use std::ffi::OsString;
use std::io::{BufRead as _, IsTerminal};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How often the helper of a sandbox pane reads the state of its run.
const BADGE_EVERY: Duration = Duration::from_secs(1);

pub(crate) const HELP: &str = "Panes (a lone + separates two panes):
  run HARNESS [ARGS]   a new sandbox run, as `paddock HARNESS [ARGS]`
  view ID [--write]    an existing run, read-only unless --write
  shell                a host shell in the current folder
  board                paddock board --watch
  mac [APP]            paddock mac view (macOS)
Without panes: run of the default harness above, shell below.

Examples:
  paddock split                          start the split, or attach to it again
  paddock split run claude + shell       Claude Code above, a host shell below
  paddock split run codex + board + shell
  paddock split add view 1a2b3c          add a pane (inside the split or outside)
  paddock split ls                       list the panes
  paddock split --windows run claude + shell
                                         each pane in its own window, without tmux

The border of each pane says where it runs, and paddock sets it, not the
program in the pane: `sandbox · claude · ID` in sage for a sandbox,
`host · shell · FOLDER` in amber for the host. A pane that you open with
tmux keys shows as a host shell.

Keys: Ctrl-B o next pane · Ctrl-B z zoom · Ctrl-B d leave. The mouse
focuses a pane. Leaving keeps the sandboxes running: `paddock split`
attaches again. The split runs on paddock's own tmux server, never on
yours, and outside tmux it uses the current terminal. Inside tmux, it
opens a new terminal window. `paddock help split-guide` explains more.";

#[derive(Args)]
pub(crate) struct SplitArgs {
    /// Open each pane in its own terminal window, without tmux.
    #[arg(long)]
    windows: bool,
    /// Start the split and do not attach a terminal to it.
    #[arg(long)]
    detached: bool,
    /// With `ls`: print JSON.
    #[arg(long)]
    json: bool,
    /// The panes, `add KIND [ARGS]` or `ls`. Flags of paddock split go
    /// before them.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "PANES"
    )]
    words: Vec<String>,
}

pub(crate) fn run(env: &HostEnv, a: SplitArgs) -> Result<i32> {
    match a.words.first().map(String::as_str) {
        Some("ls") => {
            let json = a.json || a.words[1..].iter().any(|w| w == "--json");
            if let Some(w) = a.words[1..].iter().find(|w| *w != "--json") {
                bail!("paddock split ls takes no {w}");
            }
            ls(env, json)
        }
        Some("add") => add(env, &a.words[1..]),
        _ => open(env, &a),
    }
}

/// The message without tmux.
pub(crate) fn no_tmux() -> String {
    let install = if cfg!(target_os = "macos") {
        "brew install tmux".to_string()
    } else {
        paddock::doctor::install_command(&["tmux"])
    };
    format!(
        "paddock split needs tmux, and tmux is not on PATH. Install it: {install}\n\
         Or open each pane in its own terminal window: paddock split --windows [PANES]"
    )
}

fn open(env: &HostEnv, a: &SplitArgs) -> Result<i32> {
    let cwd = std::env::current_dir().context("cannot read the current folder")?;
    let panes = || {
        if a.words.is_empty() {
            default_panes(env, &cwd)
        } else {
            split::parse(&a.words)
        }
    };
    let tmux = Tmux::find(env);
    if a.windows {
        return windows(env, tmux.as_ref(), &panes()?, &cwd);
    }
    let Some(t) = tmux else {
        // The panes first: a wrong pane is the first thing to fix.
        panes()?;
        bail!("{}", no_tmux());
    };
    if t.has(split::SESSION) {
        if !a.words.is_empty() {
            bail!(
                "a split runs already. `paddock split` without panes attaches to it, and `paddock split add KIND` adds a pane"
            );
        }
    } else {
        let me = std::env::current_exe().context("cannot find the paddock executable")?;
        let jobs = panes()?
            .iter()
            .map(|p| job(env, &t, p, &cwd, &me))
            .collect::<Result<Vec<_>>>()?;
        // This terminal attaches: the split starts at its size, so the
        // layout fits it (the layout hook keeps it fitting after that).
        let here = !a.detached && env.var("TMUX").filter(|v| !v.is_empty()).is_none();
        let size = here.then(term_size).flatten();
        let missing = split::start(&t, env, &me, &jobs, size)?;
        warn_unlabeled(&missing);
    }
    if a.detached {
        println!(
            "paddock split: the split runs. `paddock split` attaches to it, `paddock split ls` lists its panes."
        );
        return Ok(0);
    }
    attach(env, &t)
}

/// The columns and lines of the terminal on stdout, without the status line.
fn term_size() -> Option<(u32, u32)> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    // SAFETY: TIOCGWINSZ writes only into `ws`.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return None;
    }
    let (x, y) = (u32::from(ws.ws_col), u32::from(ws.ws_row));
    (x > 0 && y > 1).then_some((x, y - 1))
}

/// Panes that paddock added and that have no label show as host shells.
fn warn_unlabeled(missing: &[String]) {
    if !missing.is_empty() {
        eprintln!(
            "paddock split: pane {} has no label yet, so it shows as a host shell. A sandbox pane starts its program only once it has its label.",
            missing.join(", ")
        );
    }
}

/// `run DEFAULT` above, `shell` below.
fn default_panes(env: &HostEnv, cwd: &Path) -> Result<Vec<Pane>> {
    let project = paddock::build::resolve_project(Some(cwd))?;
    let loaded = paddock::manifest::Manifest::load(env, &project)?;
    let Some(h) = loaded.merged.default_harness else {
        bail!(
            "name the panes, for example `paddock split run claude + shell`, or set default_harness in paddock.toml"
        );
    };
    Ok(vec![
        Pane {
            kind: Kind::Run,
            args: vec![h],
        },
        Pane {
            kind: Kind::Shell,
            args: vec![],
        },
    ])
}

/// Attach this terminal, or a new terminal window, to the split.
fn attach(env: &HostEnv, t: &Tmux) -> Result<i32> {
    let argv = split::attach_argv(t);
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let tmux_var = env.var("TMUX").filter(|v| !v.is_empty());
    // In tmux, a client in the pane would answer to two Ctrl-B prefixes.
    if tty && tmux_var.is_none() {
        let e = Command::new(&argv[0]).args(&argv[1..]).exec();
        bail!("cannot start tmux: {e}");
    }
    match new_window(env, &argv, None) {
        Ok(()) => {
            println!("paddock split: opened in a new terminal window.");
            Ok(0)
        }
        // Not in paddock's own tmux: the split shows in this pane of the
        // user's tmux, with the user's prefix in front of each key.
        Err(_) if tty && !split::in_paddock_tmux(t, tmux_var) => {
            eprintln!("paddock split: no terminal window opens, so the split shows in this pane. Press Ctrl-B twice for its keys.");
            let e = Command::new(&argv[0])
                .args(&argv[1..])
                .env_remove("TMUX")
                .env_remove("TMUX_PANE")
                .exec();
            bail!("cannot start tmux: {e}");
        }
        Err(e) => Err(e.context(format!(
            "cannot open a terminal window for the split. Attach from a terminal: {}",
            argv.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        ))),
    }
}

/// Start `argv` in a new terminal window, in folder `cwd`.
fn new_window(env: &HostEnv, argv: &[OsString], cwd: Option<&Path>) -> Result<()> {
    let mut full = paddock::onboard::terminal_command(
        env.which("xdg-terminal-exec").as_deref(),
        env.var("TERMINAL").filter(|t| !t.trim().is_empty()),
        paddock::onboard::Os::HOST,
    )?;
    full.extend(argv.iter().cloned());
    let mut c = Command::new(&full[0]);
    c.args(&full[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c.spawn()
        .map(paddock::reap::in_background)
        .with_context(|| format!("cannot start {}", full[0].to_string_lossy()))
}

/// `--windows`: each pane in its own terminal window.
fn windows(env: &HostEnv, t: Option<&Tmux>, panes: &[Pane], cwd: &Path) -> Result<i32> {
    let me = std::env::current_exe().context("cannot find the paddock executable")?;
    let jobs = panes
        .iter()
        .map(|p| job_with(env, t, p, cwd, &me))
        .collect::<Result<Vec<_>>>()?;
    for j in &jobs {
        new_window(env, &window_argv(j), Some(cwd))?;
        println!(
            "paddock split: opened {} in a new terminal window.",
            j.title
        );
    }
    Ok(0)
}

/// The shell that prints the label of a window before its program.
const WINDOW_HEADER: &str =
    "printf '\\033[1;38;2;%sm%s\\033[0m\\n' \"$1\" \"$2\"; shift 2; exec \"$@\"";

/// A window of `--windows`: a header line with the label, in the color of
/// its side, then the program. No tmux border shows the label there.
fn window_argv(j: &Job) -> Vec<OsString> {
    let hex = j.side.color().trim_start_matches('#');
    let rgb: Vec<String> = (0..3)
        .map(|i| {
            u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
                .unwrap_or(255)
                .to_string()
        })
        .collect();
    ["/bin/sh", "-c", WINDOW_HEADER, "paddock-split"]
        .iter()
        .map(OsString::from)
        .chain([
            OsString::from(rgb.join(";")),
            OsString::from(format!("paddock: {}", split::clean(&j.title))),
        ])
        .chain(j.argv.iter().map(OsString::from))
        .collect()
}

fn add(env: &HostEnv, words: &[String]) -> Result<i32> {
    let pane = split::parse_one(words)?;
    let Some(t) = Tmux::find(env) else {
        bail!("{}", no_tmux());
    };
    if !t.has(split::SESSION) {
        bail!("no split runs. Start one with `paddock split`");
    }
    let cwd = std::env::current_dir().context("cannot read the current folder")?;
    let me = std::env::current_exe().context("cannot find the paddock executable")?;
    let j = job(env, &t, &pane, &cwd, &me)?;
    let target = split::caller_pane(&t, env.var("TMUX"), env.var("TMUX_PANE"));
    let (id, missing) = split::add(&t, env, &me, &j, target.as_deref())?;
    println!("paddock split: added pane {id}: {}", j.title);
    warn_unlabeled(&missing);
    Ok(0)
}

fn ls(env: &HostEnv, json: bool) -> Result<i32> {
    let panes = Tmux::find(env)
        .filter(|t| t.has(split::SESSION))
        .map(|t| split::list(&t))
        .unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&panes)?);
        return Ok(0);
    }
    if panes.is_empty() {
        println!("No split runs. `paddock split` starts one.");
        return Ok(0);
    }
    println!("  {:<6} {:<5} {:<14} TITLE", "PANE", "ID", "STATE");
    for p in &panes {
        let state = match (&p.badge, p.dead) {
            (Some(b), _) => b.as_str(),
            (None, true) => "ended",
            (None, false) => "running",
        };
        let mark = if p.active { '*' } else { ' ' };
        println!("{mark} {:<6} {:<5} {:<14} {}", p.pane, p.id, state, p.title);
    }
    Ok(0)
}

fn job(env: &HostEnv, t: &Tmux, p: &Pane, cwd: &Path, me: &Path) -> Result<Job> {
    job_with(env, Some(t), p, cwd, me)
}

/// What a pane runs, its label and its environment.
fn job_with(env: &HostEnv, t: Option<&Tmux>, p: &Pane, cwd: &Path, me: &Path) -> Result<Job> {
    let me_s = me.to_string_lossy().into_owned();
    let mut j = Job {
        side: Side::Host,
        title: String::new(),
        argv: vec![],
        cwd: cwd.to_path_buf(),
        env: paddock::detach::caller_env(),
        keep_tmux: true,
        kind: p.kind,
        watch: None,
        passthrough: false,
        tmux: t.map(|t| t.exe.clone()).unwrap_or_default(),
        sock: t.map(|t| t.sock.clone()).unwrap_or_default(),
    };
    match p.kind {
        Kind::Run => {
            let harness = p.args[0].clone();
            j.side = Side::Sandbox;
            j.title = split::title(Side::Sandbox, &harness, "starting");
            j.argv = [me_s, "run".into()]
                .into_iter()
                .chain(p.args.clone())
                .collect();
            // The sandbox gets no TMUX anyway (policy.rs); the run needs none.
            j.keep_tmux = false;
            j.watch = Some(Watch {
                harness,
                id: None,
                discover: true,
            });
        }
        Kind::View => {
            let write = p.args.iter().any(|a| a == "--write");
            let raw = p
                .args
                .iter()
                .find(|a| *a != "--write")
                .expect("parse checked");
            let id = paddock::ids::resolve(env, raw)?;
            let running = paddock::state::Running::list(env);
            let me_run = running.iter().find(|r| r.id == id);
            if write && me_run.is_some_and(|r| r.relay) {
                bail!("a view pane with --write is not offered for {id}. A background process started this run and owns the input");
            }
            let harness = me_run
                .map(|r| r.harness.clone())
                .or_else(|| paddock::detach::Record::load(env, &id).map(|r| r.harness))
                .unwrap_or_else(|| "run".into());
            j.side = Side::Sandbox;
            let detail = if write {
                id.clone()
            } else {
                format!("{id} · read-only")
            };
            j.title = split::title(Side::Sandbox, &harness, &detail);
            j.argv = view_argv(env, t, &id, write)?;
            j.keep_tmux = false;
            j.watch = Some(Watch {
                harness,
                id: Some(id),
                discover: false,
            });
        }
        Kind::Shell => {
            let shell = env
                .var("SHELL")
                .filter(|s| Path::new(s).is_absolute())
                .unwrap_or("/bin/sh");
            j.title = split::title(Side::Host, "shell", &split::folder(&env.home, cwd));
            j.argv = vec![shell.into()];
        }
        Kind::Board => {
            j.title = split::title(Side::Host, "board", "every project");
            j.argv = vec![me_s, "board".into(), "--watch".into()];
        }
        Kind::Mac => {
            let target = p.args.first().cloned();
            j.side = Side::Sandbox;
            j.title = split::title(Side::Sandbox, "mac", target.as_deref().unwrap_or("desktop"));
            j.argv = vec![me_s, "mac".into(), "view".into()];
            if let Some(app) = target {
                j.argv.extend(["--app".into(), app]);
            }
            j.passthrough = true;
            j.watch = Some(Watch {
                harness: "mac".into(),
                id: None,
                discover: false,
            });
        }
    }
    Ok(j)
}

/// A view pane: a read-only tmux client of the run's session, or its log.
fn view_argv(env: &HostEnv, t: Option<&Tmux>, id: &str, write: bool) -> Result<Vec<String>> {
    if let Some(t) = t.filter(|t| t.has(id)) {
        let mut v = vec![
            t.exe.to_string_lossy().into_owned(),
            "-S".into(),
            t.sock.to_string_lossy().into_owned(),
            "attach-session".into(),
        ];
        if !write {
            v.push("-r".into());
        }
        v.extend(["-t".into(), format!("={id}")]);
        return Ok(v);
    }
    let log = paddock::view::log_file(env, id);
    if log.is_file() {
        return Ok(vec![
            "tail".into(),
            "-n".into(),
            "+1".into(),
            "-f".into(),
            "--".into(),
            log.to_string_lossy().into_owned(),
        ]);
    }
    bail!("sandbox {id} has no view: it has no tmux session and no run log. A sandbox started from a terminal has no view, it uses that terminal")
}

/// The helper in each pane: label the pane, then run its program. Before
/// HostEnv::detect: the pane gets the caller's environment from the job.
pub(crate) fn pane_main(file: &Path) -> Result<i32> {
    let job = split::take_job(file)?;
    let pane = std::env::var("TMUX_PANE")
        .ok()
        .filter(|p| p.starts_with('%'));
    let tmux_out = |args: Vec<String>| -> Option<String> {
        if job.tmux.as_os_str().is_empty() {
            return None;
        }
        let out = Command::new(&job.tmux)
            .arg("-S")
            .arg(&job.sock)
            .args(["-f", "/dev/null"])
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let tmux = |args: Vec<String>| {
        let _ = tmux_out(args);
    };
    let labelled = pane.as_ref().is_some_and(|p| {
        tmux_out(split::label_args(p, &job)).is_some()
            && tmux_out(split::side_args(p)).is_some_and(|o| o.trim() == job.side.word())
    });
    // Without its label, a sandbox pane shows as a host shell: its program
    // does not start.
    if job.side == Side::Sandbox && !labelled {
        eprintln!(
            "paddock split: paddock could not label this pane as a sandbox pane, so it would show as a host shell. {} does not start. Press Enter to close the pane.",
            job.argv.first().map(String::as_str).unwrap_or("The program")
        );
        let _ = std::io::stdin().lock().read_line(&mut String::new());
        return Ok(1);
    }
    // The caller's environment, with the terminal of the pane.
    let pane_vars: Vec<(String, String)> = ["TERM", "TMUX", "TMUX_PANE"]
        .iter()
        .filter(|k| **k == "TERM" || job.keep_tmux)
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    for (k, _) in std::env::vars_os() {
        std::env::remove_var(k);
    }
    for (k, v) in job.env.iter().chain(pane_vars.iter()) {
        if !k.is_empty() && !k.contains('=') && !v.contains('\0') {
            std::env::set_var(k, v);
        }
    }
    if let Err(e) = std::env::set_current_dir(&job.cwd) {
        eprintln!("paddock split: cannot enter {}: {e}", job.cwd.display());
        return Ok(1);
    }
    let Some((prog, args)) = job.argv.split_first() else {
        bail!("the pane has no program");
    };
    let mut cmd = Command::new(prog);
    cmd.args(args);
    let (Some(watch), Some(pane)) = (&job.watch, &pane) else {
        let e = cmd.exec();
        eprintln!("paddock split: cannot start {prog}: {e}");
        return Ok(127);
    };
    // A sandbox pane: paddock stays to keep the badge current and to put
    // the sandbox id of a run in the label. Ctrl-C goes to the program,
    // not to this helper.
    // SAFETY: signal() changes only the disposition of these signals.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        cmd.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot start {prog}"))?;
    let env = HostEnv::detect()?;
    let mut id = watch.id.clone();
    let mut shown = "";
    let mut next = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if std::time::Instant::now() >= next {
            next += BADGE_EVERY;
            if id.is_none() && watch.discover {
                id = paddock::state::Running::list(&env)
                    .into_iter()
                    .find(|r| split::descends(r.host_pid, child.id()))
                    .map(|r| r.id);
                if let Some(i) = &id {
                    tmux(split::id_args(pane, &watch.harness, i));
                }
            }
            let badge = match &id {
                Some(i) => split::badge(paddock::board::collect::live_state(
                    &env,
                    i,
                    paddock::state::now(),
                )),
                // A run before its sandbox starts, or the mac view.
                None if watch.discover => "starting",
                None => "running",
            };
            if badge != shown {
                tmux(split::badge_args(pane, badge));
                shown = badge;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    tmux(split::badge_args(pane, "ended"));
    let code = status.code().unwrap_or(1);
    let what = if watch.discover { "run" } else { "view" };
    eprintln!(
        "\npaddock split: the {what} ended with exit code {code}. Ctrl-B x closes this pane."
    );
    Ok(code)
}

/// The layout hook of the split (`paddock split-layout TMUX SOCK`): tmux
/// runs it when a terminal attaches or changes its size. It lays out the
/// split again for the size of the window now.
pub(crate) fn layout_main(tmux: &Path, sock: &Path) -> Result<i32> {
    let t = Tmux::at(tmux.to_path_buf(), sock.to_path_buf());
    split::relayout_all(&t);
    Ok(0)
}
