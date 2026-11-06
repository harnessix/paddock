//! The route of the guided state of `paddock macvm setup`,
//! platform-neutral: who makes the guest account, whether the agent is on
//! the disk already, and who turns on its Accessibility permission.
//!
//! - The account: the provisioning options of the macOS 27 Virtualization
//!   framework (`VZMacGuestProvisioningOptions`) make it on the first boot
//!   after the install and log it in, with no Setup Assistant. A guest
//!   older than 27, a host older than 27, a framework without the class or
//!   a used first boot leave it to the person in Setup Assistant.
//! - The agent: the offline seed of the `Seed` state (`offline.rs`) put it
//!   on the disk with a first-boot daemon; else the person types the seed
//!   command in Terminal of the guest.
//! - Accessibility: the `Trust` state sets it offline in the TCC database
//!   (`tcc.rs`) when this Mac can mount the guest disk; else the person
//!   turns the switch on in the window.
//!
//! The first boot of the golden is recorded in `events.jsonl` (kind
//! [`FIRST_BOOT`]) with `provisioned` or `plain`, after the start: the
//! guest reads the options on that boot only. `PADDOCK_MACVM_GUIDED=1`
//! forces every step to the person. The password goes only into the
//! options object; [`check_options`] refuses the values that macOS
//! refuses, before the boot, so a refusal never uses up the first boot.

use super::setup_events::{self, Line, STATE};

/// The event kind of the first boot of a golden.
pub const FIRST_BOOT: &str = "first boot";
/// The event kind of the route of the guided state.
pub const ROUTE: &str = "route";
/// The full name and the account name of the provisioned account.
pub const FULL_NAME: &str = "paddock";
/// The longest password that macOS takes in the options, in bytes.
pub const PASSWORD_MAX_BYTES: usize = 511;
/// The first macOS major with the provisioning options, host and guest.
pub const FIRST_MAJOR: u32 = 27;

/// How the first boot of a golden went, from its event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstBoot {
    Unused,
    Provisioned,
    Plain,
}

/// The first boot from the event lines. A golden of an earlier paddock
/// has no such line: a second `guided starts`, or a configure or seal
/// start, proves an earlier boot.
pub fn first_boot(lines: &[Line]) -> FirstBoot {
    if let Some(l) = lines.iter().find(|l| l.kind == FIRST_BOOT) {
        return if l.text.starts_with("provisioned") {
            FirstBoot::Provisioned
        } else {
            FirstBoot::Plain
        };
    }
    let starts = |s: &str| {
        lines
            .iter()
            .filter(|l| l.kind == STATE && l.text == format!("{s} starts")) // not user text
            .count()
    };
    if starts("guided") > 1 || starts("configure") > 0 || starts("seal") > 0 {
        FirstBoot::Plain
    } else {
        FirstBoot::Unused
    }
}

/// Record the first boot, once.
pub fn record_first_boot(events: &std::path::Path, provisioned: bool) {
    if setup_events::read(events)
        .iter()
        .any(|l| l.kind == FIRST_BOOT)
    {
        return;
    }
    let how = if provisioned { "provisioned" } else { "plain" };
    setup_events::record(events, FIRST_BOOT, false, how);
}

/// Why a step of the guided state is left to the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// `PADDOCK_MACVM_GUIDED=1`.
    Forced,
    /// The host is older than macOS 27: its version.
    HostOld(String),
    /// The guest is older than macOS 27: its version.
    GuestOld(String),
    /// The framework lacks a class or a method: its name.
    Missing(String),
    /// The first boot was plain: the guest ignores the options now.
    FirstBootUsed,
    /// macOS refused the options: its error text.
    Refused(String),
}

/// Who makes the guest account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    /// Boot with the provisioning options.
    Provision,
    /// The first boot was provisioned: the account is there.
    Made,
    /// The person, in Setup Assistant.
    Person(Why),
}

