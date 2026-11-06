//! Links and login callbacks of the macOS guest route, the host side,
//! platform-neutral with Linux tests.
//!
//! A guest program opens a link: the link helper of the guest hands it to
//! the agent, and the agent sends `link.open` (`guest/link.rs`). The
//! session asks with the rules of the Linux route: the mode of `open-url`
//! in `[host]` of `config.toml` (`deny`, `ask` or `allow`, `ask` by
//! default), the approval card of the panel and of `paddock approvals`,
//! "allow for this session" and "deny for this session". Only http and
//! https links pass. The host browser opens an approved link.
//!
//! When the opened link names a callback on a loopback port
//! (`login::callback_ports`, the parser of the Linux route), the session
//! listens on `127.0.0.1:PORT` and `[::1]:PORT` of the host for
//! [`WINDOW`]. Each connection there gets a ticket and becomes a
//! `login.forward` request. The agent connects to the same port on the
//! guest loopback, and only when a guest process listens there it dials
//! the `guest-login` vsock port with the token line and the ticket line.
//! The session then copies the bytes both ways. The listener ends after
//! the window, or [`GRACE`] after the first completed callback: a
//! connection that the guest answered and that then ended. No other port
//! is ever forwarded, and nothing listens on an address that is not a
//! loopback address.

use super::text;
use crate::broker::{Answer, Approver, HostPolicy, Mode, Pending};
use crate::env::HostEnv;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a callback port listens after its login link opened: the
/// window of the Linux route.
pub const WINDOW: Duration = crate::login::WINDOW;
/// After the first completed callback the port listens this much longer,
/// for the page that the app's server redirects the browser to.
pub const GRACE: Duration = Duration::from_secs(10);
/// A host connection waits this long for the dial of the guest.
pub const DIAL_WAIT: Duration = Duration::from_secs(10);
/// The guest dial sends its token and ticket lines within this time.
pub const LINE_WAIT: Duration = Duration::from_secs(5);
/// A forwarded callback that carries no byte for this long closes.
pub const IDLE: Duration = Duration::from_secs(60);
/// The accept loop looks at the window this often.
const TICK: Duration = Duration::from_millis(100);
/// Host connections that wait for a guest dial at most.
pub const MAX_WAITING: usize = 32;
/// Links that wait for an answer of the user at most.
pub const MAX_ASKING: usize = 4;

// ---- The decision on a link ---------------------------------------------

/// What the session does with a link of the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Open,
    Ask,
    Refuse(String),
}

/// The decision on `url` from the mode of `open-url` and the answers of
/// this session: `granted` (allow for this session), `denied` (deny for
/// this session).
pub fn decide(url: &str, mode: Mode, granted: bool, denied: bool) -> Decision {
    if let Err(why) = crate::guest::link::check_url(url) {
        return Decision::Refuse(why);
    }
    match mode {
        Mode::Deny => Decision::Refuse(text::link_mode_deny()),
        _ if denied => Decision::Refuse(text::link_denied_for_session()),
        Mode::Allow => Decision::Open,
        Mode::Ask if granted => Decision::Open,
        Mode::Ask => Decision::Ask,
    }
}

/// Who asks: the app profile of the session.
#[derive(Debug, Clone, Default)]
pub struct Asker {
    /// The profile name, `chatgpt-desktop`.
    pub profile: String,
    /// The name of the app, `ChatGPT`.
    pub label: String,
    /// The profile folder, for the card.
    pub dir: PathBuf,
}

/// The approval card of one link: the card of the Linux route's
/// `open-url`, with the app of the guest as the asker.
pub fn card(id: &str, url: &str, who: &Asker, now: u64, wait: u64) -> Pending {
    Pending {
        id: id.to_string(),
        sandbox: format!("macvm-{}", who.profile), // not user text
        harness: who.profile.clone(),
        project: who.dir.clone(),
        capability: "open-url".into(),
        summary: text::link_card_summary(url),
        detail: text::link_card_detail(url),
        reason: text::link_card_reason(&who.label),
        created: now,
        expires: now + wait,
        session_allowed: true,
        ..Default::default()
    }
}

/// How one link ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// The host browser opened it: the callback ports that listen now.
    Yes(Vec<u16>),
    /// Refused, denied or not answered: the line for the event log.
    No(String),
}

