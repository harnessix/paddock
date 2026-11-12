//! The paddock kv side of `paddock gc` and `paddock doctor`, and the
//! leases of frozen sandboxes: they walk every store file.

use super::{kv_root, shard, store, KvCfg};
use crate::broker::Mode;
use crate::env::HostEnv;
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use store::{Quota, Store, Writer};

/// What `paddock gc` can remove from paddock kv.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct GcPlan {
    /// Finished runs whose files (`self` and their own pasture, or the
    /// `tree` scope of a root) are older than `keep_days`.
    pub stale: Vec<store::Stale>,
    pub expired_entries: u64,
    pub expired_bytes: u64,
    /// The store of older paddock versions, when nothing changed it for
    /// `keep_days`: its size.
    pub legacy_bytes: Option<u64>,
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Every store file except the registry and the old store.
fn shard_files(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.join("user.db")];
    for dir in ["sandbox", "tree"] {
        out.extend(
            shard::ids_in(root, dir)
                .into_iter()
                .map(|id| root.join(dir).join(format!("{id}.db"))),
        );
    }
    out.extend(
        shard::named(root)
            .into_iter()
            .map(|n| root.join("ns").join(format!("{n}.db"))),
    );
    for p in std::fs::read_dir(root.join("pasture"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for f in std::fs::read_dir(p.path()).into_iter().flatten().flatten() {
            if f.path().extension().is_some_and(|e| e == "db") {
                out.push(f.path());
            }
        }
    }
    out.retain(|p| p.is_file());
    out
}

/// A frozen sandbox keeps its leases: their time to live stops in every
/// store file until `resume_leases`. Returns how many leases it paused.
pub fn pause_leases(root: &Path, sandbox: &str, now: u64) -> Result<u64> {
    let mut n = 0;
    for f in shard_files(root) {
        n += Store::open(&f)?.pause_leases(sandbox, now)?;
    }
    Ok(n)
}

/// A thawed or ended sandbox: its paused leases run again with the time
/// that they had left. Returns how many.
pub fn resume_leases(root: &Path, sandbox: &str, now: u64) -> Result<u64> {
    let mut n = 0;
    for f in shard_files(root) {
        n += Store::open(&f)?.resume_leases(sandbox, now)?;
    }
    Ok(n)
}

/// A frozen sandbox frees its leases at once. paddock is the writer of each
/// release. Returns the kind of each scope (`pasture`, `tree`, `self`,
/// `user`, `ns`) and the key: no project path, no value.
pub fn release_leases(
    root: &Path,
    sandbox: &str,
    cfg: &KvCfg,
    now: u64,
) -> Result<Vec<(String, String)>> {
    let w = Writer {
        sandbox: "paddock",
        harness: "paddock",
    };
    let q = Quota {
        bytes: cfg.project_bytes,
        keys: cfg.max_keys,
    };
    let mut out = Vec::new();
    for f in shard_files(root) {
        for (ns, key) in Store::open(&f)?.release_leases(sandbox, &w, q, now)? {
            let kind = ns.split(':').next().unwrap_or_default().to_string();
            out.push((kind, key));
        }
    }
    Ok(out)
}

pub fn gc_plan(env: &HostEnv, running: &[String], now: u64) -> Result<GcPlan> {
    let root = kv_root(env);
    if !root.is_dir() {
        return Ok(GcPlan::default());
    }
    let cfg = KvCfg::load(env)?;
    let cutoff = now.saturating_sub(cfg.keep_days * 24 * 3600);
    let reg_file = shard::registry(&root);
    let reg = if reg_file.is_file() {
        Some(Store::open(&reg_file)?)
    } else {
        None
    };
    // A running child reads its parent and writes the tree of its root, so
    // the sandboxes above a running one stay too.
    let mut keep: Vec<String> = running.to_vec();
    if let Some(r) = &reg {
        for id in running {
            keep.extend(r.ancestors(id)?);
        }
    }
    let mut ids = shard::ids_in(&root, "sandbox");
    ids.extend(shard::ids_in(&root, "tree"));
    ids.sort();
    ids.dedup();
    let mut stale = Vec::new();
    for id in ids.into_iter().filter(|i| !keep.contains(i)) {
        let files = [
            root.join("sandbox").join(format!("{id}.db")),
            root.join("tree").join(format!("{id}.db")),
        ];
        let mut last = files.iter().map(|f| mtime(f)).max().unwrap_or(0);
        if let Some((started, ended)) = reg.as_ref().map(|r| r.times(&id)).transpose()?.flatten() {
            last = last.max(started).max(ended.unwrap_or(0));
        }
        if last >= cutoff {
            continue;
        }
        let (mut entries, mut bytes) = (0, 0);
        for f in files.iter().filter(|f| f.is_file()) {
            entries += Store::open(f)?.totals()?.0;
            bytes += shard::file_bytes(f);
        }
        stale.push(store::Stale {
            sandbox: id,
            entries,
            bytes,
            last,
        });
    }
    let (mut expired_entries, mut expired_bytes) = (0, 0);
    for f in shard_files(&root) {
        let (n, b) = Store::open(&f)?.expired(now)?;
        expired_entries += n;
        expired_bytes += b;
    }
    let legacy = shard::legacy(&root);
    let legacy_bytes =
        (legacy.is_file() && mtime(&legacy) < cutoff).then(|| shard::file_bytes(&legacy));
    Ok(GcPlan {
        stale,
        expired_entries,
        expired_bytes,
        legacy_bytes,
    })
}

