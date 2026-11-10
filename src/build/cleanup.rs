use super::{discard_worktree, Worktree};
use crate::env::HostEnv;

/// Drops the worktree unless `keep` takes it (the build succeeded).
pub(super) struct PendingWorktree<'a> {
    pub(super) env: &'a HostEnv,
    pub(super) wt: Option<Worktree>,
}

impl PendingWorktree<'_> {
    pub(super) fn keep(&mut self) -> Option<Worktree> {
        self.wt.take()
    }
}

impl Drop for PendingWorktree<'_> {
    fn drop(&mut self) {
        if let Some(w) = self.wt.take() {
            discard_worktree(self.env, &w);
        }
    }
}

/// The run folder of a shadow run while the build can still fail.
pub(super) struct PendingShadow(pub(super) Option<crate::shadow::Plan>);

impl Drop for PendingShadow {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = p.remove();
        }
    }
}
