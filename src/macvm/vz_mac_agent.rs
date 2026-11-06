//! The control line to the guest agent on the host side, macOS only: the
//! `VZVirtioSocketListener` of the guest ports, the reader thread of the
//! control connection, the forward of the `guest-proxy` port to a unix
//! socket after the token line, a handler per other port (the
//! `guest-files` receiver of `files_port`), and [`Agent`], the writer with
//! the request ids and the timeouts.
//!
//! The session shim (`run.rs`) and the golden setup (`vz_mac.rs`) share
//! it: both listen for the agent's `hello` and send requests.

use super::proto::{self, Call, Request};
use super::text;
use crate::vm::bridge;
use crate::vm::ports;
use crate::vm::vz::dup_fd;
use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_virtualization::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use super::security::TOKEN_LINE_MAX;
use super::security::{self, ControlGate};

/// What the control line reports to its owner.
pub enum AgentEvent {
    /// A new control connection from the guest, with its writer.
    Connected(std::fs::File),
    /// One message of the guest.
    Message(proto::Event),
    /// The control connection closed.
    Gone,
    /// A `guest-files` transfer landed in the profile.
    File {
        kind: proto::FileKind,
        name: String,
        path: PathBuf,
    },
    /// A `guest-files` transfer was refused or closed, with the line.
    FileRefused(String),
}

/// Where the events of the control line go.
pub type Sink = Arc<dyn Fn(AgentEvent) + Send + Sync>;
/// The owner of a guest connection on one port.
pub type Handler = Arc<dyn Fn(OwnedFd) + Send + Sync>;

struct ListenIvars {
    sink: Sink,
    token: String,
    /// One control connection per session (checklist item 13).
    gate: ControlGate,
    /// Where each other port goes: a unix socket per port.
    forwards: BTreeMap<u32, PathBuf>,
    /// The ports with a handler of their own.
    handlers: BTreeMap<u32, Handler>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AllocAnyThread]
    #[name = "PaddockGuestListener"]
    #[ivars = ListenIvars]
    struct ListenDelegate;

    unsafe impl NSObjectProtocol for ListenDelegate {}

    unsafe impl VZVirtioSocketListenerDelegate for ListenDelegate {
        #[unsafe(method(listener:shouldAcceptNewConnection:fromSocketDevice:))]
        fn should_accept(
            &self,
            _listener: &VZVirtioSocketListener,
            conn: &VZVirtioSocketConnection,
            _device: &VZVirtioSocketDevice,
        ) -> Bool {
            let port = unsafe { conn.destinationPort() };
            let fd = match dup_fd(unsafe { conn.fileDescriptor() }) {
                Ok(fd) => fd,
                Err(e) => {
                    log::warn!("macvm: guest connection to port {port}: {e}");
                    return Bool::NO;
                }
            };
            let iv = self.ivars();
            if port == ports::GUEST_CONTROL {
                if !iv.gate.open() {
                    log::warn!("macvm: a second guest control connection was refused");
                    return Bool::NO;
                }
                control_connection(fd, iv.sink.clone(), iv.gate.clone());
                return Bool::YES;
            }
            if let Some(path) = iv.forwards.get(&port) {
                token_forward(fd, iv.token.clone(), path.clone());
                return Bool::YES;
            }
            if let Some(handler) = iv.handlers.get(&port) {
                handler(fd);
                return Bool::YES;
            }
            Bool::NO
        }
    }
);

/// The listener of the guest ports and its delegate. Keep it alive for
/// the life of the VM: the framework holds it weakly.
#[derive(Clone)]
pub struct Listener(Retained<VZVirtioSocketListener>, Retained<ListenDelegate>);

// SAFETY: the listener is installed on the queue of the VM and used from
// that queue only; the delegate's state is immutable after `new`.
unsafe impl Send for Listener {}
unsafe impl Sync for Listener {}

impl Listener {
    /// A listener for the control port, every port of `forwards` and
    /// every port of `handlers`.
    pub fn new(
        sink: Sink,
        token: String,
        forwards: BTreeMap<u32, PathBuf>,
        handlers: BTreeMap<u32, Handler>,
    ) -> Self {
        let this = ListenDelegate::alloc().set_ivars(ListenIvars {
            sink,
            token,
            gate: ControlGate::new(),
            forwards,
            handlers,
        });
        let delegate: Retained<ListenDelegate> = unsafe { msg_send![super(this), init] };
        Self(unsafe { VZVirtioSocketListener::new() }, delegate)
    }

    /// Put the listener on the socket device of `vm`. Call it on the
    /// queue of the VM.
    pub fn install(&self, vm: &VZVirtualMachine) {
        let Some(dev) = socket_device(vm) else {
            return;
        };
        unsafe { self.0.setDelegate(Some(ProtocolObject::from_ref(&*self.1))) };
        let mut ports = vec![ports::GUEST_CONTROL];
        ports.extend(self.1.ivars().forwards.keys().copied());
        ports.extend(self.1.ivars().handlers.keys().copied());
        for port in ports {
            unsafe { dev.setSocketListener_forPort(&self.0, port) };
        }
    }
}