/// The gate of the links of one session.
pub struct Links {
    pub env: HostEnv,
    /// The rules of `[host]`; an unreadable `config.toml` refuses every link.
    pub policy: Result<HostPolicy, String>,
    pub approver: Arc<dyn Approver>,
    pub who: Asker,
    granted: AtomicBool,
    denied: AtomicBool,
    asking: AtomicUsize,
}

impl Links {
    pub fn new(
        env: HostEnv,
        policy: Result<HostPolicy, String>,
        approver: Arc<dyn Approver>,
        who: Asker,
    ) -> Self {
        Links {
            env,
            policy,
            approver,
            who,
            granted: AtomicBool::new(false),
            denied: AtomicBool::new(false),
            asking: AtomicUsize::new(0),
        }
    }

    /// Decide, ask when the mode says so, and open with `open`. Then the
    /// callback ports of the link listen through `callbacks`. Blocks while
    /// the card waits: call it from a thread of its own.
    pub fn handle(
        &self,
        url: &str,
        open: &dyn Fn(&str) -> Result<(), String>,
        callbacks: &Callbacks,
    ) -> Opened {
        let kind = super::proto::link_kind(url);
        let policy = match &self.policy {
            Ok(p) => p,
            Err(e) => return Opened::No(text::link_policy_unreadable(&kind, e)),
        };
        let mode = policy.mode("open-url");
        let granted = self.granted.load(Ordering::SeqCst);
        let denied = self.denied.load(Ordering::SeqCst);
        match decide(url, mode, granted, denied) {
            Decision::Refuse(why) => return Opened::No(text::link_refused(&kind, &why)),
            Decision::Open => {}
            Decision::Ask => {
                if self.asking.fetch_add(1, Ordering::SeqCst) >= MAX_ASKING {
                    self.asking.fetch_sub(1, Ordering::SeqCst);
                    return Opened::No(text::link_too_many_asks(&kind, MAX_ASKING));
                }
                let id = crate::broker::new_request_id();
                let p = card(&id, url, &self.who, crate::state::now(), policy.wait);
                let answer = self
                    .approver
                    .ask(&self.env, &p, Duration::from_secs(policy.wait));
                self.asking.fetch_sub(1, Ordering::SeqCst);
                match answer {
                    Some(Answer::Once) => {}
                    Some(Answer::Session) => self.granted.store(true, Ordering::SeqCst),
                    Some(Answer::Deny) => return Opened::No(text::link_denied(&kind)),
                    Some(Answer::DenySession) => {
                        self.denied.store(true, Ordering::SeqCst);
                        return Opened::No(text::link_denied(&kind));
                    }
                    None => return Opened::No(text::link_unanswered(&kind, policy.wait)),
                }
            }
        }
        if let Err(e) = open(url) {
            return Opened::No(text::link_open_failed(&kind, &e));
        }
        Opened::Yes(callbacks.open_url(url))
    }
}

// ---- The callback windows ------------------------------------------------

/// The times of the forwarder: [`WINDOW`], [`GRACE`] and [`DIAL_WAIT`],
/// shorter in the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub window: Duration,
    pub grace: Duration,
    pub dial_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            window: WINDOW,
            grace: GRACE,
            dial_wait: DIAL_WAIT,
        }
    }
}

/// The callback ports that listen, each with the end of its window. Pure.
#[derive(Debug, Default)]
pub struct Windows {
    pub timing: Timing,
    until: BTreeMap<u16, Instant>,
    /// The ports whose first callback completed: the end moved to the
    /// grace, and a new link does not move it back.
    done: BTreeMap<u16, Instant>,
}

impl Windows {
    pub fn new(timing: Timing) -> Self {
        Windows {
            timing,
            ..Default::default()
        }
    }

    /// Open the window of `port` until `now + window`, or move its end
    /// there. True when the port did not listen before.
    pub fn allow(&mut self, port: u16, now: Instant) -> bool {
        let fresh = !self.active(port, now);
        if fresh {
            self.done.remove(&port);
        }
        if !self.done.contains_key(&port) {
            self.until.insert(port, now + self.timing.window);
        }
        fresh
    }

    /// Whether `port` listens at `now`.
    pub fn active(&self, port: u16, now: Instant) -> bool {
        self.until.get(&port).is_some_and(|u| *u > now)
    }

    /// The first completed callback of `port`: its window ends at
    /// `now + grace`. True the first time.
    pub fn completed(&mut self, port: u16, now: Instant) -> bool {
        if self.done.contains_key(&port) || !self.active(port, now) {
            return false;
        }
        let end = (now + self.timing.grace).min(self.until[&port]);
        self.until.insert(port, end);
        self.done.insert(port, now);
        true
    }

