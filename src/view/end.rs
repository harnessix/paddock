//! The end of a view session, and of the server when it is empty.
use super::{valid_id, Tmux};

/// After the session ends: stop the server when it has no session left.
/// tmux checks `server_sessions` in the same command list as the
/// kill-session, so no other client can start a session in between.
const END_IF_EMPTY: [&str; 4] = [
    "if-shell",
    "-F",
    "#{==:#{server_sessions},0}",
    "kill-server",
];

impl Tmux {
    /// End the session `id`. When no session is left, end the server too:
    /// the run that ends the last session leaves no tmux server behind.
    ///
    /// One tmux command list does both steps. tmux runs the commands of one
    /// client one after the other and runs no command of another client in
    /// between. A new run that starts a session on the same server either
    /// comes first, so the server is not empty and stays, or comes after
    /// the server is gone and starts a new one. Two separate tmux calls
    /// (list-sessions, then kill-server) could end the session of a run
    /// that started in between, also a detached harness.
    pub fn kill(&self, id: &str) -> bool {
        if !valid_id(id) {
            return false;
        }
        let target = format!("={id}");
        let mut args = vec!["kill-session", "-t", &target, ";"];
        args.extend(END_IF_EMPTY);
        self.run(&args)
    }
}

#[cfg(test)]
#[path = "end_tests.rs"]
mod tests;
