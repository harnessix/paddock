//! The `protect = "uid"` part of `sudo paddock tack setup` and `sudo
//! paddock tack remove` (Linux): tackd's state folder, the helper
//! accounts, and, on remove, every ACL that tackd granted.

use super::accounts;
use super::protect::{self, Registry};
use super::service::Runner;
use anyhow::{Context as _, Result};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// The users that need a helper account: each user of `people` (and the
/// one who ran sudo) whose configuration has an entry at `uid`.
pub fn wanted(
    people: &[(String, u32)],
    config: &dyn Fn(u32) -> Result<super::config::Tack, String>,
) -> (Vec<(String, u32)>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (name, uid) in people {
        match config(*uid) {
            Ok(c) if protect::wants_uid(&c) => out.push((name.clone(), *uid)),
            Ok(_) => {}
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    out.sort_by_key(|(_, u)| *u);
    out.dedup();
    (out, problems)
}

/// tackd's state folder: root's, 0755, made when it is not there.
pub fn state_folder(state: &Path, owner: u32) -> Result<()> {
    if !state.exists() {
        std::fs::create_dir_all(state)
            .with_context(|| format!("cannot make {}", state.display()))?;
    }
    std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o755))?;
    let m = std::fs::symlink_metadata(state)?;
    if !m.is_dir() || m.uid() != owner {
        anyhow::bail!(
            "{} is not a folder of uid {owner}: remove it and run setup again",
            state.display()
        );
    }
    Ok(())
}

/// Make the helper accounts that `users` need and keep the registry.
/// Returns the lines for the user, and whether every account is ready.
pub fn setup(
    state: &Path,
    users: &[(String, u32)],
    runner: &dyn Runner,
) -> Result<(Vec<String>, bool)> {
    state_folder(state, 0)?;
    let mut lines = Vec::new();
    let mut ok = true;
    let mut reg = match Registry::load(state, 0) {
        Ok(r) => r,
        Err(e) => {
            lines.push(format!("note      {e}; it is written again"));
            Registry::default()
        }
    };
    for (user, uid) in users {
        match accounts::ensure(state, user, *uid, runner) {
            Ok((h, made)) => {
                lines.push(format!(
                    "{}   helper account {} (uid {}) for {user}: programs at protect = \"uid\" run as it",
                    if made { "made" } else { "kept" },
                    h.name,
                    h.uid
                ));
                reg.put(h);
            }
            Err(e) => {
                ok = false;
                lines.push(format!("error     {e:#}"));
            }
        }
    }
    // An owner who is gone keeps no entry.
    reg.helpers.retain(|h| accounts::by_uid(h.owner).is_some());
    reg.save(state)?;
    if users.is_empty() {
        lines.push("no user has an allowlist entry at protect = \"uid\" yet: no helper account is needed (run sudo paddock tack setup again after you add one)".into());
    }
    Ok((lines, ok))
}

/// Remove every ACL in the ledger, every helper account (from the
/// registry and by its comment in `/etc/passwd`), and the state folder.
pub fn remove(state: &Path, passwd: &str, runner: &dyn Runner) -> Vec<String> {
    let mut out = Vec::new();
    let reg = Registry::load(state, 0).unwrap_or_default();
    let mut helpers: Vec<(String, u32, u32)> = reg
        .helpers
        .iter()
        .map(|h| (h.name.clone(), h.uid, h.owner))
        .collect();
    for x in accounts::ours_in(passwd) {
        if !helpers.iter().any(|h| h.1 == x.1) {
            helpers.push(x);
        }
    }
    let ledger = super::acl::Ledger::new(state);
    for g in ledger.all() {
        let gid = accounts::by_uid(g.owner).map(|p| p.gid).unwrap_or(g.owner);
        let r = super::acl::revoke(&g, gid, super::acl::MAX_FILES);
        out.push(format!(
            "revoked   the ACLs for uid {} in {} ({} files{})",
            g.helper,
            g.folder.display(),
            r.changed,
            if r.given_back > 0 {
                format!(
                    ", {} files of the helper given to uid {}",
                    r.given_back, g.owner
                )
            } else {
                String::new()
            }
        ));
        for e in r.errors.iter().take(3) {
            out.push(format!("error     {e}"));
        }
    }
    if let Err(e) = ledger.retain(&|_| false) {
        out.push(format!(
            "error     cannot delete {}: {e:#}",
            ledger.path().display()
        ));
    }
    for (name, uid, _) in &helpers {
        let home = accounts::homes(state).join(name);
        match accounts::delete(name, *uid, Some(&home), runner) {
            Ok(l) => out.extend(l),
            Err(e) => out.push(format!("error     {e:#}")),
        }
    }
    if state.exists() {
        match std::fs::remove_dir_all(state) {
            Ok(()) => out.push(format!("deleted   {}", state.display())),
            Err(e) => out.push(format!("error     cannot delete {}: {e}", state.display())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_users_with_an_entry_at_uid_get_a_helper() {
        let people = vec![
            ("alice".to_string(), 1000),
            ("bob".to_string(), 1001),
            ("carol".to_string(), 1002),
        ];
        let cfg = |uid: u32| -> Result<super::super::config::Tack, String> {
            match uid {
                1000 => super::super::config::parse("[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"/usr/bin/gh\"\n").map_err(|e| e.to_string()),
                1001 => super::super::config::parse("[tack.secrets.NPM_TOKEN]\n[[tack.secrets.NPM_TOKEN.allow]]\nprogram = \"/usr/bin/npm\"\n").map_err(|e| e.to_string()),
                _ => Err("bad file".into()),
            }
        };
        let (w, problems) = wanted(&people, &cfg);
        assert_eq!(w, [("alice".to_string(), 1000)]);
        assert_eq!(problems, ["carol: bad file"]);
    }
}
