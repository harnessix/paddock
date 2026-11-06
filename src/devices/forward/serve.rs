//! `paddock device serve-remote [--socket PATH]`: the serving side.
//!
//! It serves the devices of the user configuration of this machine to
//! another paddock, over the socket framing of socket.rs, on the standard
//! input and output (one request per process: `ssh HOST paddock device
//! serve-remote`) or on a Unix socket (many connections). It is a broker
//! without an agent sandbox: the caller identity is the transport,
//! `remote:PEER`, where PEER is the client address that sshd put into
//! `SSH_CONNECTION`, or `local` for the standard input of a plain process
//! and for a socket. Only a table with a `remote:` grant that covers the
//! peer is on; every other table answers "disabled" with the grant to
//! add. The caller fields of a request are text for the card and the
//! audit line. The devices run as on this machine: in a device sandbox,
//! on the host, or shared, with their own card, instance and log.

use super::super::grant::GrantCtx;
use super::super::sandbox::{Instances, Launcher};
use super::super::{disabled, enabled, socket, Device, DeviceCtx, DevicesCfg};
use crate::broker::Approver;
use crate::env::HostEnv;
use anyhow::{Context as _, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The peer of a request that came on the standard input without ssh, or
/// on a socket: a process of this machine.
pub const PEER_LOCAL: &str = "local";
/// The name that the devices see as the harness of a remote caller.
pub const HARNESS: &str = "remote";
/// How often the socket server looks at the stop flag.
const TICK: Duration = Duration::from_millis(200);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Ask a socket server in this process to stop (tests).
pub fn ask_stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// The peer of this process: the client address of `SSH_CONNECTION`
/// (sshd sets it, the client cannot), else `local`.
pub fn peer_of(env: &HostEnv) -> String {
    env.var("SSH_CONNECTION")
        .and_then(|c| c.split_whitespace().next())
        .filter(|a| !a.is_empty() && a.len() <= 64)
        .map(|a| a.to_string())
        .unwrap_or_else(|| PEER_LOCAL.to_string())
}

/// The identity that the devices see: `remote:PEER`.
pub fn identity(peer: &str) -> String {
    format!("remote:{peer}")
}

/// The launcher of the device sandboxes: the product one, or a thread
/// when the test variable asks for it.
fn launcher(env: &HostEnv) -> Launcher {
    match env.var(super::super::shared::LAUNCHER_VAR) {
        Some("thread") => Launcher::Thread,
        _ => Launcher::host(),
    }
}

/// The devices for one peer, and the registry of their instances.
pub type Served = (Vec<Arc<dyn Device>>, Arc<Instances>);

/// The devices of this machine for the paddock behind `peer`, with the
/// instance registry that holds their device sandboxes. The caller that
/// a device sees is `remote:PEER` with the harness `remote`.
pub fn devices_for(env: &HostEnv, peer: &str, approver: Arc<dyn Approver>) -> Result<Served> {
    let cfg = DevicesCfg::load(env)?;
    let grants = GrantCtx::for_remote(peer);
    let instances = Instances::with_launcher(env.clone(), launcher(env));
    let id = identity(peer);
    let ctx = DeviceCtx {
        env: env.clone(),
        sandbox: id.clone(),
        harness: HARNESS.into(),
        project: PathBuf::from(&id),
        started_by: Vec::new(),
        approver,
        instances: instances.clone(),
    };
    let mut devices = enabled(&cfg, false, &grants, &ctx);
    let off = disabled(&cfg, &devices, &grants, &ctx);
    devices.extend(off);
    Ok((devices, instances))
}

/// Serve one request that comes on `input` and answers on `output` (the
/// ssh case). The server speaks to a socket pair: one thread moves the
/// input into it and shuts the write side when the input ends, so the
/// server sees a gone peer as it does for a socket and cancels the
/// request. Returns when the answer is written. The device instances of
/// the request end before that.
pub fn serve_stdio(
    env: &HostEnv,
    peer: &str,
    approver: Arc<dyn Approver>,
    input: impl Read + Send + 'static,
    mut output: impl Write,
) -> Result<()> {
    let (devices, instances) = devices_for(env, peer, approver)?;
    let server = socket::Server::for_remote(&identity(peer), devices);
    let (a, b) = UnixStream::pair().context("cannot make a socket pair")?;
    let mut to_server = a.try_clone().context("cannot clone the socket pair")?;
    std::thread::spawn(move || {
        let mut input = input;
        let _ = std::io::copy(&mut input, &mut to_server);
        let _ = to_server.shutdown(std::net::Shutdown::Write);
    });
    let worker = std::thread::spawn(move || server.handle(b));
    // Each piece of the answer goes out at once: a stream must reach the
    // other side as it comes, and the standard output is line buffered.
    let mut from_server = a;
    let mut buf = [0u8; 65536];
    let copied = loop {
        match from_server.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                if let Err(e) = output.write_all(&buf[..n]).and_then(|()| output.flush()) {
                    break Err(e);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => break Err(e),
        }
    };
    let _ = output.flush();
    let _ = worker.join();
    instances.stop_all();
    copied.context("cannot write the answer")?;
    Ok(())
}

/// Serve on the socket `path` until SIGTERM or SIGINT. The folder is
/// made 0700 and the socket 0600: who can connect is who can reach the
/// folder, which is the whole authentication of a socket transport.
pub fn serve_socket(
    env: &HostEnv,
    peer: &str,
    approver: Arc<dyn Approver>,
    path: &Path,
) -> Result<()> {
    let (devices, instances) = devices_for(env, peer, approver)?;
    if let Some(dir) = path.parent() {
        crate::privfs::dir(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    crate::sockpath::check(path)?;
    let server = socket::Server::for_remote(&identity(peer), devices);
    let serving = socket::serve(path, server)?;
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a plain sigaction(2) with a handler that stores a flag.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    log::info!(
        "serving the devices of this machine to remote callers on {}",
        path.display()
    );
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(TICK);
    }
    drop(serving);
    instances.stop_all();
    Ok(())
}

/// The command: on `socket` when given, else one request on the standard
/// input and output. The peer is the ssh client, or `local`.
pub fn run(env: &HostEnv, socket: Option<&Path>) -> Result<i32> {
    let approver: Arc<dyn Approver> = Arc::new(crate::broker::HostApprover);
    match socket {
        Some(p) => serve_socket(env, PEER_LOCAL, approver, p)?,
        None => {
            let peer = peer_of(env);
            serve_stdio(
                env,
                &peer,
                approver,
                std::io::stdin(),
                std::io::stdout().lock(),
            )?;
        }
    }
    Ok(0)
}
