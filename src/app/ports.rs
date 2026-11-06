//! Login callbacks to `http://127.0.0.1:PORT` for an app in a sandbox
//! (`[apps.NAME] login_ports`). See the doc comment of `app::ports` in
//! `app.rs` for what the two sides do.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const PORTS_FILE: &str = "ports.json";
const FWD_SOCK: &str = "fwd.sock";

/// TCP ports in LISTEN state, from the text of /proc/net/tcp or
/// /proc/net/tcp6. Privileged ports are left out.
pub fn listening(tcp: &str) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    for line in tcp.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 || f[3] != "0A" {
            continue;
        }
        if let Some(port) = f[1]
            .rsplit(':')
            .next()
            .and_then(|h| u16::from_str_radix(h, 16).ok())
        {
            if port >= 1024 {
                out.insert(port);
            }
        }
    }
    out
}

fn scan() -> BTreeSet<u16> {
    let mut s = BTreeSet::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        s.extend(listening(&std::fs::read_to_string(f).unwrap_or_default()));
    }
    s
}

fn copy_both(a: TcpStream, b: UnixStream) {
    let (Ok(mut a2), Ok(mut b2)) = (a.try_clone(), b.try_clone()) else {
        return;
    };
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(Shutdown::Write);
    });
    let (mut a, mut b) = (a, b);
    let _ = std::io::copy(&mut b, &mut a);
    let _ = a.shutdown(Shutdown::Write);
    let _ = t.join();
}

