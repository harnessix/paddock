//! The note at the end of a run about files in the recovery folders.

use super::{scan, RecoveryCfg};
use crate::env::HostEnv;
use std::path::Path;

/// Quick Recovery at exit: the files left in the recovery folders, and how to copy them.
pub fn notice(env: &HostEnv, id: &str, home: &Path, started: u64) {
    let id = crate::ids::short(id);
    let cfg = match RecoveryCfg::load(env) {
        Ok(c) => c,
        Err(e) => {
            crate::note::warn(format!("{e:#}"));
            return;
        }
    };
    let items: Vec<_> = scan(env, &cfg, home, started)
        .into_iter()
        .filter(|i| i.new)
        .collect();
    if items.is_empty() {
        return;
    }
    let mut t = format!(
        "the sandbox saved {} file(s) outside the project. They stay in the sandbox until you recover them:",
        items.len()
    );
    for i in items.iter().take(10) {
        t += &format!("\n  {}", i.path);
    }
    if items.len() > 10 {
        t += &format!("\n  ... and {} more", items.len() - 10);
    }
    crate::note::info(t);
    crate::note::info(format!(
        "list them with `paddock files {id}`, copy them with `paddock recover {id} PATH`"
    ));
}
