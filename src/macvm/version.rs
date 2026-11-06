//! The version logic of the macOS guest route: the protocol
//! compatibility, what the host does with the `hello` of an agent of
//! another version, what the `hello` after an `agent.update` means, and
//! the agent folder of one paddock version. Pure: the tests run on every
//! system.

use super::layout::safe_name;
use super::text;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;

/// A protocol version of the control line. Equal majors talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

/// The protocol version, under the name the peers use.
pub type Protocol = Version;

impl Version {
    /// True when the two sides can talk: the same major.
    pub fn talks(self, other: Version) -> bool {
        self.major == other.major
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The paddock version that the host and the agent report in `hello` and
/// `token`. `PADDOCK_MACVM_VERSION` at build time replaces it, so that the
/// Mac checks build a second agent with a bumped version from the same
/// source.
pub fn paddock_version() -> &'static str {
    match option_env!("PADDOCK_MACVM_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
}

/// Two protocol versions talk when their majors are equal, whatever the
/// minors.
pub fn compatible(a: Protocol, b: Protocol) -> bool {
    a.major == b.major
}

/// One side of the `hello`: its paddock version and its protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer<'a> {
    pub paddock: &'a str,
    pub protocol: Protocol,
}

/// What the host does after a `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The versions match: send `token` and go on.
    Talk,
    /// Send `agent.update` first; the new agent dials in with a new
    /// `hello`. A downgrade of paddock runs the same way: the host
    /// version wins.
    Update,
    /// End the session with this line: the agent speaks another major
    /// and cannot update itself.
    Refuse(String),
}

/// What the host does with the `hello` of an agent. Equal versions talk.
/// Any other agent that can update itself updates, newer or older. An
/// agent that cannot update talks when the majors are equal, and is
/// refused with the line that names `paddock macvm update` otherwise.
pub fn agent_action(host: Peer, agent: Peer, can_update: bool) -> Action {
    if host.paddock == agent.paddock && host.protocol == agent.protocol {
        return Action::Talk;
    }
    if can_update {
        return Action::Update;
    }
    if compatible(host.protocol, agent.protocol) {
        return Action::Talk;
    }
    Action::Refuse(text::agent_cannot_update(
        agent.paddock,
        &agent.protocol.to_string(),
        host.paddock,
        &host.protocol.to_string(),
    ))
}

/// What the `hello` after an `agent.update` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Followup {
    /// The new agent runs: its version is the host's.
    Updated,
    /// The update did not take: the `launch` script fell back to `prev/`
    /// after three fast failures, or the new agent never started and the
    /// old one is back. The host does not update again in this session,
    /// or the two would loop.
    RolledBack,
}

/// The meaning of the version in the `hello` that follows an update to
/// `host`.
pub fn after_update(host: &str, now: &str) -> Followup {
    if now == host {
        Followup::Updated
    } else {
        Followup::RolledBack
    }
}

/// How two paddock versions relate, for the lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Same,
    Upgrade,
    Downgrade,
    /// A version that is not `major.minor.patch[-pre]`.
    Unknown,
}

/// `direction("0.9.0", "0.10.0")` is `Upgrade`. A pre-release suffix
/// sorts before its release (`0.9.0-rc1 < 0.9.0`), as in semver.
pub fn direction(from: &str, to: &str) -> Direction {
    let (Some(a), Some(b)) = (parse(from), parse(to)) else {
        return Direction::Unknown;
    };
    match a.cmp(&b) {
        Ordering::Equal => Direction::Same,
        Ordering::Less => Direction::Upgrade,
        Ordering::Greater => Direction::Downgrade,
    }
}

/// `(numbers, pre-release)`: a pre-release is `Some` and sorts first, so
/// the key of a release is `(numbers, None)` and `None > Some` would be
/// wrong. The key uses `(numbers, is_release, pre)`.
fn parse(v: &str) -> Option<(Vec<u64>, bool, String)> {
    let v = v.trim().trim_start_matches('v');
    let (nums, pre) = match v.split_once('-') {
        Some((n, p)) => (n, Some(p)),
        None => (v, None),
    };
    let numbers: Option<Vec<u64>> = nums.split('.').map(|p| p.parse().ok()).collect();
    let numbers = numbers?;
    if numbers.is_empty() || numbers.len() > 4 {
        return None;
    }
    Some((numbers, pre.is_none(), pre.unwrap_or("").to_string()))
}

