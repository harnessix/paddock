//! `paddock board`: every project, ticket and agent run at a glance.

use anyhow::Result;
use paddock::board;
use paddock::env::HostEnv;
use std::io::{IsTerminal, Write};
use std::time::Duration;

#[derive(clap::Args, Debug)]
#[command(after_help = "\
States, the most urgent first: waiting (a card or a question waits for you), \
blocked, stuck, working, idle, open (a ticket without a run), done, ended.\n\n\
A run belongs to a ticket when its prompt file is .paddock/tickets/NAME.md of \
the project. `paddock help board-guide` explains the rules.")]
pub(crate) struct BoardArgs {
    /// Print JSON (version 1), the data of the Omarchy panel.
    #[arg(long)]
    json: bool,
    /// Draw the board again when it changes. With --json, print one line
    /// of JSON for each change. Ctrl-C ends it.
    #[arg(long)]
    watch: bool,
}

/// How often --watch reads the state.
const POLL: Duration = Duration::from_secs(2);

pub(crate) fn run(env: &HostEnv, a: BoardArgs) -> Result<i32> {
    let mut last = String::new();
    loop {
        let snap = board::collect(env, paddock::state::now());
        let shown = if a.json {
            board::to_json(&snap.board, !a.watch)?
        } else {
            let tty = std::io::stdout().is_terminal();
            let ansi = tty && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
            board::text::render(&snap, width(tty), ansi)
        };
        if shown != last {
            let mut out = std::io::stdout().lock();
            let wrote = if a.json {
                writeln!(out, "{shown}")
            } else if a.watch && out.is_terminal() {
                write!(out, "\x1b[H\x1b[2J{shown}")
            } else if a.watch {
                writeln!(out, "{shown}")
            } else {
                write!(out, "{shown}")
            };
            // The reader went away: there is nobody to show the board to.
            if wrote.and_then(|()| out.flush()).is_err() || !a.watch {
                return Ok(0);
            }
            last = shown;
        }
        // A watch writes only on a change: it also ends when its reader
        // goes away between changes, so no watch outlives its panel.
        if reader_gone() {
            return Ok(0);
        }
        std::thread::sleep(POLL);
    }
}

/// The read end of the stdout pipe is closed.
fn reader_gone() -> bool {
    let mut p = libc::pollfd {
        fd: libc::STDOUT_FILENO,
        events: 0,
        revents: 0,
    };
    // SAFETY: poll on one local pollfd, without waiting.
    let n = unsafe { libc::poll(&mut p, 1, 0) };
    n > 0 && p.revents & (libc::POLLERR | libc::POLLHUP) != 0
}

/// The terminal width, or `COLUMNS`. None: lines stay whole.
fn width(tty: bool) -> Option<usize> {
    if tty {
        // SAFETY: TIOCGWINSZ writes into a local winsize.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_col > 0
        {
            return Some(size.ws_col as usize);
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse().ok())
        .filter(|w: &usize| *w > 0)
}
