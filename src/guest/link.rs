//! Links and login callbacks in the guest, platform-neutral.
//!
//! The default browser of the guest is the link helper `Paddock Links.app`:
//! a bundle whose executable is a copy of the agent binary. The offline
//! step of the golden makes it the handler of `http` and `https` for the
//! guest account (`macvm/launch_services.rs`), so macOS shows no prompt to
//! change the default browser. When a guest program opens a link, Launch
//! Services starts the helper with the link in an Apple event
//! (`link_macos.rs`). The helper writes the link as one line to the
//! agent's socket [`SOCKET`], and the agent sends the `link.open` event to
//! the host, which asks and opens the link in the host browser.
//!
//! The other direction: when the host opened a login link whose callback
//! is a loopback port, it listens on that port of the host loopback. Each
//! connection there becomes a `login.forward` request: the agent connects
//! to the same port on the guest loopback first, and only when a guest
//! process listens there it dials the `guest-login` vsock port with the
//! token line and the ticket line, and copies the bytes both ways.

use crate::macvm::text;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The bundle identifier of the link helper. The LaunchServices
/// preference of the golden names it for `http` and `https`.
pub const BUNDLE_ID: &str = "dev.paddock.link";
/// The folder of the helper bundle, under the agent folder.
pub const BUNDLE_DIR: &str = "link/Paddock Links.app";
/// The executable name in `Contents/MacOS` of the bundle.
pub const EXECUTABLE: &str = "paddock-link";
/// The socket of the agent where the helper writes each link.
pub const SOCKET: &str = "/Library/Application Support/paddock/link.sock";
/// The longest link, in bytes.
pub const URL_MAX: usize = 32 * 1024;
/// The helper waits this long for the first link, then exits.
pub const HELPER_WAIT: Duration = Duration::from_secs(10);
/// After the last link, the helper waits this long for more, then exits.
pub const HELPER_LINGER: Duration = Duration::from_secs(3);
/// The agent gives the helper this long to send its line.
pub const LINE_WAIT: Duration = Duration::from_secs(5);
/// The connect to a guest loopback port gives up after this.
pub const LOCAL_CONNECT: Duration = Duration::from_secs(2);
/// A forwarded callback that carries no byte for this long closes.
pub const FORWARD_IDLE: Duration = Duration::from_secs(60);

/// The bundle of the helper in the agent folder `support`.
pub fn bundle_path(support: &Path) -> PathBuf {
    support.join(BUNDLE_DIR)
}

/// The executable of the helper in its bundle.
pub fn executable(bundle: &Path) -> PathBuf {
    bundle.join("Contents/MacOS").join(EXECUTABLE)
}

/// True when this process runs as the helper: its executable is the one
/// of a helper bundle.
pub fn is_helper(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| n == EXECUTABLE)
        && exe.parent().is_some_and(|p| p.ends_with("Contents/MacOS"))
}

/// The `Info.plist` of the helper: a background app (no Dock icon, no
/// menu bar) that claims the `http` and `https` schemes.
pub fn info_plist(version: &str) -> String {
    let body = format!(
        // not user text
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleName</key>
	<string>Paddock Links</string>
	<key>CFBundleDisplayName</key>
	<string>Paddock Links</string>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>LSMinimumSystemVersion</key>
	<string>14.0</string>
	<key>CFBundleURLTypes</key>
	<array>
		<dict>
			<key>CFBundleURLName</key>
			<string>Web links</string>
			<key>CFBundleTypeRole</key>
			<string>Viewer</string>
			<key>CFBundleURLSchemes</key>
			<array>
				<string>http</string>
				<string>https</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    );
    body
}

/// Write `body` to `path` through a temporary file and a rename, unless
/// the file holds it already. True when the file changed.
fn write_if_changed(path: &Path, body: &[u8], mode: u32) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::read(path).is_ok_and(|now| now == body) {
        return Ok(false);
    }
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, body)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Make the helper bundle at `bundle`: the `Info.plist` and a copy of the
/// agent binary `exe`, each only when it differs. True when anything
/// changed, so Launch Services must read the bundle again.
pub fn ensure_bundle(bundle: &Path, exe: &Path, version: &str) -> io::Result<bool> {
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    let plist = write_if_changed(
        &bundle.join("Contents/Info.plist"),
        info_plist(version).as_bytes(),
        0o644,
    )?;
    let binary = std::fs::read(exe)?;
    let copied = write_if_changed(&executable(bundle), &binary, 0o755)?;
    Ok(plist || copied)
}

