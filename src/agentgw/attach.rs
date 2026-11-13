//! `paddock openclaw tui`: the real client of the harness, in a small
//! sandbox of its own, attached to the sandboxed gateway.
//!
//! The client gets the URL and the token from paddock, the home of the
//! clients, and of the host loopback only the port of the gateway. With
//! no gateway running, paddock offers to start one.

use super::shape::{Shape, Values};
use super::state::{self, Phase, State};
use super::{control, launch, spec};
use crate::env::HostEnv;
use crate::profile::{Harness, HarnessFactory};
use anyhow::{bail, Context as _, Result};
use std::io::{BufRead as _, IsTerminal as _, Write as _};

/// The port of a gateway that answers on the host and that paddock did not
/// start: the `host_port` of the profile, when something listens there.
pub fn host_gateway(env: &HostEnv, name: &str, shape: &Shape) -> Option<u16> {
    let p = shape.host_port?;
    let ours = state::live(env, name).map(|s| s.port);
    (ours != Some(p) && launch::answers(p)).then_some(p)
}

/// The notice for a gateway on the host that paddock did not start: it
/// runs without a sandbox, and how to move it into one.
pub fn host_notice(name: &str, why: Option<&str>, shape: &Shape, port: u16) -> String {
    let started_by = shape
        .host_commands
        .first()
        .map(|c| format!(" (`{name} {c}` starts it)"))
        .unwrap_or_default();
    let mut s = String::new();
    if let Some(why) = why {
        s.push_str(&format!("{why}. "));
    }
    s.push_str(&format!(
        "A {name} gateway that paddock did not start answers on 127.0.0.1:{port}{started_by}: it runs on the host without a sandbox. To move to the sandboxed gateway, stop that one, then run `paddock gateway {name} start`. `paddock {name} tui` then attaches to it"
    ));
    s
}

/// Ask on the terminal. False without one.
fn ask(question: &str) -> bool {
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return false;
    }
    eprint!("{question} [Y/n] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    let a = line.trim().to_ascii_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

/// The running gateway, or a new one when the user agrees (or `yes`).
pub fn ensure(env: &HostEnv, name: &str, shape: &Shape, yes: bool) -> Result<State> {
    if let Some(st) = state::live(env, name).filter(|s| s.phase == Phase::Running) {
        return Ok(st);
    }
    if let Some(p) = host_gateway(env, name, shape) {
        eprintln!("paddock: {}", host_notice(name, None, shape, p));
    }
    let q = format!("paddock: no sandboxed {name} gateway runs. Start one now?");
    if !yes && !ask(&q) {
        bail!("no sandboxed {name} gateway runs. Start it with: paddock gateway {name} start");
    }
    eprintln!("paddock: starting the sandboxed {name} gateway ...");
    let st = control::start(env, name, &control::StartOpts::default())?;
    eprintln!("paddock: the {name} gateway listens on {}", st.url);
    Ok(st)
}

/// The harness and its gateway shape.
pub fn harness(env: &HostEnv, name: &str) -> Result<(Harness, Shape)> {
    let factory = HarnessFactory::new(env)?;
    let h = factory.create(name)?;
    let shape = super::shape_of(&h.profile)?;
    Ok((h, shape))
}

/// Run the client `command` with the arguments `extra` in its sandbox,
/// attached to the gateway. Returns its exit code.
pub fn attach(
    env: &HostEnv,
    name: &str,
    command: &str,
    extra: &[String],
    yes: bool,
) -> Result<i32> {
    crate::update::refuse_in_sandbox(env, &format!("paddock {name} {command}"))?;
    let (h, shape) = harness(env, name)?;
    let st = ensure(env, name, &shape, yes)?;
    let token = state::existing_token(env, name).with_context(|| {
        format!("the {name} gateway has no token: paddock gateway {name} start")
    })?;
    let spec = spec::client(env, &h, &shape, st.port, &token, command, extra)?;
    let interactive = std::io::stdin().is_terminal();
    let mut p = launch::prepare(env, &spec, "auto", interactive)?;
    if let Some(r) = &p.reason {
        eprintln!("paddock: the client runs in the VM: {r}");
    }
    log::info!(
        "{name} client attaches to {} ({})",
        shape.url(&Values {
            port: st.port,
            token: ""
        }),
        p.backend
    );
    let mut child = p
        .cmd
        .spawn()
        .with_context(|| format!("cannot start the {name} client sandbox"))?;
    // Ctrl-C belongs to the client: paddock waits for it to end. The
    // handlers change after the start, so the client keeps its own.
    // SAFETY: plain signal(2) calls; the old handlers come back below.
    let (old_int, old_quit) = unsafe {
        (
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGQUIT, libc::SIG_IGN),
        )
    };
    let result = (|| -> Result<i32> {
        let _rec = state::ClientGuard::new(env, name, child.id())?;
        let status = child.wait()?;
        use std::os::unix::process::ExitStatusExt as _;
        Ok(status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
    })();
    // SAFETY: as above.
    unsafe {
        libc::signal(libc::SIGINT, old_int);
        libc::signal(libc::SIGQUIT, old_quit);
    }
    result
}
