//! The one-time host approval of a project `paddock.toml` that widens
//! access, and the hex helpers of its hash.

use super::{Base, Loaded, Manifest};
use crate::env::HostEnv;
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

fn approval_file(env: &HostEnv, project_file: &Path) -> PathBuf {
    let key = hex(&Sha256::digest(project_file.to_string_lossy().as_bytes()))[..16].to_string();
    env.paddock_state().join("approved").join(key)
}

fn widening_hash(m: &Manifest, base: &Base) -> String {
    hex(&Sha256::digest(m.widening(base).to_string().as_bytes()))
}

/// Bytes from lowercase or uppercase hex. None for odd lengths or other
/// characters.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// With `--network`, an approval that also covers the mode of the file
/// counts too.
pub fn is_approved(env: &HostEnv, project_file: &Path, m: &Manifest, base: &Base) -> bool {
    let Ok(s) = std::fs::read_to_string(approval_file(env, project_file)) else {
        return false;
    };
    let full = Base {
        cli_network: false,
        ..base.clone()
    };
    s.trim() == widening_hash(m, base) || s.trim() == widening_hash(m, &full)
}

pub fn approve(env: &HostEnv, project_file: &Path, m: &Manifest, base: &Base) -> Result<()> {
    let f = approval_file(env, project_file);
    crate::privfs::write(&f, widening_hash(m, base))?;
    Ok(())
}

/// Make sure that any widening in the project file is approved. Ask on a
/// terminal; refuse without one.
pub fn require_approval(env: &HostEnv, loaded: &Loaded, assume_yes: bool) -> Result<()> {
    let Some((file, m)) = &loaded.project else {
        return Ok(());
    };
    let base = &loaded.base;
    if !m.widens(base) || is_approved(env, file, m, base) {
        return Ok(());
    }
    let summary = serde_json::to_string_pretty(&m.widening(base))?;
    eprintln!(
        "paddock: {} asks for more access than the defaults:\n{summary}\n\
         The project directory is writable from inside the sandbox, so an agent can edit this file.",
        file.display()
    );
    if assume_yes {
        return approve(env, file, m, base);
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "not approved. Review the file, then run: paddock approve {}",
            file.parent().unwrap().display()
        );
    }
    eprint!("Approve this access? [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "y" | "Y" | "yes") {
        approve(env, file, m, base)
    } else {
        bail!("not approved")
    }
}
