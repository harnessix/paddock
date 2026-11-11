//! The tunnel of the proxy: two streams spliced both ways with an idle
//! timer, and the registry of open tunnels, so the wake handler of the
//! macOS guest route closes the tunnels that died with the host's sleep
//! (the app reconnects, as every laptop app does).

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// A stream the proxy can copy both ways: it clones, half closes and
/// takes a read timeout.
pub trait Duplex: Read + Write + Send + 'static {
    fn dup(&self) -> std::io::Result<Box<dyn Duplex>>;
    fn close_write(&self);
    fn set_tick(&self, d: Duration) -> std::io::Result<()>;
}

impl Duplex for TcpStream {
    fn dup(&self) -> std::io::Result<Box<dyn Duplex>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn close_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
    fn set_tick(&self, d: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(d))
    }
}

impl Duplex for Box<dyn Duplex> {
    fn dup(&self) -> std::io::Result<Box<dyn Duplex>> {
        (**self).dup()
    }
    fn close_write(&self) {
        (**self).close_write()
    }
    fn set_tick(&self, d: Duration) -> std::io::Result<()> {
        (**self).set_tick(d)
    }
}

impl Duplex for UnixStream {
    fn dup(&self) -> std::io::Result<Box<dyn Duplex>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn close_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
    fn set_tick(&self, d: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(d))
    }
}

/// One open tunnel: when it last carried a byte, and whether the host
/// asked it to close.
pub struct Tunnel {
    last: Mutex<Instant>,
    closed: AtomicBool,
}

impl Tunnel {
    fn idle_for(&self) -> Duration {
        self.last.lock().map(|l| l.elapsed()).unwrap_or_default()
    }
}

/// Every open tunnel of this process.
static TUNNELS: Mutex<Vec<Weak<Tunnel>>> = Mutex::new(Vec::new());

fn register() -> Arc<Tunnel> {
    let t = Arc::new(Tunnel {
        last: Mutex::new(Instant::now()),
        closed: AtomicBool::new(false),
    });
    let mut all = TUNNELS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|w| w.strong_count() > 0);
    all.push(Arc::downgrade(&t));
    t
}

/// The number of open tunnels.
pub fn open_tunnels() -> usize {
    let mut all = TUNNELS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|w| w.strong_count() > 0);
    all.len()
}

/// Close every tunnel with no traffic for `idle` or longer. Returns how
/// many. Each pump sees the request at its next tick (200 ms).
pub fn close_tunnels_idle_for(idle: Duration) -> usize {
    let all = TUNNELS.lock().unwrap_or_else(|e| e.into_inner());
    let mut n = 0;
    for t in all.iter().filter_map(Weak::upgrade) {
        if t.idle_for() >= idle && !t.closed.swap(true, Ordering::SeqCst) {
            n += 1;
        }
    }
    n
}

/// The wake handler: a tunnel that carried nothing in the last second
/// slept through the host's sleep and its peer is gone. Close it, so the
/// app reconnects instead of waiting for a dead socket.
pub fn close_idle_tunnels() -> usize {
    close_tunnels_idle_for(Duration::from_secs(1))
}

/// Copy one direction until it ends. Returns the bytes copied.
fn pump(
    mut from: Box<dyn Duplex>,
    mut to: Box<dyn Duplex>,
    tunnel: Arc<Tunnel>,
    idle: Duration,
) -> u64 {
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        if tunnel.closed.load(Ordering::SeqCst) {
            break;
        }
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                total += n as u64;
                if let Ok(mut l) = tunnel.last.lock() {
                    *l = Instant::now();
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if tunnel.idle_for() >= idle {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    to.close_write();
    total
}

/// Copy both directions until one side closes, until the tunnel has no
/// traffic in either direction for `idle`, or until the host closes it.
/// Long-lived connections such as WebSockets stay open while they carry
/// traffic. Returns the bytes from `a` to `b` and from `b` to `a`.
pub(crate) fn splice(a: impl Duplex, b: impl Duplex, idle: Duration) -> (u64, u64) {
    let tick = Duration::from_millis(200).min(idle);
    let _ = a.set_tick(tick);
    let _ = b.set_tick(tick);
    let (Ok(a_read), Ok(a_write), Ok(b_read), Ok(b_write)) = (a.dup(), a.dup(), b.dup(), b.dup())
    else {
        return (0, 0);
    };
    let tunnel = register();
    let t2 = tunnel.clone();
    let t = std::thread::spawn(move || pump(a_read, b_write, t2, idle));
    let back = pump(b_read, a_write, tunnel, idle);
    let forth = t.join().unwrap_or(0);
    (forth, back)
}
