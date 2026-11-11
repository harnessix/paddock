//! The links and the login callbacks of a session, macOS only: the
//! `link.open` event of the agent, the approval and the open in the host
//! browser in a thread of their own, the `guest-login` port, and the
//! `login.forward` requests of the forwarder. The rules and the forwarder
//! are platform-neutral in `login_route.rs`. `run.rs` holds the session.

use super::events::Events;
use super::login_route::{Asker, Callbacks, Links, Opened, Timing};
use super::proto::{self, Request};
use super::run::{Event, Session, Why};
use super::text;
use super::vz_mac_agent::Handler;
use crate::env::HostEnv;
use crate::vm::helper::AppPlan;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// The check mode of `macvm-session.sh login`: with this set to 1 in a
/// `--bench` session, an approved link opens no host browser; the
/// callback port listens as usual.
pub const DRY_VAR: &str = "PADDOCK_MACVM_LINK_DRY";

/// The links and the callbacks of one session.
pub(super) struct Login {
    pub(super) callbacks: Callbacks,
    links: Arc<Links>,
    dry: bool,
    log: Arc<Events>,
}

impl Login {
    pub(super) fn new(env: &HostEnv, plan: &AppPlan, token: &str, tx: Sender<Event>) -> Self {
        let log = Arc::new(Events::open(&plan.profile));
        let ask_tx = tx.clone();
        let tell_log = log.clone();
        let callbacks = Callbacks::new(
            token.to_string(),
            Timing::default(),
            move |port, ticket| {
                let _ = ask_tx.send(Event::LoginForward(port, ticket));
            },
            move |note| {
                let line = note.line(&Timing::default());
                log::info!("macvm: {line}");
                tell_log.record("login", &line, &[]);
            },
        );
        let profile = plan
            .profile
            .dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let who = Asker {
            profile,
            label: plan.app.clone(),
            dir: plan.profile.dir.clone(),
        };
        let mut policy = crate::broker::HostPolicy::load(env).map_err(|e| format!("{e:#}"));
        // A link that the app of a guest session opens is the person's own
        // click in that app (a sign-in, a help page): it opens in the host
        // browser without a card, as on the Mac itself, unless `[host]
        // open-url` says otherwise. A card that nobody sees left every
        // sign-in waiting.
        if let Ok(p) = &mut policy {
            if !crate::broker::HostPolicy::configured(env, "open-url") {
                p.set_mode("open-url", crate::broker::Mode::Allow);
            }
        }
        let links = Links::new(
            env.clone(),
            policy,
            Arc::new(crate::broker::HostApprover),
            who,
        );
        let dry = plan.bench && env.var(DRY_VAR) == Some("1");
        Login {
            callbacks,
            links: Arc::new(links),
            dry,
            log,
        }
    }

    /// The handler of the `guest-login` port: one thread per dial.
    pub(super) fn handler(&self) -> Handler {
        let callbacks = self.callbacks.clone();
        Arc::new(move |fd: OwnedFd| {
            let callbacks = callbacks.clone();
            std::thread::spawn(move || callbacks.guest_dial(UnixStream::from(fd)));
        })
    }

    /// A link of the guest: decide, ask, open, listen. The card may wait
    /// for minutes, so a thread does it.
    fn opened(&self, url: String) {
        let (links, callbacks, log, dry) = (
            self.links.clone(),
            self.callbacks.clone(),
            self.log.clone(),
            self.dry,
        );
        std::thread::spawn(move || {
            let kind = proto::link_kind(&url);
            let open = |u: &str| -> Result<(), String> {
                if dry {
                    log.record("link", &text::link_dry(&kind), &[]);
                    return Ok(());
                }
                crate::gatewayd::open_url(&links.env, u, &Default::default())
                    .map_err(|e| format!("{e:#}"))
            };
            let line = match links.handle(&url, &open, &callbacks) {
                Opened::Yes(ports) => text::link_opened(&kind, &ports),
                Opened::No(line) => line,
            };
            log::info!("macvm: {line}");
            log.record("link", &line, &[]);
        });
    }
}

impl Session<'_> {
    /// `link.open` of the agent.
    pub(super) fn link_open(&mut self, url: String) {
        self.login.opened(url);
    }

    /// The forwarder took a connection of a callback port: the agent
    /// connects to the guest port and dials back.
    pub(super) fn login_forward(&mut self, port: u16, ticket: String) {
        self.send(Request::LoginForward { port, ticket }, Why::Other);
    }
}
