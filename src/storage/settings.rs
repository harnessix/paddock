//! `[storage]` settings of the automatic cleanup, the space guard and the
//! shared build cache. The other `[storage]` keys belong to dedupe.rs,
//! caches.rs, layers.rs and shadow.rs, which read them on their own.
//!
//! A wrong value never stops a run: paddock uses the default for that key
//! and gives back a warning, which `paddock doctor` and the run print.

use crate::env::HostEnv;

pub const GIB: u64 = 1 << 30;

/// A free-space floor: the smaller of `gib` and `percent` of the volume.
/// A zero in either turns the floor off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Floor {
    pub gib: u64,
    pub percent: u64,
}

impl Floor {
    /// The floor in bytes on a volume of `total` bytes. 0 means off.
    pub fn bytes(&self, total: u64) -> u64 {
        if self.gib == 0 || self.percent == 0 {
            return 0;
        }
        let pct = (u128::from(total) * u128::from(self.percent) / 100) as u64;
        pct.min(self.gib.saturating_mul(GIB))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The automatic pass at the start of a run.
    pub auto_gc: bool,
    /// Days without use before the build output of a worktree with
    /// unmerged commits goes.
    pub worktree_cache_days: u64,
    /// Snapshots and undo trash folders older than this go.
    pub snapshot_days: u64,
    pub hard: Floor,
    pub soft: Floor,
    /// The per-project cargo build cache of worktree runs.
    pub build_cache: bool,
    /// The size of all build caches together, in GiB. 0: no limit.
    pub build_cache_gib: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_gc: true,
            worktree_cache_days: 2,
            snapshot_days: 30,
            hard: Floor {
                gib: 15,
                percent: 5,
            },
            soft: Floor {
                gib: 50,
                percent: 10,
            },
            build_cache: true,
            build_cache_gib: 40,
        }
    }
}

impl Settings {
    /// The settings of the user configuration, and a warning for each value
    /// that paddock replaced with its default.
    pub fn load(env: &HostEnv) -> (Settings, Vec<String>) {
        let f = env.paddock_config().join("config.toml");
        match std::fs::read_to_string(&f) {
            Ok(text) => Self::parse(&text),
            Err(_) => (Settings::default(), vec![]),
        }
    }

    pub fn parse(text: &str) -> (Settings, Vec<String>) {
        let mut s = Settings::default();
        let mut warn = Vec::new();
        let v: toml::Value = match toml::from_str(text) {
            Ok(v) => v,
            Err(_) => {
                warn.push("the configuration file is not valid TOML: paddock uses the default [storage] settings".into());
                return (s, warn);
            }
        };
        let Some(t) = v.get("storage").and_then(|t| t.as_table()) else {
            return (s, warn);
        };
        let mut boolean = |key: &str, slot: &mut bool| match t.get(key) {
            None => {}
            Some(toml::Value::Boolean(b)) => *slot = *b,
            Some(_) => warn.push(format!(
                "[storage] {key} must be true or false: paddock uses {slot}"
            )),
        };
        boolean("auto_gc", &mut s.auto_gc);
        boolean("build_cache", &mut s.build_cache);
        let mut number = |key: &str, slot: &mut u64, max: u64| match t.get(key) {
            None => {}
            Some(toml::Value::Integer(n)) if *n >= 0 && (*n as u64) <= max => *slot = *n as u64,
            Some(_) => warn.push(format!(
                "[storage] {key} must be a whole number from 0 to {max}: paddock uses {slot}"
            )),
        };
        number("worktree_cache_days", &mut s.worktree_cache_days, 3650);
        number("snapshot_days", &mut s.snapshot_days, 3650);
        number("hard_floor_gib", &mut s.hard.gib, 1 << 20);
        number("hard_floor_percent", &mut s.hard.percent, 100);
        number("soft_floor_gib", &mut s.soft.gib, 1 << 20);
        number("soft_floor_percent", &mut s.soft.percent, 100);
        number("build_cache_gib", &mut s.build_cache_gib, 1 << 20);
        (s, warn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_a_full_table() {
        let (s, w) = Settings::parse("");
        assert_eq!(s, Settings::default());
        assert!(w.is_empty());
        let (s, w) = Settings::parse(
            "[storage]\nauto_gc = false\nworktree_cache_days = 5\nhard_floor_gib = 20\nhard_floor_percent = 2\nsoft_floor_gib = 0\nbuild_cache = false\nbuild_cache_gib = 8\ndedupe = false\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert!(!s.auto_gc && !s.build_cache);
        assert_eq!(s.worktree_cache_days, 5);
        assert_eq!(
            s.hard,
            Floor {
                gib: 20,
                percent: 2
            }
        );
        assert_eq!(s.soft.gib, 0);
        assert_eq!(s.build_cache_gib, 8);
    }

    #[test]
    fn a_wrong_value_keeps_the_default_and_warns() {
        let (s, w) = Settings::parse(
            "[storage]\nauto_gc = \"yes\"\nhard_floor_percent = 150\nsoft_floor_gib = -1\n",
        );
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].contains("auto_gc"), "{w:?}");
        assert!(w.iter().any(|l| l.contains("hard_floor_percent")));
        let (s, w) = Settings::parse("[storage\n");
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn a_floor_is_the_smaller_of_its_two_limits() {
        let f = Floor {
            gib: 15,
            percent: 5,
        };
        // 906 GiB: 5% is 45 GiB, so 15 GiB counts.
        assert_eq!(f.bytes(906 * GIB), 15 * GIB);
        // 100 GiB: 5% is 5 GiB.
        assert_eq!(f.bytes(100 * GIB), 5 * GIB);
        assert_eq!(Floor { gib: 0, percent: 5 }.bytes(100 * GIB), 0);
        assert_eq!(
            Floor {
                gib: 15,
                percent: 0
            }
            .bytes(100 * GIB),
            0
        );
    }
}
