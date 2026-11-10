//! `paddock run dsh --horse-id ID`: a run that continues a horse.
//!
//! The run resumes the newest dsh session of the horse that has rows. Its
//! native id stays when dsh still has the session file in the sandbox
//! home; otherwise paddock projects the session into a new file first. A
//! horse without such a session gets a new session, with the DNA entry
//! first, as a new run does.

use super::RunHorse;
use crate::horse::live::custody::find_session_dir;
use crate::horse::{dna, dsh, Binding, Store};
use anyhow::{bail, Result};
use std::path::Path;

/// The session that a continued run holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continued {
    pub run: RunHorse,
    /// The dsh id of the resumed session. None: a new session, which the
    /// run binds as usual.
    pub native: Option<String>,
}

/// The session of horse `id` that a dsh run continues. `sessions_root`:
/// the dsh sessions folder of the sandbox home, as paddock sees it.
pub fn continue_dsh(
    store: &Store,
    id: &str,
    harness: &str,
    sessions_root: &Path,
) -> Result<Continued> {
    if harness != dsh::HARNESS {
        bail!("--horse-id works with dsh runs only, not with {harness}");
    }
    let horse = store.horse(id)?;
    let newest = store
        .sessions()?
        .into_iter()
        .filter(|s| s.horse == horse.id && s.binding.harness == dsh::HARNESS)
        .filter(|s| s.head.is_some())
        .max_by_key(|s| s.created_ms);
    if let Some(s) = newest {
        let native = match &s.binding.native_id {
            Some(n) if find_session_dir(sessions_root, n).is_some() => n.clone(),
            _ => dsh::project(store, &s.id, sessions_root, crate::horse::now_ms())?.native_id,
        };
        return Ok(Continued {
            run: RunHorse {
                horse: horse.id,
                session: s.id,
                harness: dsh::HARNESS.into(),
            },
            native: Some(native),
        });
    }
    let locked = store.lock()?;
    let entry = dna::render_entry(&dna::get_or_default(store, &horse.dna), "start");
    let session = locked.new_session_first(
        &horse.id,
        Binding {
            harness: dsh::HARNESS.into(),
            native_id: None,
            version: dsh::VERSION.into(),
            container: dsh::CONTAINER.into(),
        },
        Some(("system", &entry)),
    )?;
    Ok(Continued {
        run: RunHorse {
            horse: horse.id,
            session: session.id,
            harness: dsh::HARNESS.into(),
        },
        native: None,
    })
}