/// Why a link is refused, or `Ok` for an http or https link that the
/// host may see.
pub fn check_url(url: &str) -> Result<(), String> {
    if url.len() > URL_MAX {
        return Err(text::link_too_long(url.len(), URL_MAX));
    }
    if url.chars().any(char::is_control) || !crate::gatewayd::web_link(url) {
        return Err(text::link_not_web());
    }
    Ok(())
}

/// The agent side of one helper connection: read one line, check it, hand
/// it to `sink` (the `link.open` event), answer `ok` or `error TEXT`.
pub fn serve_one(mut stream: impl Read + Write, sink: &dyn Fn(&str) -> Result<(), String>) {
    let line = match crate::macvm::proto::read_raw_line(&mut stream, URL_MAX + 1) {
        Ok(Some(l)) => l,
        Ok(None) => return,
        Err(e) => {
            let msg = text::link_unreadable(&format!("{e:#}"));
            log::warn!("link: {msg}");
            let _ = writeln!(stream, "error {msg}"); // not user text
            return;
        }
    };
    let url = line.trim_end_matches('\r');
    let answer = check_url(url).and_then(|_| sink(url));
    match answer {
        Ok(()) => {
            log::info!("link: {}", crate::macvm::proto::link_kind(url));
            let _ = writeln!(stream, "ok");
        }
        Err(e) => {
            log::warn!("link: refused {}: {e}", crate::macvm::proto::link_kind(url));
            let _ = writeln!(stream, "error {e}"); // not user text
        }
    }
}

/// Bind the agent's socket at `path`: a socket left by an earlier agent
/// goes first, and only the owner may connect (0600).
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if std::os::unix::fs::FileTypeExt::is_socket(&m.file_type()) => {
            std::fs::remove_file(path)?
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                text::link_socket_taken(path),
            ))
        }
        Err(_) => {}
    }
    let l = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// Where the agent hands each link: the `link.open` event.
pub type Sink = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Serve the helpers, one thread per connection, until the listener
/// fails.
pub fn serve(listener: UnixListener, sink: Sink) {
    for conn in listener.incoming() {
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                log::warn!("link: accept failed: {e}");
                continue;
            }
        };
        let sink = sink.clone();
        let _ = std::thread::Builder::new()
            .name("link".into())
            .spawn(move || {
                let _ = conn.set_read_timeout(Some(LINE_WAIT));
                serve_one(conn, &*sink)
            });
    }
}

/// The helper side: send one link to the agent at `path` and read its
/// answer.
pub fn send(path: &Path, url: &str) -> Result<(), String> {
    let mut s = UnixStream::connect(path).map_err(|e| text::link_no_agent(path, &e.to_string()))?;
    let _ = s.set_read_timeout(Some(LINE_WAIT));
    writeln!(s, "{url}").map_err(|e| text::link_no_agent(path, &e.to_string()))?;
    match crate::macvm::proto::read_raw_line(&mut s, 4096) {
        Ok(Some(l)) if l == "ok" => Ok(()),
        Ok(Some(l)) => Err(l.strip_prefix("error ").unwrap_or(&l).to_string()),
        Ok(None) => Err(text::link_no_answer()),
        Err(e) => Err(text::link_no_agent(path, &format!("{e:#}"))),
    }
}

/// A ticket of the host: 16 to 64 lower-case hex digits, so it can never
/// break the line.
pub fn valid_ticket(t: &str) -> bool {
    (16..=64).contains(&t.len()) && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Connect to `port` on the guest loopback: `127.0.0.1` first, then
/// `[::1]`. An error means that no guest process listens there.
pub fn connect_local(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&v4, timeout) {
        Ok(s) => Ok(s),
        Err(first) => {
            let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
            TcpStream::connect_timeout(&v6, timeout).map_err(|_| first)
        }
    }
}

/// The start of one forward: a guest process must listen on `port`, then
/// `dial` opens the `guest-login` connection, which gets the token line
/// and the ticket line. The two streams to splice, or the reason.
pub fn forward_start<U: Write>(
    port: u16,
    ticket: &str,
    token: Option<&str>,
    local: impl FnOnce(u16) -> io::Result<TcpStream>,
    dial: impl FnOnce() -> io::Result<U>,
) -> Result<(TcpStream, U), String> {
    if port == 0 || !valid_ticket(ticket) {
        return Err(text::login_forward_bad());
    }
    let Some(token) = token else {
        return Err(text::login_forward_no_token());
    };
    let local = local(port).map_err(|e| text::login_nothing_listens(port, &e.to_string()))?;
    let mut up = dial().map_err(|e| text::login_no_host(port, &e.to_string()))?;
    let front = format!("{token}\n{ticket}\n"); // not user text
    up.write_all(front.as_bytes())
        .and_then(|_| up.flush())
        .map_err(|e| text::login_no_host(port, &e.to_string()))?;
    Ok((local, up))
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
