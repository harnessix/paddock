//! The session record of the parent: the slot, once it is known, the
//! counters of the stub network, and the health, the start and the app of
//! the session for `paddock app status`.

use super::host::{RecordApp, RecordHealth, RecordStart};
use super::proto::Health;
use super::run::Session;
use std::time::{Duration, Instant};

/// Two `health` messages closer than this write the record once.
const HEALTH_RECORD: Duration = Duration::from_secs(1);

impl Session<'_> {
    /// Change the record of the parent in place. Nothing when there is
    /// no record or it is not readable.
    pub(super) fn update_record(&self, change: impl FnOnce(&mut serde_json::Value)) -> bool {
        let Some(path) = &self.plan.record else {
            return false;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        change(&mut v);
        crate::privfs::write_atomic(path, v.to_string()).is_ok()
    }

    /// The last `health` of the guest into the record, at most once a
    /// second, with the count of the host windows.
    pub(super) fn write_health(&mut self, h: &Health) {
        let now = Instant::now();
        if self
            .health_written
            .is_some_and(|t| now.duration_since(t) < HEALTH_RECORD)
        {
            return;
        }
        let health = RecordHealth {
            t: now_secs(),
            load: h.load,
            mem_free: h.mem_free,
            data_disk_free: h.data_disk_free,
            app_running: h.app_running,
            windows: self.shell.order.len(),
            input_source: h.input_source.clone(),
            locked: h.locked,
        };
        let value = serde_json::to_value(&health).unwrap_or_default();
        if self.update_record(|v| v["health"] = value) {
            self.health_written = Some(now);
        }
    }

    /// How the session started and the seconds to the first hello.
    pub(super) fn write_start(&mut self, secs: f64) {
        let start = RecordStart {
            kind: self.start.word().to_string(),
            secs: (secs * 1000.0).round() / 1000.0,
        };
        let value = serde_json::to_value(&start).unwrap_or_default();
        self.update_record(|v| v["start"] = value);
    }

    /// The app of the session, at its launch.
    pub(super) fn write_app(&mut self, name: &str, version: &str) {
        let app = RecordApp {
            name: name.to_string(),
            version: version.to_string(),
        };
        let value = serde_json::to_value(&app).unwrap_or_default();
        self.update_record(|v| v["app_info"] = value);
    }

    /// The counters of the stub network into the session record, for
    /// `paddock app status --json`.
    pub(super) fn write_network(&mut self) {
        let Some(path) = &self.plan.record else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let status = self.network.status();
        if v["network"] == status {
            return;
        }
        v["network"] = status;
        let _ = crate::privfs::write_atomic(path, v.to_string());
    }

    /// Put the slot into the session record of the parent, once.
    pub(super) fn write_record(&mut self) {
        if self.record_done {
            return;
        }
        let Some(path) = &self.plan.record else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        v["slot"] = serde_json::json!(self.slot_index);
        v["network"] = self.network.status();
        if crate::privfs::write_atomic(path, v.to_string()).is_ok() {
            self.record_done = true;
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