/// The first virtio socket device of a VM.
pub fn socket_device(vm: &VZVirtualMachine) -> Option<Retained<VZVirtioSocketDevice>> {
    let first = unsafe { vm.socketDevices() }.firstObject()?;
    first.downcast::<VZVirtioSocketDevice>().ok()
}

/// The guest control connection: the reader thread turns each line into
/// an event, the writer goes to the owner. The gate opens again when the
/// connection ends.
fn control_connection(fd: OwnedFd, sink: Sink, gate: ControlGate) {
    std::thread::spawn(move || {
        let reader = std::fs::File::from(fd);
        let Ok(writer) = reader.try_clone() else {
            gate.close();
            return;
        };
        sink(AgentEvent::Connected(writer));
        let mut reader = reader;
        loop {
            match proto::read_raw_line(&mut reader, proto::LINE_MAX) {
                Ok(Some(line)) => match serde_json::from_str::<proto::Event>(&line) {
                    Ok(ev) => sink(AgentEvent::Message(ev)),
                    Err(e) => log::warn!("macvm: a guest line is not a protocol event: {e}"),
                },
                Ok(None) => break,
                Err(e) => {
                    log::warn!("macvm: the guest control connection: {e:#}");
                    break;
                }
            }
        }
        gate.close();
        sink(AgentEvent::Gone);
    });
}

/// A guest connection that starts with the token line, then carries the
/// bytes of `path`. A wrong token closes it.
fn token_forward(fd: OwnedFd, token: String, path: PathBuf) {
    std::thread::spawn(move || {
        let mut f = std::fs::File::from(match fd.try_clone() {
            Ok(c) => c,
            Err(_) => return,
        });
        if !security::token_line_ok(&mut f, &token) {
            log::warn!("macvm: a guest connection sent a wrong token");
            return;
        }
        match std::os::unix::net::UnixStream::connect(&path) {
            Ok(host) => {
                let _ = bridge::splice(fd, host.into());
            }
            Err(e) => log::warn!("macvm: cannot connect to {}: {e}", path.display()),
        }
    });
}

/// A request that waits for its reply.
#[derive(Debug, Clone)]
pub struct Pending {
    pub kind: String,
    pub sent: Instant,
    pub deadline: Instant,
}

/// The writer of the control connection with the request ids: every
/// request gets the next id and its timeout from `proto::timeout`.
pub struct Agent {
    writer: Arc<Mutex<std::fs::File>>,
    next_req: u64,
    pending: BTreeMap<u64, Pending>,
}

impl Agent {
    pub fn new(writer: std::fs::File) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
            next_req: 1,
            pending: BTreeMap::new(),
        }
    }

    /// Send one request. The id, for the reply.
    pub fn send(&mut self, request: Request) -> Result<u64> {
        let req = self.next_req;
        self.next_req += 1;
        let kind = request.kind();
        let timeout = proto::timeout(&request);
        let call = Call { req, request };
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        proto::write_line(&mut *w, &call).with_context(|| text::host_write_failed(&kind))?;
        w.flush().with_context(|| text::host_write_failed(&kind))?;
        log::debug!("macvm: sent {}", call.kind());
        let now = Instant::now();
        self.pending.insert(
            req,
            Pending {
                kind,
                sent: now,
                deadline: now + timeout,
            },
        );
        Ok(req)
    }

    /// The pending request of a reply, when it is one of ours.
    pub fn take(&mut self, req: u64) -> Option<Pending> {
        self.pending.remove(&req)
    }

    /// The requests whose timeout passed, removed from the table.
    pub fn expired(&mut self, now: Instant) -> Vec<(u64, Pending)> {
        let gone: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(r, _)| *r)
            .collect();
        gone.into_iter()
            .filter_map(|r| self.pending.remove(&r).map(|p| (r, p)))
            .collect()
    }

    /// Give one pending request the deadline `at`, when that is later
    /// than its own: the guest said that it needs longer.
    pub fn extend(&mut self, req: u64, at: Instant) {
        if let Some(p) = self.pending.get_mut(&req) {
            p.deadline = p.deadline.max(at);
        }
    }

    /// Move every deadline by `by`: the VM was paused for that long
    /// (`paddock freeze`), so the requests did not wait for the guest.
    pub fn delay(&mut self, by: Duration) {
        for p in self.pending.values_mut() {
            p.deadline += by;
        }
    }

    /// How long a pending request waited, for the log.
    pub fn waited(p: &Pending) -> Duration {
        p.deadline.saturating_duration_since(p.sent)
    }
}
