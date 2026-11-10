//! Run-specific inputs retained in the encrypted horse store.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anchor {
    pub goal: String,
    pub base: Option<String>,
    /// Paths masked by the target sandbox's credential guard.
    pub hidden: Vec<PathBuf>,
    #[serde(default)]
    pub ceiling: Option<crate::spawn::Ceiling>,
}

fn path(run: &str) -> Result<String> {
    if !crate::view::valid_id(run) {
        bail!("invalid run id");
    }
    Ok(format!("trainer/anchors/{run}.json"))
}

pub fn save(store: &crate::horse::Store, run: &str, anchor: &Anchor) -> Result<()> {
    store.write_ref(&path(run)?, &serde_json::to_vec(anchor)?)
}

pub fn load(store: &crate::horse::Store, run: &str) -> Result<Option<Anchor>> {
    store
        .read_ref(&path(run)?)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(Into::into))
        .transpose()
}

/// Paths recorded for a sandbox must cover both absolute and project-relative
/// spellings. A mask can name a directory, not only a single file.
pub fn hidden(spec: &crate::spec::SandboxSpec) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for mount in &spec.mounts {
        if !matches!(
            mount.kind,
            crate::spec::MountKind::MaskFile | crate::spec::MountKind::MaskDir
        ) {
            continue;
        }
        paths.push(mount.dst.clone());
        if let Ok(rel) = mount.dst.strip_prefix(&spec.project) {
            if !rel.as_os_str().is_empty() {
                paths.push(rel.to_path_buf());
            }
        }
    }
    paths
}

/// Include configured masks even when the path did not yet exist at launch.
pub fn hidden_for(env: &crate::env::HostEnv, built: &crate::build::Built) -> Vec<PathBuf> {
    let mut paths = hidden(&built.spec);
    for configured in &built.manifest.secrets.hide {
        let expanded = env.expand(configured, &built.spec.project);
        paths.push(expanded.clone());
        if let Ok(relative) = expanded.strip_prefix(&built.spec.project) {
            if !relative.as_os_str().is_empty() {
                paths.push(relative.to_path_buf());
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_absolute_and_project_relative_masks() {
        let project = PathBuf::from("/tmp/demo-project");
        let mut spec = crate::spec::SandboxSpec::new(
            "test".into(),
            "codex",
            &project,
            "/tmp/demo-home".into(),
            std::path::Path::new("/tmp/home"),
            1,
            1,
            "demo",
        );
        spec.add_mount(crate::spec::Mount::mask(
            project.join("private/token"),
            false,
            "manifest secrets.hide",
        ));
        let masks = hidden(&spec);
        assert!(masks.contains(&project.join("private/token")));
        assert!(masks.contains(&PathBuf::from("private/token")));
    }

    #[test]
    fn anchor_is_encrypted_at_rest_and_round_trips() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(temp.path());
        let store = crate::horse::Store::open_host(&env)?;
        let secret = "task-canary-041321";
        let anchor = Anchor {
            goal: secret.into(),
            base: None,
            hidden: vec![PathBuf::from("private/file")],
            ceiling: None,
        };
        save(&store, "test-run", &anchor)?;
        let raw = std::fs::read(store.root().join("trainer/anchors/test-run.json"))?;
        assert!(!raw
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        assert_eq!(load(&store, "test-run")?.unwrap().goal, secret);
        assert!(save(&store, "../bad", &anchor).is_err());
        Ok(())
    }
}
