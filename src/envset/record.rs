//! The record of a run: names and origins, never values.

use super::*;

/// What a run got at its start: names and origins, never values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub api_version: String,
    pub id: String,
    pub agent: String,
    pub project: PathBuf,
    pub started: u64,
    pub mode: Mode,
    pub groups: Vec<Group>,
    pub held: Vec<Held>,
}

/// The group of a spec entry, by its origin.
pub fn group_of(origin: &str) -> &'static str {
    if origin == "host env (inherited)" {
        "host"
    } else if origin.starts_with("manifest env.") || origin.starts_with("command: env prefix") {
        "yours"
    } else if origin.starts_with("credentials") {
        "keys"
    } else {
        "agent"
    }
}

/// The record of the environment of a spec. The backend adds the basics
/// of the host (backend::base_env), so they are named here too.
pub fn record(env: &HostEnv, spec: &crate::spec::SandboxSpec, mode: Mode) -> Record {
    let mut groups: Vec<Group> = [
        ("agent", format!("Needed by {}", spec.harness)),
        ("yours", "Your variables".to_string()),
        ("keys", "Keys".to_string()),
        ("host", host_title().to_string()),
    ]
    .into_iter()
    .map(|(id, t)| group(id, t))
    .collect();
    let idx = |id: &str| {
        ["agent", "yours", "keys", "host"]
            .iter()
            .position(|g| *g == id)
            .unwrap_or(0)
    };
    for (k, _) in env.vars.iter().filter(|(k, _)| is_basic(k)) {
        if !spec.env.contains_key(k) {
            groups[0].items.push(Item {
                name: k.clone(),
                origin: "basics".into(),
                passes: true,
                ..Default::default()
            });
        }
    }
    for (k, v) in &spec.env {
        let g = idx(group_of(&v.origin));
        groups[g].items.push(Item {
            name: k.clone(),
            origin: v.origin.clone(),
            passes: true,
            handed_over: v.origin.ends_with(" env_pass") && policy::looks_secret(k),
            ..Default::default()
        });
    }
    let host = groups[3].items.len();
    groups[3].count = Some(host);
    groups[3].passes = Some(mode == Mode::Inherit);
    let mut held: Vec<Held> = Vec::new();
    for (kind, target, rule) in &spec.boundary {
        if kind != crate::boundary::ENV {
            continue;
        }
        let (r, reason) = if rule.starts_with("cloud") {
            ("key", "cloud")
        } else if rule.starts_with("secret") {
            ("key", "secret")
        } else if rule == "env hide" {
            ("hidden", "env hide")
        } else if rule.starts_with("account") {
            ("account", "login folder")
        } else {
            ("never", rule.as_str())
        };
        held.push(Held {
            name: target.clone(),
            rule: r.into(),
            reason: reason.into(),
            text: refusal(target, None)
                .map(|r| r.sentence)
                .unwrap_or_else(|| format!("{target} stays on this computer: {rule}.")),
        });
    }
    for (k, v) in &env.vars {
        if spec.env.contains_key(k) || held.iter().any(|h| h.name == *k) {
            continue;
        }
        if let Some(w) = policy::never_pass(k, Some(v)) {
            if w != Never::Sandbox {
                held.push(Held {
                    name: k.clone(),
                    rule: "never".into(),
                    reason: w.name().into(),
                    text: policy::never_sentence(k, w),
                });
            }
        }
    }
    held.sort_by(|a, b| a.name.cmp(&b.name));
    Record {
        api_version: RUN_API.into(),
        id: spec.id.clone(),
        agent: spec.harness.clone(),
        project: spec.project.clone(),
        started: crate::state::now(),
        mode,
        groups,
        held,
    }
}

fn records_dir(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("env-runs")
}

/// The most records that paddock keeps.
pub const KEEP_RECORDS: usize = 200;

/// Write the record of a run: `env-runs/ID.json` in the state folder,
/// 0600. The oldest go when there are more than `KEEP_RECORDS`.
pub fn save(env: &HostEnv, r: &Record) -> Result<PathBuf> {
    let dir = records_dir(env);
    crate::privfs::dir(&env.paddock_state())?;
    crate::privfs::dir(&dir)?;
    if r.id.contains(['/', '\0']) || r.id.starts_with('.') || r.id.is_empty() {
        bail!("'{}' is not a run id", r.id);
    }
    let path = dir.join(format!("{}.json", r.id));
    crate::privfs::write_atomic(&path, serde_json::to_vec_pretty(r)?)?;
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .collect()
        })
        .unwrap_or_default();
    if all.len() > KEEP_RECORDS {
        all.sort();
        for (_, p) in all.iter().take(all.len() - KEEP_RECORDS) {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(path)
}

/// The record of a run by its id or a unique prefix of it.
pub fn load_record(env: &HostEnv, id: &str) -> Result<Record> {
    let dir = records_dir(env);
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|x| x == "json")
                        && p.file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| s.starts_with(id))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    let exact = found
        .iter()
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(id))
        .cloned();
    let path = match (exact, found.len()) {
        (Some(p), _) => p,
        (None, 1) => found.remove(0),
        (None, 0) => bail!("no record of a run with the id {id}"),
        (None, n) => bail!("{n} runs start with {id}: give more of the id"),
    };
    let bytes = std::fs::read(&path)?;
    serde_json::from_slice(&bytes).with_context(|| format!("{} is invalid", path.display()))
}
