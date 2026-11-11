//! The migration of a profile between two stamps: why a stamp is stale, and
//! what goes before the next start. Pure; the table test is
//! `session::migrate_table`. `session` re-exports it.

use super::session::{Now, Reason, Stamp, What, SCHEMA};

/// Why a stamp is stale against the host now, before the files are looked
/// at. The golden comes first, because its clone makes every later reason
/// moot. `bound` says the data disk is formatted: then the profile keeps
/// its system disk and a new golden is no reason.
pub fn stale_reason(old: &Stamp, now: &Now, bound: bool) -> Option<Reason> {
    if old.schema != SCHEMA {
        Some(Reason::Unreadable)
    } else if old.golden != now.golden && !bound {
        Some(Reason::GoldenChanged)
    } else if old.host_build != now.host_build || old.vz != now.vz {
        Some(Reason::HostChanged)
    } else if old.protocol.major != now.protocol.major {
        Some(Reason::ProtocolChanged)
    } else if old.display != now.display {
        Some(Reason::DisplayChanged)
    } else {
        None
    }
}

/// What a migration between two stamps does to a profile. `prepare` carries
/// it out, `paddock app status` prints it before the cold boot. The data
/// disk, the downloads, the app copy and the backups are never in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    /// A backup of the disk pair comes first. No migration asks for it now:
    /// the system disk never moves under data.
    Backup,
    /// `system.img` is cloned again from the golden.
    System,
    /// `aux.img` is copied again from the golden.
    Aux,
    /// `state.vzs` is discarded: the next start is a cold boot.
    State,
}

impl Migration {
    /// The removal that carries the step out, if it is one.
    pub fn removal(self) -> Option<What> {
        match self {
            Migration::Backup => None,
            Migration::System => Some(What::System),
            Migration::Aux => Some(What::Aux),
            Migration::State => Some(What::State),
        }
    }

    /// The word of the status line.
    pub fn word(self) -> &'static str {
        match self {
            Migration::Backup => "backup",
            Migration::System => "system",
            Migration::Aux => "aux",
            Migration::State => "state",
        }
    }
}

/// The migration of a profile from the stamp of its last session to the
/// host now. Pure. The same golden and the same paddock give nothing. A new
/// golden gives nothing when the data disk is formatted (`bound`): the
/// profile keeps its system disk. A new golden over a blank data disk gives
/// `[System, Aux, State]`, with no backup because the disk holds nothing. A
/// new paddock of the same protocol major gives nothing: the agent updates
/// itself. A new major, a new host build, a new display or a stamp of an
/// unknown schema give `[State]`: the saved state goes, the data disk
/// stays.
pub fn migrate(old: &Stamp, now: &Now, bound: bool) -> Vec<Migration> {
    match stale_reason(old, now, bound) {
        None => vec![],
        Some(Reason::GoldenChanged) => {
            vec![Migration::System, Migration::Aux, Migration::State]
        }
        Some(_) => vec![Migration::State],
    }
}
