use super::{valid_id, PaneOutput, Tmux};
use std::process::Stdio;

/// One line of `#{pane_last_output_time},#{window_activity}`.
pub fn parse_last_output(line: &str) -> PaneOutput {
    let (pane, window) = line.trim().split_once(',').unwrap_or((line.trim(), ""));
    let pick = if pane.trim().is_empty() { window } else { pane };
    let pick = pick.trim();
    if pick.is_empty() {
        return PaneOutput::Quiet;
    }
    match pick.parse::<u64>() {
        Ok(0) => PaneOutput::Quiet,
        Ok(t) => PaneOutput::At(t),
        Err(_) => PaneOutput::Unknown,
    }
}

impl Tmux {
    /// When the pane last wrote output. `pane_last_output_time` is empty
    /// until the pane writes something, and tmux before 3.5 does not know it.
    /// Then `window_activity` counts: each run has one window, and tmux sets
    /// that time when the window starts and on each output.
    pub fn last_output(&self, id: &str) -> PaneOutput {
        if !valid_id(id) || !self.sock.exists() {
            return PaneOutput::Missing;
        }
        let target = format!("={id}");
        let out = self
            .cmd()
            .args([
                "list-panes",
                "-t",
                &target,
                "-F",
                "#{pane_last_output_time},#{window_activity}",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let Ok(out) = out else {
            return PaneOutput::Unknown;
        };
        if !out.status.success() {
            return PaneOutput::Missing;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        parse_last_output(text.lines().next().unwrap_or(""))
    }

    /// Count all clients viewing this session, including read-only clients.
    pub fn clients(&self, id: &str) -> usize {
        if !valid_id(id) || !self.sock.exists() {
            return 0;
        }
        self.cmd()
            .args([
                "list-clients",
                "-t",
                &format!("={id}"),
                "-F",
                "#{client_readonly}",
            ])
            .stdout(Stdio::piped())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map_or(0, |o| String::from_utf8_lossy(&o.stdout).lines().count())
    }
}