    /// The window of `port` is over: forget it. True once, for the one
    /// accept loop that says so.
    pub fn expire(&mut self, port: u16, now: Instant) -> bool {
        match self.until.get(&port) {
            Some(u) if *u <= now => {
                self.until.remove(&port);
                true
            }
            _ => false,
        }
    }

    /// Whether the window of `port` ended by a completed callback.
    pub fn was_completed(&self, port: u16) -> bool {
        self.done.contains_key(&port)
    }

    /// The ports that listen at `now`.
    pub fn ports(&self, now: Instant) -> Vec<u16> {
        self.until
            .iter()
            .filter(|(_, u)| **u > now)
            .map(|(p, _)| *p)
            .collect()
    }
}

// ---- The forwarder -------------------------------------------------------

/// What the forwarder tells the session, for its event log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Listening {
        port: u16,
        addrs: Vec<SocketAddr>,
    },
    BindFailed {
        port: u16,
        why: String,
    },
    /// A connection waited for the guest in vain.
    NoDial {
        port: u16,
    },
    Forwarded {
        port: u16,
        up: u64,
        down: u64,
    },
    Completed {
        port: u16,
    },
    Closed {
        port: u16,
        completed: bool,
    },
}

impl Note {
    /// The line of the event log.
    pub fn line(&self, timing: &Timing) -> String {
        match self {
            Note::Listening { port, addrs } => text::login_listening(*port, addrs),
            Note::BindFailed { port, why } => text::login_bind_failed(*port, why),
            Note::NoDial { port } => text::login_no_dial(*port, timing.dial_wait),
            Note::Forwarded { port, up, down } => text::login_forwarded(*port, *up, *down),
            Note::Completed { port } => text::login_completed(*port, timing.grace),
            Note::Closed { port, completed } => text::login_closed(*port, *completed),
        }
    }
}

/// A host connection that waits for the guest dial.
struct Waiting {
    conn: TcpStream,
    port: u16,
    at: Instant,
}

struct State {
    windows: Windows,
    waiting: HashMap<String, Waiting>,
}

type Ask = Box<dyn Fn(u16, String) + Send + Sync>;
type Tell = Box<dyn Fn(Note) + Send + Sync>;

struct Inner {
    token: String,
    /// Sends `login.forward` for a port and a ticket.
    ask: Ask,
    tell: Tell,
    state: Mutex<State>,
    /// The addresses to bind: the two loopback addresses, or the test's.
    addrs: Vec<std::net::IpAddr>,
}

/// The callback forwarder of one session. Clones share it.
#[derive(Clone)]
pub struct Callbacks(Arc<Inner>);

