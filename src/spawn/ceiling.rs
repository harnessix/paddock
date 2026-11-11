//! The ceiling of a spawned child: what the parent had, and the check
//! that refuses a child spec that is wider.

use super::{from_env_or_manifest, network_rank, Ceiling};
use crate::broker::Mode;
use crate::build::Built;
use crate::spec::{GuiMode, MountKind, Network};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};

impl Ceiling {
    pub fn of(built: &Built, backend: &str, modes: &BTreeMap<String, Mode>) -> Self {
        let s = &built.spec;
        let (chain, depth) = match &built.spawn {
            Some(c) => (c.chain.clone(), c.depth),
            None => (Vec::new(), 0),
        };
        let mut chain = chain;
        chain.push(s.id.clone());
        let mut m = built.manifest.clone();
        // A child has no display and publishes no ports.
        m.gui = Default::default();
        m.network.publish.clear();
        m.apps.clear();
        m.default_harness = None;
        Self {
            parent: s.id.clone(),
            parent_harness: s.harness.clone(),
            horse: built.horse,
            chain,
            depth,
            repo: built.home_key.clone(),
            head_from: s.project.clone(),
            backend: backend.to_string(),
            network: s.network,
            lan: s.lan,
            inherit_env: built.inherit_env,
            env_names: s
                .env
                .iter()
                .filter(|(_, v)| from_env_or_manifest(&v.origin))
                .map(|(k, _)| k.clone())
                .collect(),
            allow_domains: s.allow_domains.iter().cloned().collect(),
            host_ports: s.host_ports.iter().map(|(p, _)| *p).collect(),
            mounts: s
                .mounts
                .iter()
                .filter(|m| m.origin.starts_with("manifest"))
                .filter_map(|m| m.src.clone().map(|p| (p, m.kind == MountKind::Rw)))
                .collect(),
            manifest: m,
            modes: modes.clone(),
            auth: built.auth.to_string(),
            devices: Vec::new(),
        }
    }

    /// Refuse `--device` names that the parent does not have. The child
    /// never gets more than its parent.
    pub fn check_devices(&self, want: &[String]) -> Result<()> {
        for d in want {
            if !self.devices.iter().any(|x| x == d) {
                bail!(
                    "spawn: the child would get more than its parent: the device {d} (this sandbox has {})",
                    if self.devices.is_empty() {
                        "no device".to_string()
                    } else {
                        self.devices.join(", ")
                    }
                );
            }
        }
        Ok(())
    }

    /// The root of the spawn tree: the sandbox that the user started.
    pub fn root(&self) -> &str {
        self.chain
            .first()
            .map_or(self.parent.as_str(), |r| r.as_str())
    }

