//! Links for app sandboxes: `paddock app open`, the link socket inside the
//! sandbox (`paddock inbound`), and the host URL scheme handlers.

use super::{app_profile, ports, run, AppOptions, INBOUND_DIR};
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A link the app may receive: one of its own schemes, a sane length, and no
/// control characters.
pub fn valid_link(link: &str, schemes: &[String]) -> Result<()> {
    if link.len() > 8192 || link.chars().any(|c| c.is_control()) {
        bail!("the link is too long or has control characters");
    }
    let url = url::Url::parse(link).context("not a valid link")?;
    if !schemes.iter().any(|s| s == url.scheme()) {
        bail!(
            "{}:// links do not belong to this app (it handles: {})",
            url.scheme(),
            schemes.join(", ")
        );
    }
    Ok(())
}

/// `paddock inbound`, inside an app sandbox: start the app with each link
/// that arrives on the socket.
pub fn inbound_serve() -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    // Login callbacks of a terminal harness: ports only, no app to start.
    if std::env::var("PADDOCK_INBOUND_CMD").is_err()
        && std::env::var("PADDOCK_INBOUND_PORTS").is_ok_and(|v| v == "1")
    {
        ports::serve_inside(PathBuf::from(INBOUND_DIR))?;
        loop {
            std::thread::park();
        }
    }
    let cmd: Vec<String> = serde_json::from_str(
        &std::env::var("PADDOCK_INBOUND_CMD").context("not in an app sandbox")?,
    )?;
    let schemes: Vec<String> = std::env::var("PADDOCK_INBOUND_SCHEMES")
        .unwrap_or_default()
        .split(',')
        .map(str::to_string)
        .collect();
    if cmd.is_empty() {
        bail!("no app command");
    }
    if std::env::var("PADDOCK_INBOUND_PORTS").is_ok_and(|v| v == "1") {
        ports::serve_inside(PathBuf::from(INBOUND_DIR))?;
    }
    let sock = PathBuf::from(INBOUND_DIR).join("open.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = std::os::unix::net::UnixListener::bind(&sock)?;
    for conn in listener.incoming().flatten() {
        let mut line = String::new();
        let mut reader = BufReader::new(std::io::Read::take(&conn, 9000));
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let link = line.trim_end_matches(['\r', '\n']);
        let reply = match valid_link(link, &schemes) {
            Err(e) => format!("error: {e}"),
            Ok(()) => match Command::new(&cmd[0])
                .args(&cmd[1..])
                .arg(link)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    std::thread::spawn(move || child.wait());
                    "ok".to_string()
                }
                Err(e) => format!("error: cannot start the app: {e}"),
            },
        };
        let mut w = &conn;
        let _ = writeln!(w, "{reply}");
    }
    Ok(())
}

/// `paddock app open NAME LINK`, on the host: give a link to the running app
/// sandbox, or start the app with it.
pub fn open(env: &HostEnv, name: &str, link: &str) -> Result<i32> {
    use std::io::{BufRead, BufReader, Write};
    let profile = app_profile(env, name)?;
    let schemes = profile
        .app
        .as_ref()
        .map(|a| a.schemes.clone())
        .unwrap_or_default();
    valid_link(link, &schemes)?;
    // A running flatpak app has no link socket: a second `flatpak run` hands
    // the link to it.
    let running = crate::state::Running::list(env)
        .into_iter()
        .find(|r| r.harness == name && r.gui == "app" && r.backend != "flatpak");
    if let Some(r) = running {
        let inbound = env.paddock_run().join(&r.id).join("inbound");
        let mut s = ports::connect_inside(&inbound, "open.sock")
            .with_context(|| format!("the {name} sandbox is not ready for links yet"))?;
        writeln!(s, "{link}")?;
        let mut reply = String::new();
        BufReader::new(&s).read_line(&mut reply)?;
        let reply = reply.trim();
        if reply != "ok" {
            bail!("the {name} sandbox refused the link: {reply}");
        }
        return Ok(0);
    }
    run(
        env,
        &AppOptions {
            name: name.to_string(),
            args: vec![link.to_string()],
            ..Default::default()
        },
    )
}

pub fn link_desktop_file(env: &HostEnv, name: &str) -> PathBuf {
    env.data_dir
        .join("applications")
        .join(format!("paddock-{name}-links.desktop"))
}

fn handlers_file(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("link-handlers.json")
}

fn xdg_mime(env: &HostEnv, args: &[&str]) -> Result<String> {
    let exe = env
        .which("xdg-mime")
        .context("link handling needs xdg-mime (package xdg-utils)")?;
    crate::env::capture(Command::new(exe).args(args))
}

/// Make paddock the handler of the app's URL schemes, and remember the old
/// handlers so that `release_links` can give them back.
pub fn claim_links(env: &HostEnv, name: &str) -> Result<Vec<(String, String)>> {
    let p = app_profile(env, name)?;
    let schemes = p.app.map(|a| a.schemes).unwrap_or_default();
    if schemes.is_empty() {
        return Ok(vec![]);
    }
    // Never a build: see launchers::launcher_exe.
    let me = crate::launchers::launcher_exe(env)?;
    let mime: String = schemes
        .iter()
        .map(|s| format!("x-scheme-handler/{s};"))
        .collect();
    let f = link_desktop_file(env, name);
    std::fs::create_dir_all(f.parent().unwrap())?;
    std::fs::write(
        &f,
        format!("[Desktop Entry]\nType=Application\nName={name} links (paddock)\nExec={} app open {name} %u\nNoDisplay=true\nTerminal=false\nMimeType={mime}\n", me.display()),
    )?;
    let mut saved: std::collections::BTreeMap<String, String> =
        std::fs::read_to_string(handlers_file(env))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    let ours = f.file_name().unwrap().to_string_lossy().into_owned();
    let mut out = Vec::new();
    for s in schemes {
        let mime = format!("x-scheme-handler/{s}");
        let old = xdg_mime(env, &["query", "default", &mime]).unwrap_or_default();
        if old != ours {
            saved.insert(s.clone(), old.clone());
        }
        xdg_mime(env, &["default", &ours, &mime])?;
        out.push((s, saved.get(&out_key(&mime)).cloned().unwrap_or(old)));
    }
    std::fs::create_dir_all(handlers_file(env).parent().unwrap())?;
    std::fs::write(handlers_file(env), serde_json::to_string_pretty(&saved)?)?;
    Ok(out)
}

fn out_key(mime: &str) -> String {
    mime.trim_start_matches("x-scheme-handler/").to_string()
}

/// Give the app's URL schemes back to the handlers they had before.
pub fn release_links(env: &HostEnv, name: &str) -> Result<Vec<(String, String)>> {
    let p = app_profile(env, name)?;
    let schemes = p.app.map(|a| a.schemes).unwrap_or_default();
    let mut saved: std::collections::BTreeMap<String, String> =
        std::fs::read_to_string(handlers_file(env))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    let mut out = Vec::new();
    for s in schemes {
        if let Some(old) = saved.remove(&s) {
            if !old.is_empty() {
                xdg_mime(env, &["default", &old, &format!("x-scheme-handler/{s}")])?;
            }
            out.push((s, old));
        }
    }
    let _ = std::fs::remove_file(link_desktop_file(env, name));
    if handlers_file(env).exists() {
        std::fs::write(handlers_file(env), serde_json::to_string_pretty(&saved)?)?;
    }
    Ok(out)
}