/// `agent/<version>/` under the `macvm` folder: the share of one paddock
/// version. The version is one path component.
pub fn agent_dir(paddock_version: &str) -> PathBuf {
    PathBuf::from("agent").join(safe_name(paddock_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    const P1_0: Protocol = Protocol { major: 1, minor: 0 };
    const P1_3: Protocol = Protocol { major: 1, minor: 3 };
    const P2_0: Protocol = Protocol { major: 2, minor: 0 };

    fn peer(paddock: &str, protocol: Protocol) -> Peer<'_> {
        Peer { paddock, protocol }
    }

    #[test]
    fn agent_action_cases() {
        let host = peer("0.9.0", P1_3);
        // Equal versions talk, with or without the update ability.
        assert_eq!(agent_action(host, peer("0.9.0", P1_3), true), Action::Talk);
        assert_eq!(agent_action(host, peer("0.9.0", P1_3), false), Action::Talk);
        // A newer host updates the agent.
        assert_eq!(
            agent_action(host, peer("0.8.0", P1_0), true),
            Action::Update
        );
        // An older host updates too: a downgrade of paddock.
        assert_eq!(
            agent_action(peer("0.8.0", P1_0), peer("0.9.0", P1_3), true),
            Action::Update
        );
        // A different major with can_update updates.
        assert_eq!(
            agent_action(host, peer("0.5.0", P2_0), true),
            Action::Update
        );
        // A different minor without can_update talks: the majors agree.
        assert_eq!(agent_action(host, peer("0.8.0", P1_0), false), Action::Talk);
        // A different major without can_update is the one refusal, and
        // its text names the versions and `paddock macvm update`.
        match agent_action(host, peer("0.5.0", P2_0), false) {
            Action::Refuse(line) => {
                for part in ["0.5.0", "2.0", "0.9.0", "1.3", "paddock macvm update"] {
                    assert!(line.contains(part), "{line:?} lacks {part:?}");
                }
            }
            other => panic!("{other:?}"),
        }
        assert!(compatible(P1_0, P1_3) && !compatible(P1_3, P2_0));
    }

    #[test]
    fn after_update_cases() {
        assert_eq!(after_update("0.9.1", "0.9.1"), Followup::Updated);
        assert_eq!(after_update("0.9.1", "0.9.0"), Followup::RolledBack);
        assert_eq!(after_update("0.9.1", "0.8.0"), Followup::RolledBack);
    }

    #[test]
    fn direction_cases() {
        assert_eq!(direction("0.9.0", "0.9.0"), Direction::Same);
        assert_eq!(direction("0.9.0", "0.10.0"), Direction::Upgrade);
        assert_eq!(direction("0.10.0", "0.9.9"), Direction::Downgrade);
        assert_eq!(direction("0.9.0-rc1", "0.9.0"), Direction::Upgrade);
        assert_eq!(direction("0.9.0", "0.9.0-rc1"), Direction::Downgrade);
        assert_eq!(direction("v0.9.0", "0.9.1"), Direction::Upgrade);
        assert_eq!(direction("0.9.0", "0.9.0.1"), Direction::Upgrade);
        assert_eq!(direction("0.9", "abc"), Direction::Unknown);
        assert_eq!(direction("", "0.9.0"), Direction::Unknown);
    }

    #[test]
    fn agent_dir_is_one_safe_component() {
        assert_eq!(agent_dir("0.9.0"), PathBuf::from("agent/0.9.0"));
        assert_eq!(
            agent_dir("0.9.0-test 2"),
            PathBuf::from("agent/0.9.0-test_2")
        );
        assert_eq!(agent_dir("../x"), PathBuf::from("agent/_x"));
        assert!(!paddock_version().is_empty());
    }
}