    /// Refuse a child that would get more than the parent. `endpoints` are
    /// the child harness's own API hosts, which it needs in allowlist mode.
    pub fn check(&self, built: &Built, backend: &str) -> Result<()> {
        let s = &built.spec;
        let no = |what: String| -> Result<()> {
            bail!("spawn: the child would get more than its parent: {what}")
        };
        if backend != self.backend {
            no(format!(
                "backend {backend}, the parent runs on {}",
                self.backend
            ))?;
        }
        if network_rank(s.network) > network_rank(self.network) {
            no(format!(
                "network {:?}, the parent has {:?}",
                s.network, self.network
            ))?;
        }
        if s.lan && !self.lan {
            no("lan access".into())?;
        }
        if built.inherit_env && !self.inherit_env {
            no("the host environment, the parent has --env clean".into())?;
        }
        if s.gui.mode != GuiMode::Off || s.gui.gpu {
            no("a display".into())?;
        }
        if !s.publish.is_empty() {
            no("published ports".into())?;
        }
        if built.home_key != self.repo {
            no(format!("the project {}", built.home_key.display()))?;
        }
        for (k, v) in &s.env {
            if from_env_or_manifest(&v.origin) && !self.env_names.contains(k) {
                no(format!("the variable {k} ({})", v.origin))?;
            }
        }
        for (p, _) in &s.host_ports {
            if !self.host_ports.contains(p) {
                no(format!("host port {p}"))?;
            }
        }
        // A parent with a list can pass on only its own domains. A parent
        // with open egress has more than any list.
        let listed = matches!(self.network, Network::Allowlist | Network::Api);
        let endpoints: BTreeSet<&String> = built.profile.network.endpoints.iter().collect();
        for d in s.allow_domains.iter().filter(|_| listed) {
            if !self.allow_domains.contains(d) && !endpoints.contains(d) {
                no(format!("the domain {d}"))?;
            }
        }
        for m in s.mounts.iter().filter(|m| m.origin.starts_with("manifest")) {
            let Some(src) = &m.src else { continue };
            let rw = m.kind == MountKind::Rw;
            let ok = self.mounts.iter().any(|(p, prw)| p == src && (*prw || !rw));
            if !ok {
                no(format!(
                    "the mount {} ({})",
                    src.display(),
                    if rw { "writable" } else { "read-only" }
                ))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::spawn::tests::ceiling;
    use crate::spawn::{Ceiling, Job};
    use std::path::Path;

    #[test]
    fn a_child_gets_at_most_the_devices_of_its_parent() {
        use crate::devices::{enabled_names, DevicesCfg, GrantCtx};
        let on = DevicesCfg::parse("[devices]\necho = true\n").unwrap();
        let off = DevicesCfg::parse("").unwrap();
        let echo = vec!["echo".to_string()];
        let root = GrantCtx::none();
        let none = GrantCtx::with_ceiling(Some(&[]));
        let with_echo = GrantCtx::with_ceiling(Some(&echo));
        // A root gets what the user configuration turns on.
        assert_eq!(enabled_names(&on, false, &root), echo);
        // A child of a parent without devices gets none, even when the
        // configuration turns one on now, and even in test mode.
        assert!(enabled_names(&on, false, &none).is_empty());
        assert!(enabled_names(&on, true, &none).is_empty());
        // A child of a parent with echo keeps it only while the user still
        // turns it on.
        assert_eq!(enabled_names(&on, false, &with_echo), echo);
        assert!(enabled_names(&off, false, &with_echo).is_empty());
        let no_mount = DevicesCfg::parse("[devices]\nmount = false\necho = true\n").unwrap();
        assert!(enabled_names(&no_mount, false, &root).is_empty());
        // `--device` beyond the parent's ceiling is refused with the ceiling
        // message; a name within it passes.
        let mut c = ceiling("p", 0, Path::new("/src/app"));
        c.devices = echo.clone();
        assert!(c.check_devices(&echo).is_ok());
        let e = format!("{:#}", c.check_devices(&["mac".to_string()]).unwrap_err());
        assert_eq!(
        e,
        "spawn: the child would get more than its parent: the device mac (this sandbox has echo)"
    );
        c.devices.clear();
        let e = format!("{:#}", c.check_devices(&echo).unwrap_err());
        assert!(e.ends_with("(this sandbox has no device)"), "{e}");
        // The devices of a job reach the child's options.
        let j = serde_json::json!({"spawn_id": "s", "sandbox_id": "sb", "harness": "codex", "branch": "b", "ceiling": c});
        let job: Job = serde_json::from_value(j).unwrap();
        assert!(
            job.devices.is_empty(),
            "an old job without devices means none"
        );
        // The ceiling travels in the job, and an old job without it means none.
        let mut c = ceiling("p", 0, Path::new("/src/app"));
        c.devices = echo.clone();
        let back: Ceiling = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.devices, echo);
        let mut v = serde_json::to_value(&c).unwrap();
        v.as_object_mut().unwrap().remove("devices");
        let old: Ceiling = serde_json::from_value(v).unwrap();
        assert!(old.devices.is_empty());
        assert_eq!(old.root(), "p");
    }
}
