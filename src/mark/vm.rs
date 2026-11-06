//! The facts of a macOS guest session that its marks show, from the
//! session record that the shim writes (`macvm::host::Session`): the app's
//! name for the menu entry, the guest input source while it differs from
//! the host's, and the note of the session's badge.

use crate::env::HostEnv;
use crate::macvm::host::{self, Session};
use crate::state::Running;

/// What the marks of one guest session show besides the run record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VmFacts {
    /// `ChatGPT (paddock)`, the Dock name of the session.
    pub display_name: String,
    /// The name of the guest input source while it differs from the
    /// host's. None while they match.
    pub guest_input: Option<String>,
    /// The badge of the session (`waking`, `display changed, restart for
    /// full size`), without the input source text, which has its own part.
    pub note: Option<String>,
}

impl VmFacts {
    /// The facts of a session record.
    pub fn of(s: &Session) -> Self {
        let guest_input = s.guest_input.clone().filter(|n| !n.is_empty());
        let input_badge = guest_input.as_deref().map(crate::macvm::text::input_source);
        let note = s
            .badge
            .clone()
            .filter(|b| !b.is_empty() && Some(b) != input_badge.as_ref());
        Self {
            display_name: s.display_name.clone(),
            guest_input,
            note,
        }
    }
}

/// The facts of every run of the list: `Some` for a guest session whose
/// record names the same shim pid, `None` for the other runs.
pub fn facts(env: &HostEnv, runs: &[Running]) -> Vec<Option<VmFacts>> {
    if !runs.iter().any(super::is_vm) {
        return vec![None; runs.len()];
    }
    let layout = crate::macvm::layout::Layout::new(env);
    let sessions = host::sessions(&layout.sessions);
    match_facts(runs, sessions.iter().map(|(_, s)| s))
}

/// The pure part of [`facts`]: a run of the guest gets the facts of the
/// session record with its pid.
pub fn match_facts<'a>(
    runs: &[Running],
    sessions: impl Iterator<Item = &'a Session> + Clone,
) -> Vec<Option<VmFacts>> {
    runs.iter()
        .map(|r| {
            if !super::is_vm(r) {
                return None;
            }
            sessions.clone().find(|s| s.pid == r.pid).map(VmFacts::of)
        })
        .collect()
}