/// The route of the guided state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub account: Account,
    /// The offline seed put the agent on the disk: no typed command.
    pub seeded: bool,
    /// The guided state ends at the agent's first health report, and the
    /// `Trust` state sets the permission offline. False: the person
    /// turns the switch on in the window.
    pub trust_offline: bool,
}

impl Route {
    /// The person does every step: the guided window of old.
    pub fn guided(why: Why) -> Route {
        Route {
            account: Account::Person(why),
            seeded: false,
            trust_offline: false,
        }
    }

    /// One word for the event log and the tests: `unattended` when no
    /// step waits on the person.
    pub fn word(&self) -> &'static str {
        match (&self.account, self.seeded && self.trust_offline) {
            (Account::Provision | Account::Made, true) => "unattended",
            (Account::Provision | Account::Made, false) => "provision",
            (Account::Person(_), true) => "offline",
            (Account::Person(_), false) => "guided",
        }
    }

    /// Why the person makes the account, when they do.
    pub fn why(&self) -> Option<&Why> {
        match &self.account {
            Account::Person(w) => Some(w),
            _ => None,
        }
    }
}

/// What the decision needs.
#[derive(Debug, Clone)]
pub struct Inputs<'a> {
    pub forced_guided: bool,
    /// `sw_vers -productVersion` of the host.
    pub host: &'a str,
    /// The macOS version of the golden.
    pub guest: &'a str,
    /// The framework check of the backend: `Err(name)` of what is missing.
    pub support: Result<(), String>,
    pub first_boot: FirstBoot,
    /// The offline seed of this golden is done (`seed offline` event).
    pub seeded: bool,
    /// The backend can mount the guest disk for the offline trust.
    pub disk_tool: bool,
}

/// The major of a version text such as `27.0.1`.
pub fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse().ok()
}

/// Decide the route.
pub fn decide(i: &Inputs) -> Route {
    if i.forced_guided {
        return Route {
            seeded: i.seeded,
            ..Route::guided(Why::Forced)
        };
    }
    let account = if i.first_boot == FirstBoot::Plain {
        Account::Person(Why::FirstBootUsed)
    } else if major(i.guest).is_none_or(|m| m < FIRST_MAJOR) {
        Account::Person(Why::GuestOld(i.guest.to_string()))
    } else if i.first_boot == FirstBoot::Provisioned {
        Account::Made
    } else if major(i.host).is_none_or(|m| m < FIRST_MAJOR) {
        Account::Person(Why::HostOld(i.host.to_string()))
    } else if let Err(name) = &i.support {
        Account::Person(Why::Missing(name.clone()))
    } else {
        Account::Provision
    };
    Route {
        account,
        seeded: i.seeded,
        trust_offline: i.disk_tool,
    }
}

/// Whether the event log says that the offline seed of the disk is done.
pub fn seeded(lines: &[Line]) -> bool {
    lines
        .iter()
        .rev()
        .find(|l| l.kind == setup_events::SEED_OFFLINE)
        .is_some_and(|l| l.text.starts_with("done"))
}

/// The checks of the options before the boot: the values that macOS
/// refuses. The error never holds the password.
pub fn check_options(full_name: &str, user: &str, password: &str) -> Result<(), Why> {
    let refuse = |what: &str| Err(Why::Refused(what.to_string()));
    if full_name.is_empty() {
        return refuse("full name");
    }
    if user.is_empty() || !user.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return refuse("user name");
    }
    if password.is_empty() || password.len() > PASSWORD_MAX_BYTES || password.contains('\0') {
        return refuse("password");
    }
    Ok(())
}

/// A password that never shows in a `Debug` print.
pub struct Secret(String);

impl Secret {
    pub fn new(s: &str) -> Self {
        Secret(s.to_string())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(•••)")
    }
}

/// Replace every copy of the secret in a text: an error of macOS that
/// quotes the options must not show the password.
pub fn redact(text: &str, secret: &Secret) -> String {
    if secret.0.is_empty() {
        return text.to_string();
    }
    text.replace(&secret.0, "•••")
}

#[cfg(test)]
#[path = "provision_tests.rs"]
mod tests;