/// Inside the sandbox: report listening ports, and connect forwarded
/// connections to them.
pub fn serve_inside(dir: PathBuf) -> Result<()> {
    let fwd = dir.join(FWD_SOCK);
    let _ = std::fs::remove_file(&fwd);
    let listener = UnixListener::bind(&fwd)?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            std::thread::spawn(move || {
                // Read the port line one byte at a time: a buffered read
                // could take the first bytes of the forwarded stream too.
                let mut line = Vec::new();
                let mut b = [0u8; 1];
                let mut r = &conn;
                while line.len() < 8 {
                    match r.read(&mut b) {
                        Ok(1) if b[0] == b'\n' => break,
                        Ok(1) => line.push(b[0]),
                        _ => return,
                    }
                }
                let Ok(port) = String::from_utf8_lossy(&line).trim().parse::<u16>() else {
                    return;
                };
                // Only ports the app listens on right now.
                if !scan().contains(&port) {
                    return;
                }
                let Ok(tcp) = TcpStream::connect(("127.0.0.1", port)) else {
                    return;
                };
                copy_both(tcp, conn);
            });
        }
    });
    std::thread::spawn(move || {
        let mut last = BTreeSet::new();
        loop {
            let now = scan();
            if now != last {
                let tmp = dir.join("ports.json.tmp");
                if std::fs::write(&tmp, serde_json::to_string(&now).unwrap_or_default()).is_ok() {
                    let _ = std::fs::rename(&tmp, dir.join(PORTS_FILE));
                }
                last = now;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    Ok(())
}

/// On the host: follow the reported ports and listen on 127.0.0.1 for
/// each, while it is reported. Stops when dropped.
pub struct Forwarder {
    stop: Arc<AtomicBool>,
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Forwarder {
    pub fn start(dir: PathBuf) -> Self {
        Self::start_filtered(dir, |_| true)
    }

    /// Forward only the reported ports that `filter` accepts, for example
    /// a login callback port while its login is under way.
    pub fn start_filtered(dir: PathBuf, filter: fn(u16) -> bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || manage(&dir, &s2, filter));
        Self { stop }
    }
}

/// The sandbox writes this folder, so the ports file is read without
/// following a symlink, never from a FIFO, and only while it is small.
fn read_ports(dir: &Path) -> BTreeSet<u16> {
    crate::safefs::Root::open(dir)
        .and_then(|r| r.read(Path::new(PORTS_FILE), 64 * 1024))
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_slice(&t).ok())
        .unwrap_or_default()
}

/// Connect to a socket that the sandbox made in `dir`. A symlink there
/// is refused, so the sandbox cannot point paddock at a host socket.
pub fn connect_inside(dir: &Path, name: &str) -> Result<UnixStream> {
    let root = crate::safefs::Root::open(dir)?;
    root.dir(Path::new(""), false)?
        .connect(std::ffi::OsStr::new(name))
}

fn manage(dir: &Path, stop: &Arc<AtomicBool>, filter: fn(u16) -> bool) {
    let mut open: BTreeMap<u16, Arc<AtomicBool>> = BTreeMap::new();
    while !stop.load(Ordering::SeqCst) {
        let mut wanted = read_ports(dir);
        wanted.retain(|p| filter(*p));
        open.retain(|p, flag| {
            let keep = wanted.contains(p);
            if !keep {
                flag.store(true, Ordering::SeqCst);
            }
            keep
        });
        for p in wanted {
            if open.contains_key(&p) {
                continue;
            }
            // Loopback only, never another address of the host.
            let l = match TcpListener::bind(("127.0.0.1", p)) {
                Ok(l) => l,
                Err(e) => {
                    log::debug!("cannot forward port {p}: {e}");
                    continue;
                }
            };
            log::debug!("forwarding port {p} into the sandbox");
            let _ = l.set_nonblocking(true);
            let flag = Arc::new(AtomicBool::new(false));
            open.insert(p, flag.clone());
            let (sock, stop) = (dir.to_path_buf(), stop.clone());
            std::thread::spawn(move || {
                while !flag.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                    match l.accept() {
                        Ok((tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let sock = sock.clone();
                            std::thread::spawn(move || {
                                let mut u = match connect_inside(&sock, FWD_SOCK) {
                                    Ok(u) => u,
                                    Err(e) => {
                                        log::warn!("port {p}: cannot reach the sandbox: {e}");
                                        return;
                                    }
                                };
                                if writeln!(u, "{p}").is_err() {
                                    return;
                                }
                                copy_both(tcp, u);
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(50)),
                    }
                }
            });
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    for f in open.values() {
        f.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_listening_ports_from_proc() {
        let tcp = "  sl  local_address rem_address   st\n   0: 0100007F:B8AC 00000000:0000 0A x\n   1: 0100007F:B8AD 0100007F:1F90 01 x\n   2: 00000000:0016 00000000:0000 0A x\n";
        assert_eq!(listening(tcp), BTreeSet::from([0xB8AC]));
    }

    // The sandbox socket check uses /proc/self/fd. That is Linux app mode.
    #[cfg(target_os = "linux")]
    #[test]
    fn never_follows_links_or_fifos_the_sandbox_plants() {
        let t = tempfile::tempdir().unwrap();
        let (inbound, host) = (t.path().join("inbound"), t.path().join("host"));
        std::fs::create_dir_all(&inbound).unwrap();
        std::fs::create_dir_all(&host).unwrap();
        // A host socket the sandbox must not reach.
        let _l = UnixListener::bind(host.join("bus")).unwrap();
        std::os::unix::fs::symlink(host.join("bus"), inbound.join(FWD_SOCK)).unwrap();
        assert!(connect_inside(&inbound, FWD_SOCK).is_err());
        std::fs::remove_file(inbound.join(FWD_SOCK)).unwrap();
        let _own = UnixListener::bind(inbound.join(FWD_SOCK)).unwrap();
        assert!(connect_inside(&inbound, FWD_SOCK).is_ok());

        // A FIFO as the ports file reads as no ports and does not hang.
        let c = std::ffi::CString::new(inbound.join(PORTS_FILE).to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_ports(&inbound).is_empty());
        std::fs::remove_file(inbound.join(PORTS_FILE)).unwrap();
        std::fs::write(host.join("p.json"), "[8080]").unwrap();
        std::os::unix::fs::symlink(host.join("p.json"), inbound.join(PORTS_FILE)).unwrap();
        assert!(read_ports(&inbound).is_empty());
        std::fs::remove_file(inbound.join(PORTS_FILE)).unwrap();
        std::fs::write(inbound.join(PORTS_FILE), "[8080]").unwrap();
        assert_eq!(read_ports(&inbound), BTreeSet::from([8080]));
    }
}
