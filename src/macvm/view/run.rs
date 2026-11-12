//! The terminal side of the view: raw mode on the TTY, the probe, the
//! loop of frames, input and size changes, and the way out that gives
//! the terminal back as it was, also after SIGTERM, SIGHUP or a panic.
//! Unix only.

use super::diff;
use super::fit::Term;
use super::map::Action;
use super::screen::Screen;
use super::source::{FrameSource, InputSink, Item, Target};
use super::stream::Stream;
use super::term::{self, Choice, Env, Protocol, Tmux};
use crate::macvm::text;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

/// The options of `paddock mac view`.
#[derive(Debug, Clone)]
pub struct Options {
    pub target: Target,
    pub max_fps: u32,
    pub protocol: Option<Protocol>,
    /// The command that shows the view again, for the last line.
    pub again: String,
}

/// How long the probe waits for its answers. In tmux, a terminal without
/// the kitty protocol takes all of it (see `term::probe_complete`).
const PROBE_WAIT: Duration = Duration::from_millis(1000);
/// A lone ESC with no byte after it for this long is the Escape key.
const ESC_WAIT: Duration = Duration::from_millis(40);
/// The loop wakes at least this often.
const TICK: Duration = Duration::from_millis(10);
/// The size of the terminal is read this often.
const SIZE_EVERY: Duration = Duration::from_millis(100);

/// The modes of the view, off, for a drop after a panic.
const RESET: &[u8] =
    b"\x1b[?1016l\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[<u\x1b[0m\x1b[?25h\x1b[?1049l";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// The TTY in raw mode; dropping it restores the mode.
struct Tty {
    fd: OwnedFd,
    saved: libc::termios,
    /// The view modes are on: a drop turns them off.
    entered: std::cell::Cell<bool>,
}

impl Tty {
    fn open() -> io::Result<Tty> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")?;
        let fd: OwnedFd = f.into();
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut saved) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd.as_raw_fd(), libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Tty {
            fd,
            saved,
            entered: std::cell::Cell::new(false),
        })
    }

    fn write(&self, mut b: &[u8]) -> io::Result<()> {
        while !b.is_empty() {
            let n = unsafe { libc::write(self.fd.as_raw_fd(), b.as_ptr().cast(), b.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                    continue;
                }
                return Err(e);
            }
            b = &b[n as usize..];
        }
        Ok(())
    }

    /// Read what is there within `wait`, or nothing.
    fn read(&self, wait: Duration) -> io::Result<Vec<u8>> {
        let mut p = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = wait.as_millis().min(i32::MAX as u128) as i32;
        let r = unsafe { libc::poll(&mut p, 1, ms) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(Vec::new());
            }
            return Err(e);
        }
        if r == 0 || p.revents & libc::POLLIN == 0 {
            if p.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the terminal closed",
                ));
            }
            return Ok(Vec::new());
        }
        let mut buf = [0u8; 4096];
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                return Ok(Vec::new());
            }
            return Err(e);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the terminal closed",
            ));
        }
        Ok(buf[..n as usize].to_vec())
    }

    fn size(&self, cell: Option<(u16, u16)>) -> Term {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
        Term::new(
            ws.ws_col,
            ws.ws_row,
            Some((ws.ws_xpixel, ws.ws_ypixel)),
            cell,
        )
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        if self.entered.get() {
            let _ = self.write(RESET);
        }
        unsafe { libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSANOW, &self.saved) };
    }
}

