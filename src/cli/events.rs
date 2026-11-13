//! `paddock events --follow [--json]`: the live state of app mode for the
//! Mac launcher, one line per event (`paddock::eventstream`).

use anyhow::{bail, Result};
use paddock::env::HostEnv;
use paddock::eventstream::{follow, Bus};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(clap::Args, Debug, Clone)]
pub(crate) struct EventsArgs {
    /// Keep printing events until the reader goes or a signal comes.
    #[arg(long)]
    follow: bool,
    /// One JSON object per line, with the schema version in `v`. A call of
    /// a device or a tool in /dev/paddock is a line of kind `device_call`
    /// with `run`, `device` and `ask` (true when the call waits for an
    /// approval). It never holds the request or the response.
    #[arg(long)]
    json: bool,
    /// The reader shows the `notice` lines as desktop notifications (paddock
    /// for Mac). While it follows, paddock on macOS does not post its own.
    #[arg(long)]
    notify: bool,
}

/// Set by SIGTERM, SIGINT and SIGHUP: the follower removes its socket and
/// ends.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

pub(crate) fn run(env: &HostEnv, a: EventsArgs) -> Result<i32> {
    if !a.follow {
        bail!("paddock events needs --follow: the stream has no history. Run `paddock events --follow --json`.");
    }
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores into an atomic, which is
        // async-signal-safe.
        unsafe {
            libc::signal(
                sig,
                on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
            )
        };
    }
    let opts = follow::Opts {
        json: a.json,
        notify: a.notify,
        ..Default::default()
    };
    let mut out = std::io::stdout().lock();
    follow::follow(
        &Bus::of(env),
        &mut out,
        &|| STOP.load(Ordering::SeqCst) || follow::reader_gone(libc::STDOUT_FILENO),
        &|| follow::disk_now(env),
        opts,
    )?;
    Ok(0)
}
