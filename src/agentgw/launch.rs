//! Turn the spec of a gateway or of a client into a command.
//!
//! Linux: the backend that `--backend auto` picks for the spec, bwrap
//! first. macOS: Seatbelt. When Seatbelt cannot run the spec, the macOS VM
//! runs it instead, and the state names the reason.
//!
//! The tests of hosts without bubblewrap set
//! `PADDOCK_TEST_GATEWAY_LAUNCHER=plain`: the command then runs with no
//! isolation, with the home and the variables of the spec. Only the
//! sandbox is missing; the files, the token, the port and the restarts are
//! the same.

use crate::backend;
use crate::env::HostEnv;
use crate::spec::SandboxSpec;
use anyhow::{bail, Context as _, Result};
use std::process::Command;

/// Test only: run the command with no sandbox.
pub const LAUNCHER_VAR: &str = "PADDOCK_TEST_GATEWAY_LAUNCHER";

/// A command that is ready to start, and the backend that renders it.
pub struct Prepared {
    pub cmd: Command,
    pub backend: String,
    /// Why the backend is not the first choice of this system.
    pub reason: Option<String>,
}

/// The test launcher is on.
pub fn plain(env: &HostEnv) -> bool {
    env.var(LAUNCHER_VAR) == Some("plain")
}

/// The backend for `spec`. `wanted` is `auto` or a backend name.
fn pick(
    env: &HostEnv,
    spec: &SandboxSpec,
    wanted: &str,
) -> Result<(Box<dyn backend::Backend>, Option<String>)> {
    if wanted != "auto" {
        let b = backend::by_name(wanted)?;
        b.check(env, spec)?;
        return Ok((b, None));
    }
    if cfg!(target_os = "macos") {
        let sb = backend::by_name(backend::seatbelt::NAME)?;
        match sb.check(env, spec) {
            Ok(()) => return Ok((sb, None)),
            Err(e) => {
                let vm = backend::by_name(backend::vm::NAME)?;
                vm.check(env, spec).with_context(|| {
                    format!("Seatbelt cannot run the gateway ({e:#}), and the VM cannot either")
                })?;
                return Ok((vm, Some(format!("Seatbelt cannot run it: {e:#}"))));
            }
        }
    }
    Ok((backend::select(env, spec, "auto")?, None))
}

/// Check that this host can run `spec`, before a start hands it to a unit.
pub fn check(env: &HostEnv, spec: &SandboxSpec, wanted: &str) -> Result<String> {
    if plain(env) {
        return Ok("plain".into());
    }
    Ok(pick(env, spec, wanted)?.0.name().to_string())
}

/// The command of `spec`. `interactive`: a client in a terminal.
pub fn prepare(
    env: &HostEnv,
    spec: &SandboxSpec,
    wanted: &str,
    interactive: bool,
) -> Result<Prepared> {
    if plain(env) {
        let Some((prog, args)) = spec.command.split_first() else {
            bail!("the spec has no command");
        };
        let mut cmd = Command::new(prog);
        cmd.args(args).env_clear();
        for (k, v) in &spec.env {
            cmd.env(k, &v.value);
        }
        cmd.env("HOME", &spec.home_src)
            .env("PATH", env.var("PATH").unwrap_or("/usr/bin:/bin"))
            .env("PADDOCK", "1")
            .current_dir(&spec.project);
        return Ok(Prepared {
            cmd,
            backend: "plain".into(),
            reason: None,
        });
    }
    let (b, reason) = pick(env, spec, wanted)?;
    let mut spec = spec.clone();
    if b.name() == backend::seatbelt::NAME {
        // Seatbelt has no network of its own: outbound traffic goes through
        // paddock's proxy, which serves in this process for as long as the
        // gateway or the client runs.
        backend::seatbelt::run::start_proxy(env, &mut spec)?;
    }
    let spec = &spec;
    b.prepare(env, spec, interactive)?;
    let launch = b.render(env, spec, interactive)?;
    crate::run::write_files(&launch)?;
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args).env_clear().envs(&launch.env);
    if let Some(dir) = &launch.cwd {
        cmd.current_dir(dir);
    }
    Ok(Prepared {
        cmd,
        backend: b.name().to_string(),
        reason,
    })
}

/// A server answers on `127.0.0.1:port`: a connection opens and stays
/// open. A forwarder such as pasta accepts the connection before the
/// gateway inside listens, and then closes it at once; that is no answer.
pub fn answers(port: u16) -> bool {
    use std::io::Read as _;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
    else {
        return false;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(200)));
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Ok(0) => false,
        Ok(_) => true,
        Err(e) => matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
    }
}

/// Nothing listens on `127.0.0.1:port` now.
pub fn free(port: u16) -> bool {
    port > 0 && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() && !answers(port)
}

/// A port that the system gives as free.
pub fn any_free() -> Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).context("cannot listen on 127.0.0.1")?;
    Ok(l.local_addr()?.port())
}

/// The addresses that listen on `port` in the network of this process, from
/// `/proc/net/tcp` and `/proc/net/tcp6` (Linux). None elsewhere.
pub fn listeners(port: u16) -> Option<Vec<String>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let mut out = Vec::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        out.extend(parse_listeners(&text, port, v6));
    }
    Some(out)
}

/// The listening addresses on `port` of one `/proc/net/tcp{,6}` table, as
/// `127.0.0.1`, `0.0.0.0`, `::1` or `::`-style text.
pub fn parse_listeners(text: &str, port: u16, v6: bool) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(local), Some(st)) = (f.get(1), f.get(3)) else {
            continue;
        };
        if *st != "0A" {
            continue;
        }
        let Some((addr, p)) = local.split_once(':') else {
            continue;
        };
        if u16::from_str_radix(p, 16).ok() != Some(port) {
            continue;
        }
        out.push(if v6 { v6_text(addr) } else { v4_text(addr) });
    }
    out
}

fn v4_text(hex: &str) -> String {
    let n = u32::from_str_radix(hex, 16).unwrap_or(u32::MAX);
    // The kernel writes the address in host byte order of a little-endian
    // word: 0100007F is 127.0.0.1.
    std::net::Ipv4Addr::from(n.swap_bytes()).to_string()
}

fn v6_text(hex: &str) -> String {
    let mut b = [0u8; 16];
    for (i, chunk) in hex.as_bytes().chunks(8).take(4).enumerate() {
        let w = std::str::from_utf8(chunk)
            .ok()
            .and_then(|s| u32::from_str_radix(s, 16).ok())
            .unwrap_or(u32::MAX)
            .swap_bytes();
        b[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
    }
    std::net::Ipv6Addr::from(b).to_string()
}

/// A listener address is the loopback of this host.
pub fn is_loopback(addr: &str) -> bool {
    match addr.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_loopback(),
        Ok(std::net::IpAddr::V6(a)) => {
            a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v| v.is_loopback())
        }
        Err(_) => false,
    }
}