/// The items of `paddock gc`: one for the files of finished runs, expired
/// keys and the old store. It is paddock's own data, so `--yes` alone
/// removes it.
pub fn gc_items(env: &HostEnv, now: u64) -> Result<Vec<crate::storage::Item>> {
    let running: Vec<String> = crate::state::Running::list(env)
        .into_iter()
        .map(|r| r.id)
        .collect();
    let plan = gc_plan(env, &running, now)?;
    if plan.stale.is_empty() && plan.expired_entries == 0 && plan.legacy_bytes.is_none() {
        return Ok(Vec::new());
    }
    let days = KvCfg::load(env)?.keep_days;
    let entries: u64 = plan.stale.iter().map(|s| s.entries).sum::<u64>() + plan.expired_entries;
    let mut what = format!(
        "paddock kv: {entries} keys of {} finished runs and expired keys",
        plan.stale.len()
    );
    if plan.legacy_bytes.is_some() {
        what.push_str(", and the store of older paddock versions");
    }
    Ok(vec![crate::storage::Item {
        what,
        tier: crate::storage::Tier::Auto,
        bytes: plan.stale.iter().map(|s| s.bytes).sum::<u64>()
            + plan.expired_bytes
            + plan.legacy_bytes.unwrap_or(0),
        reason: format!(
            "the self scope and the own pasture of runs, and the tree scope of spawn trees, that ended more than {days} days ago ([kv] keep_days), keys whose time to live passed, and the project scope of older paddock versions when nothing changed it for {days} days"
        ),
        warning: None,
        needs: vec![],
        action: crate::storage::Action::Kv {
            sandboxes: plan.stale.iter().map(|s| s.sandbox.clone()).collect(),
        },
    }])
}

/// Remove what the plan names. Sandboxes that run now are skipped.
pub fn gc_apply(env: &HostEnv, sandboxes: &[String], now: u64) -> Result<u64> {
    let running: Vec<String> = crate::state::Running::list(env)
        .into_iter()
        .map(|r| r.id)
        .collect();
    let root = kv_root(env);
    let cfg = KvCfg::load(env)?;
    let cutoff = now.saturating_sub(cfg.keep_days * 24 * 3600);
    let mut n = 0;
    let mut reg = Store::open(&shard::registry(&root))?;
    for id in sandboxes.iter().filter(|s| !running.contains(s)) {
        shard::check_id(id)?;
        for dir in ["sandbox", "tree"] {
            let f = root.join(dir).join(format!("{id}.db"));
            if f.is_file() {
                n += Store::open(&f)?.totals()?.0;
                shard::remove_file(&f)?;
            }
        }
        reg.forget_sandbox(id)?;
    }
    for f in shard_files(&root) {
        n += Store::open(&f)?.purge_expired(now)?;
    }
    let legacy = shard::legacy(&root);
    if legacy.is_file() && mtime(&legacy) < cutoff {
        n += Store::open(&legacy)?.totals()?.0;
        shard::remove_file(&legacy)?;
    }
    reg.forget_ended(cutoff, &running)?;
    Ok(n)
}

/// The lines of `paddock doctor` about paddock kv.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    let cfg = match KvCfg::load(env) {
        Ok(c) => c,
        Err(e) => return vec![format!("  error    {e:#}")],
    };
    if !cfg.enabled {
        return vec!["  off      paddock kv: [kv] enabled = false".into()];
    }
    let root = kv_root(env);
    let user = match cfg.user {
        Mode::Ask => "asks at first use",
        Mode::Allow => "allowed",
        Mode::Deny => "off",
    };
    let reg = shard::registry(&root);
    let state = if reg.is_file() {
        match Store::open(&reg) {
            Ok(_) => format!("files in {}", root.display()),
            Err(e) => return vec![format!("  error    paddock kv: {e:#}")],
        }
    } else {
        "the files are created at the first start".to_string()
    };
    let mut out = vec![format!(
        "  on       paddock kv: {state}; user scope {user}; self data kept {} days after a run",
        cfg.keep_days
    )];
    for (name, ns) in &cfg.namespaces {
        let missing: Vec<&str> = ns
            .projects
            .iter()
            .filter(|p| !env.expand(p, Path::new("/")).is_dir())
            .map(String::as_str)
            .collect();
        out.push(if missing.is_empty() {
            format!(
                "  on       paddock kv: namespace ns:{name} for {} projects, harnesses {}",
                ns.projects.len(),
                ns.harnesses.as_ref().map_or("all".into(), |h| h.join(", "))
            )
        } else {
            format!(
                "  warn     paddock kv: namespace ns:{name}: no folder {}; fix projects in [kv.namespaces.{name}]",
                missing.join(", ")
            )
        });
    }
    if shard::legacy(&root).is_file() {
        out.push(format!(
            "  note     paddock kv: {} holds the project scope of an older paddock; sandboxes no longer read it. `paddock kv list --legacy` shows it, and `paddock gc` removes it {} days after its last change",
            shard::legacy(&root).display(),
            cfg.keep_days
        ));
    }
    out
}
