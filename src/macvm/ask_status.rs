//! `logs/questions.json` of a session: the firewall questions that are
//! open, whether asking is paused, and the hosts of the guest system that
//! were denied without asking, counted per host. The firewall of the
//! session writes it whole (renamed into place) at each change, and
//! `paddock app status` reads it. A file that does not parse reads as
//! empty, so a crash in the middle of a write costs nothing.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name, next to the connection log.
pub const FILE: &str = "questions.json";
/// The hosts of the guest system that the file keeps, most denied first.
pub const SYSTEM_MAX: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskStatus {
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub open: Vec<OpenQuestion>,
    #[serde(default)]
    pub system: Vec<SystemHost>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub app: String,
    pub host: String,
    pub port: u16,
    /// The connections that wait on the answer now.
    #[serde(default)]
    pub waiting: u64,
    #[serde(default)]
    pub asked_at: u64,
    /// On the screen, not in the queue.
    #[serde(default)]
    pub shown: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemHost {
    pub host: String,
    pub port: u16,
    /// The last guest process that connected.
    pub process: String,
    pub count: u64,
}

impl AskStatus {
    /// The connections of the guest system denied without asking.
    pub fn system_denied(&self) -> u64 {
        self.system.iter().map(|s| s.count).sum()
    }
}

/// The file of a session, next to its connection log.
pub fn path_next_to(connections_log: &Path) -> PathBuf {
    connections_log.with_file_name(FILE)
}

/// Count one denied connection of the guest system.
pub fn count_system(hosts: &mut Vec<SystemHost>, host: &str, port: u16, process: &str) {
    match hosts.iter_mut().find(|s| s.host == host && s.port == port) {
        Some(s) => {
            s.count += 1;
            s.process = process.to_string();
        }
        None => hosts.push(SystemHost {
            host: host.to_string(),
            port,
            process: process.to_string(),
            count: 1,
        }),
    }
    hosts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.host.cmp(&b.host)));
    hosts.truncate(SYSTEM_MAX);
}

pub fn write(path: &Path, s: &AskStatus) {
    if let Some(dir) = path.parent() {
        if let Err(e) = crate::privfs::dir(dir) {
            log::debug!("macvm: cannot make {}: {e}", dir.display()); // not user text
            return;
        }
    }
    let body = serde_json::to_string_pretty(s).unwrap_or_default();
    if let Err(e) = crate::privfs::write_atomic(path, body) {
        log::debug!("macvm: cannot write {}: {e}", path.display()); // not user text
    }
}

pub fn read(path: &Path) -> AskStatus {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// A new session starts with no questions.
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_and_a_broken_one_reads_as_empty() {
        let t = tempfile::tempdir().unwrap();
        let p = path_next_to(&t.path().join("logs").join("connections.jsonl"));
        assert!(p.ends_with("logs/questions.json"));
        assert_eq!(read(&p), AskStatus::default());
        let mut s = AskStatus {
            paused: true,
            ..Default::default()
        };
        s.open.push(OpenQuestion {
            app: "com.openai.chat".into(),
            host: "a.example".into(),
            port: 443,
            waiting: 1,
            asked_at: 5,
            shown: true,
        });
        count_system(
            &mut s.system,
            "weather-edge.apple.com",
            443,
            "com.apple.weather",
        );
        write(&p, &s);
        assert_eq!(read(&p), s);
        std::fs::write(&p, "{\"paused\": tr").unwrap();
        assert_eq!(read(&p), AskStatus::default(), "half a file is nothing");
        clear(&p);
        assert!(!p.exists());
    }

    #[test]
    fn system_hosts_are_counted_per_host_and_capped() {
        let mut h = Vec::new();
        count_system(&mut h, "b.apple.com", 443, "x");
        count_system(&mut h, "a.apple.com", 443, "y");
        count_system(&mut h, "a.apple.com", 443, "z");
        assert_eq!(h[0].host, "a.apple.com");
        assert_eq!((h[0].count, h[0].process.as_str()), (2, "z"));
        let s = AskStatus {
            system: h.clone(),
            ..Default::default()
        };
        assert_eq!(s.system_denied(), 3);
        for i in 0..SYSTEM_MAX + 10 {
            count_system(&mut h, &format!("h{i}.apple.com"), 443, "p");
        }
        assert_eq!(h.len(), SYSTEM_MAX);
        assert_eq!(h[0].host, "a.apple.com", "the most denied stays");
    }
}