fn tmux_facts() -> Option<Tmux> {
    let out = |args: &[&str]| -> String {
        std::process::Command::new("tmux")
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    Some(Tmux {
        version: out(&["display", "-p", "#{version}"]),
        passthrough: out(&["show", "-Av", "allow-passthrough"]),
        client: out(&["display", "-p", "#{client_termname} #{client_termtype}"]),
    })
}

/// Ask the terminal. The TTY is in raw mode.
fn probe(tty: &Tty, wrap: bool) -> io::Result<term::Replies> {
    tty.write(&term::probe(wrap))?;
    term::collect_replies(wrap, PROBE_WAIT, |d| tty.read(d))
}

/// Detect the protocol without the view: for `paddock mac view --probe`
/// and the start of the view. The TTY must be in raw mode.
fn detect(tty: &Tty, forced: Option<Protocol>) -> io::Result<(Choice, term::Replies, Env)> {
    let env = Env::from_vars(|k| std::env::var(k).ok());
    let tmux = env.tmux.then(tmux_facts).flatten();
    let wrap = tmux.as_ref().is_some_and(|t| t.passes());
    let replies = probe(tty, wrap)?;
    Ok((
        term::choose(&env, tmux.as_ref(), &replies, forced),
        replies,
        env,
    ))
}

/// What the probe found, as lines for `--probe`, and true when a
/// protocol works.
pub fn probe_report(forced: Option<Protocol>) -> anyhow::Result<(Vec<String>, bool)> {
    let tty = Tty::open().map_err(|_| anyhow::anyhow!(text::view_not_a_terminal()))?;
    let (choice, r, env) = detect(&tty, forced)?;
    let size = tty.size(r.cell);
    drop(tty);
    let mut lines = text::view_probe_lines(&text::ViewProbe {
        protocol: choice.protocol.map_or("none", |p| p.name()),
        tmux_wrap: choice.wrap,
        kitty: r.kitty,
        sixel: r.da1.as_ref().is_some_and(|p| p.contains(&4)),
        kitty_keys: r.kitty_keys,
        mouse_pixels: r.mouse_pixels,
        cols: size.cols,
        rows: size.rows,
        cell_w: size.cell_w,
        cell_h: size.cell_h,
        guessed: size.guessed,
        term: &env.term,
        term_program: &env.term_program,
    });
    lines.extend(choice.notes);
    Ok((lines, choice.protocol.is_some()))
}

/// Run the view until the user leaves it, the stream ends or a signal
/// comes. Returns the exit code: 0 when the user left.
pub fn run(
    opts: &Options,
    source: &mut dyn FrameSource,
    sink: Box<dyn InputSink + Send>,
    fallback: &dyn Fn(&[String]) -> anyhow::Result<i32>,
) -> anyhow::Result<i32> {
    use std::io::IsTerminal;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!(text::view_not_a_terminal());
    }
    let tty = Tty::open()?;
    let (choice, replies, _) = detect(&tty, opts.protocol)?;
    let Some(protocol) = choice.protocol else {
        drop(tty);
        return fallback(&choice.notes);
    };
    let term = tty.size(replies.cell);
    if term.rows < 2 {
        anyhow::bail!(text::view_too_small());
    }
    let label = opts.target.label();
    let mut screen = Screen::new(protocol, choice.wrap, term, &label);
    screen.mouse_pixels = replies.mouse_pixels;
    screen.kitty_keys = replies.kitty_keys;
    if let Some(n) = choice.notes.first() {
        screen.note = n.clone();
    } else if term.guessed {
        screen.note = "the terminal reports no cell size, so the mouse is approximate".into();
    }
    STOP.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
    }
    let mut stream = match Stream::start(source, &opts.target, opts.max_fps, screen.max()) {
        Ok(s) => s,
        Err(e) => {
            drop(tty);
            anyhow::bail!(e);
        }
    };
    // The input goes to the guest on its own thread, so a slow guest
    // does not stop the picture. Errors come back to the status line.
    let (act_tx, act_rx) = mpsc::channel::<Action>();
    let (err_tx, err_rx) = mpsc::channel::<String>();
    let mut sink = sink;
    std::thread::spawn(move || {
        for a in act_rx {
            if let Err(e) = sink.send(&a) {
                if err_tx.send(e).is_err() {
                    return;
                }
            }
        }
    });

    tty.entered.set(true);
    tty.write(&screen.enter())?;
    if !replies.rest.is_empty() {
        let step = screen.input(&replies.rest, 0);
        for a in step.actions {
            let _ = act_tx.send(a);
        }
    }
    let start = Instant::now();
    let mut last_size = Instant::now();
    let mut last_byte = Instant::now();
    let mut ended: Option<String> = None;
    let result = (|| -> anyhow::Result<()> {
        loop {
            if STOP.load(Ordering::SeqCst) {
                return Ok(());
            }
            let now = start.elapsed().as_millis() as u64;
            let bytes = tty.read(TICK)?;
            let step = if !bytes.is_empty() {
                last_byte = Instant::now();
                screen.input(&bytes, now)
            } else if screen.pending() && last_byte.elapsed() >= ESC_WAIT {
                screen.idle(now)
            } else {
                Default::default()
            };
            tty.write(&step.bytes)?;
            for a in step.actions {
                let _ = act_tx.send(a);
            }
            if step.leave {
                return Ok(());
            }
            let mut waiting = Vec::new();
            loop {
                match stream.frames().try_recv() {
                    Ok(Item::Frame(f)) => waiting.push(f),
                    Ok(Item::End(why)) => {
                        ended = Some(why);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        ended = Some("the stream closed".into());
                        break;
                    }
                }
            }
            if let Some((f, _dropped)) = diff::newest(waiting, |f| f.seq) {
                tty.write(&screen.frame(f))?;
            }
            if ended.is_some() {
                return Ok(());
            }
            while let Ok(e) = err_rx.try_recv() {
                tty.write(&screen.action_failed(&e))?;
            }
            if last_size.elapsed() >= SIZE_EVERY {
                last_size = Instant::now();
                tty.write(&screen.resize(tty.size(replies.cell)))?;
                // A new room gets frames of its size; their seq starts at 1.
                match stream.room(source, screen.max(), now) {
                    Ok(true) => screen.subscribed(),
                    Ok(false) => {}
                    Err(e) => tty.write(&screen.say(text::view_resize_failed(&e)))?,
                }
            }
        }
    })();
    source.stop();
    let _ = tty.write(&screen.leave());
    tty.entered.set(false);
    drop(tty);
    result?;
    if let Some(why) = ended {
        eprintln!(
            "{}",
            text::view_stream_ended(&label, &super::screen::clean(&why, 300))
        );
        return Ok(1);
    }
    eprintln!("{}", text::view_left(&label, &opts.again));
    Ok(0)
}
