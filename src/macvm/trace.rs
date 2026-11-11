//! Where the time of a host resize goes, pure, with tests on Linux. The
//! session keeps one [`Trace`] per resized window and writes its
//! [`Trace::fields`] into the event log as the line `the host resize
//! ended`, once the guest has the last size.
//!
//! The stages of one step:
//! - `host`: from the AppKit event to the session's handling of it.
//! - `rtt`: from the `window.move` to its reply.
//! - `agent`: the move inside the agent (the Accessibility calls), from
//!   the reply.
//! - `paint`: the view and the cover after the reply, on the host.
//! - `behind`: how long the host window was larger than the guest
//!   window's last frame, so the cover showed. The longest stretch and
//!   the sum.

use std::time::{Duration, Instant};

/// The timings of one resize.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    started: Instant,
    steps: u32,
    sent: u32,
    host: Vec<f64>,
    rtt: Vec<f64>,
    agent: Vec<f64>,
    paint: Vec<f64>,
    sent_at: Option<Instant>,
    behind_since: Option<Instant>,
    behind_max: f64,
    behind_sum: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The value at fraction `p` (0 to 1) of the sorted values, 0 without
/// values.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let i = ((v.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    v[i]
}

/// Round to a tenth of a millisecond, for the log line.
fn tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

impl Trace {
    pub fn new(now: Instant) -> Self {
        Trace {
            started: now,
            steps: 0,
            sent: 0,
            host: Vec::new(),
            rtt: Vec::new(),
            agent: Vec::new(),
            paint: Vec::new(),
            sent_at: None,
            behind_since: None,
            behind_max: 0.0,
            behind_sum: 0.0,
        }
    }

    /// A host step reached the session `queued` after its AppKit event.
    pub fn step(&mut self, queued: Duration) {
        self.steps += 1;
        self.host.push(ms(queued));
    }

    /// A `window.move` went to the guest.
    pub fn sent(&mut self, now: Instant) {
        self.sent += 1;
        self.sent_at = Some(now);
    }

    /// The reply of the move in flight came, with the agent's own time
    /// when it gave one.
    pub fn replied(&mut self, now: Instant, agent_ms: Option<f64>) {
        if let Some(t) = self.sent_at.take() {
            self.rtt.push(ms(now.saturating_duration_since(t)));
        }
        if let Some(a) = agent_ms.filter(|a| a.is_finite() && *a >= 0.0) {
            self.agent.push(a);
        }
    }

    /// The host painted the view and the cover in `took`.
    pub fn painted(&mut self, took: Duration) {
        self.paint.push(ms(took));
    }

    /// Whether the host window is larger than the guest's last frame now.
    pub fn behind(&mut self, behind: bool, now: Instant) {
        match (behind, self.behind_since) {
            (true, None) => self.behind_since = Some(now),
            (false, Some(since)) => {
                let d = ms(now.saturating_duration_since(since));
                self.behind_max = self.behind_max.max(d);
                self.behind_sum += d;
                self.behind_since = None;
            }
            _ => {}
        }
    }

    /// The fields of the log line at `now`. `dropped` is the count of
    /// frames that a newer one replaced before they were sent.
    pub fn fields(&mut self, dropped: u32, now: Instant) -> Vec<(&'static str, f64)> {
        self.behind(false, now);
        vec![
            ("ms", tenth(ms(now.saturating_duration_since(self.started)))),
            ("steps", self.steps as f64),
            ("sent", self.sent as f64),
            ("dropped", dropped as f64),
            ("host_p50", tenth(percentile(&self.host, 0.5))),
            ("host_p95", tenth(percentile(&self.host, 0.95))),
            ("rtt_p50", tenth(percentile(&self.rtt, 0.5))),
            ("rtt_p95", tenth(percentile(&self.rtt, 0.95))),
            ("agent_p50", tenth(percentile(&self.agent, 0.5))),
            ("agent_p95", tenth(percentile(&self.agent, 0.95))),
            ("paint_p95", tenth(percentile(&self.paint, 0.95))),
            ("behind_max", tenth(self.behind_max)),
            ("behind_sum", tenth(self.behind_sum)),
        ]
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
