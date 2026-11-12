//! Connections admitted by the firewall and tracked until they close.

use super::super::connlog::{self, Entry};
use super::super::firewall::{self, Conn, Decision, Mode, Rule};
use super::super::proxy_header::Identity;
use crate::proxy::allow::Allowlist;
use std::path::PathBuf;
use std::time::Instant;

pub(super) struct Held {
    pub(super) stream: std::os::unix::net::UnixStream,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) id: Option<Identity>,
    pub(super) require_allow: bool,
}

impl Held {
    fn decision(&self, mode: Mode, api: Option<&Allowlist>, rules: &[Rule]) -> Decision {
        let name = self
            .id
            .as_ref()
            .map(Identity::app_name)
            .unwrap_or_else(|| "unknown".into());
        let conn = match &self.id {
            Some(id) => Conn::of(id, &name, &self.host, self.port),
            None => Conn::new(&name, &self.host, self.port),
        };
        firewall::decide(mode, api, rules, &conn)
    }

    pub(super) fn denied(&self, mode: Mode, api: Option<&Allowlist>, rules: &[Rule]) -> bool {
        matches!(self.decision(mode, api, rules), Decision::Deny(_))
    }

    pub(super) fn rejected_at_hold(
        &self,
        mode: Mode,
        api: Option<&Allowlist>,
        rules: &[Rule],
    ) -> bool {
        match self.decision(mode, api, rules) {
            Decision::Deny(_) => true,
            Decision::Ask => self.require_allow,
            Decision::Allow(_) => false,
        }
    }
}

/// An allowed connection: its log line waits for the close.
#[derive(Debug)]
pub struct Opened {
    pub(super) entry: Entry,
    pub(super) log: PathBuf,
    pub(super) started: Instant,
    pub(super) require_allow: bool,
}

impl Opened {
    pub fn close(self, up: u64, down: u64) {
        let secs = self.started.elapsed().as_secs();
        let e = self.entry.closed(up, down, secs);
        if let Err(err) = connlog::record_at(&self.log, e) {
            log::debug!("macvm: cannot write the connection log: {err}");
        }
    }
}