impl Callbacks {
    /// `ask` sends a `login.forward` request to the agent, `tell` takes
    /// the lines for the event log.
    pub fn new(
        token: String,
        timing: Timing,
        ask: impl Fn(u16, String) + Send + Sync + 'static,
        tell: impl Fn(Note) + Send + Sync + 'static,
    ) -> Self {
        Callbacks(Arc::new(Inner {
            token,
            ask: Box::new(ask),
            tell: Box::new(tell),
            state: Mutex::new(State {
                windows: Windows::new(timing),
                waiting: HashMap::new(),
            }),
            addrs: vec![Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()],
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The link `url` opened on the host: listen on each loopback callback
    /// port that it names. The ports.
    pub fn open_url(&self, url: &str) -> Vec<u16> {
        let ports = crate::login::callback_ports(url);
        for &p in &ports {
            self.listen(p);
        }
        ports
    }

    /// The ports that listen now.
    pub fn ports(&self) -> Vec<u16> {
        self.lock().windows.ports(Instant::now())
    }

    /// Listen on `port` of the host loopback for the window. A port that
    /// listens already gets the new end of the window.
    pub fn listen(&self, port: u16) {
        if port == 0 {
            return;
        }
        if !self.lock().windows.allow(port, Instant::now()) {
            return;
        }
        let mut bound = Vec::new();
        let mut errors = Vec::new();
        for ip in &self.0.addrs {
            debug_assert!(ip.is_loopback());
            if !ip.is_loopback() {
                continue;
            }
            match bind(SocketAddr::new(*ip, port)) {
                Ok(l) => bound.push(l),
                Err(e) => errors.push(format!("{ip}: {e}")), // not user text
            }
        }
        if bound.is_empty() {
            self.lock().windows.until.remove(&port);
            (self.0.tell)(Note::BindFailed {
                port,
                why: errors.join(", "),
            });
            return;
        }
        let addrs = bound.iter().filter_map(|l| l.local_addr().ok()).collect();
        (self.0.tell)(Note::Listening { port, addrs });
        for l in bound {
            let me = self.clone();
            let _ = std::thread::Builder::new()
                .name("login-callback".into())
                .spawn(move || me.accept_loop(l, port));
        }
    }

    fn accept_loop(&self, listener: TcpListener, port: u16) {
        let _ = listener.set_nonblocking(true);
        loop {
            let now = Instant::now();
            let late = {
                let mut st = self.lock();
                if !st.windows.active(port, now) {
                    let completed = st.windows.was_completed(port);
                    let first = st.windows.expire(port, now);
                    // The connections of the port that wait go with it.
                    st.waiting.retain(|_, w| w.port != port);
                    drop(st);
                    if first {
                        (self.0.tell)(Note::Closed { port, completed });
                    }
                    return;
                }
                let before = st.waiting.len();
                let wait = st.windows.timing.dial_wait;
                st.waiting
                    .retain(|_, w| !(w.port == port && now.duration_since(w.at) >= wait));
                before - st.waiting.len()
            };
            for _ in 0..late {
                (self.0.tell)(Note::NoDial { port });
            }
            match listener.accept() {
                Ok((conn, peer)) => self.accepted(conn, peer, port),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(TICK),
                Err(e) => {
                    log::warn!("macvm: login callback port {port}: accept failed: {e}");
                    std::thread::sleep(TICK);
                }
            }
        }
    }

    fn accepted(&self, conn: TcpStream, peer: SocketAddr, port: u16) {
        if !peer.ip().is_loopback() || conn.set_nonblocking(false).is_err() {
            return;
        }
        let ticket = super::session::random_hex(16);
        {
            let mut st = self.lock();
            if st.waiting.len() >= MAX_WAITING {
                log::warn!("macvm: login callback port {port}: {MAX_WAITING} connections wait, refusing one");
                return;
            }
            st.waiting.insert(
                ticket.clone(),
                Waiting {
                    conn,
                    port,
                    at: Instant::now(),
                },
            );
        }
        (self.0.ask)(port, ticket);
    }

    /// One dial of the guest on the `guest-login` port: the token line,
    /// the ticket line, then the bytes of the waiting host connection. A
    /// wrong token or an unknown ticket closes it without a byte.
    pub fn guest_dial<S: crate::proxy::Duplex>(&self, mut stream: S) {
        let _ = stream.set_tick(LINE_WAIT);
        if !super::security::token_line_ok(&mut stream, &self.0.token) {
            log::warn!("macvm: a guest-login connection sent a wrong token");
            return;
        }
        let ticket = match super::proto::read_raw_line(&mut stream, 128) {
            Ok(Some(t)) => t,
            _ => return,
        };
        let w = {
            let mut st = self.lock();
            let Some(w) = st.waiting.remove(&ticket) else {
                drop(st);
                log::warn!("macvm: a guest-login connection named an unknown ticket");
                return;
            };
            if !st.windows.active(w.port, Instant::now()) {
                return;
            }
            w
        };
        let port = w.port;
        let (up, down) = crate::proxy::splice(w.conn, stream, IDLE);
        (self.0.tell)(Note::Forwarded { port, up, down });
        if down > 0 && self.lock().windows.completed(port, Instant::now()) {
            (self.0.tell)(Note::Completed { port });
        }
    }

    /// The times of this forwarder.
    pub fn timing(&self) -> Timing {
        self.lock().windows.timing
    }

    /// Host connections that wait for a guest dial now.
    pub fn waiting(&self) -> usize {
        self.lock().waiting.len()
    }
}

/// A listener on one loopback address. A port that a host program holds
/// fails here and is named in the log; a port whose last listener of this
/// session ends in this moment gets three tries.
fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            text::login_not_loopback(),
        ));
    }
    let mut tries = 0;
    loop {
        match TcpListener::bind(addr) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse && tries < 2 => {
                tries += 1;
                std::thread::sleep(TICK * 2);
            }
            r => return r,
        }
    }
}

#[cfg(test)]
#[path = "login_route_tests.rs"]
mod tests;
