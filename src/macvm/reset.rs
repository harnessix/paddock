//! `paddock app reset NAME [--data | --all]`: the three tiers over
//! `session::remove`, the lock that refuses a reset while the profile runs,
//! the backup of the pair before `--data`, and the move to `trash/` of
//! `--all`. `session` re-exports it; the tests are in `session_tests.rs`.

use super::backup::Outcome;
use super::layout::{safe_name, Layout};
use super::lock::{LockError, ProfileLock};
use super::session::{remove, Profile, Stamp, What};
use super::text::{self, ResetTier};
use anyhow::{bail, Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// What each tier removes, through `session::remove`. The plain tier
/// keeps the system set with the data disk; `--data` removes the pair;
/// `--all` removes the folder (after its move to the trash).
pub fn reset_removals(tier: ResetTier) -> Vec<What> {
    match tier {
        ResetTier::System => vec![What::State, What::Apps, What::Drop, What::Logs],
        ResetTier::Data => vec![
            What::State,
            What::Stamp,
            What::Apps,
            What::Drop,
            What::Logs,
            What::System,
            What::Aux,
            What::MachineId,
            What::Data,
        ],
        ResetTier::All => vec![What::All],
    }
}

/// What a reset did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reset {
    /// The entries that went.
    pub removed: Vec<PathBuf>,
    /// The backup of the pair (`--data`), or the trash folder (`--all`).
    pub aside: Option<PathBuf>,
}

/// The trash folder of a profile removed now.
pub fn trash_dir(layout: &Layout, app: &str, home: &str, now: u64) -> PathBuf {
    layout.trash.join(format!(
        "{}-{}-{}",
        safe_name(app),
        safe_name(home),
        crate::waiting::format_rfc3339(now).replace(':', "-")
    ))
}

/// One reset. `backup` makes the backup of the pair before `--data`; a
/// skipped backup stops the reset. `rules_file` is the firewall rules
/// file of the app, when it exists; `--all` takes it along into the
/// trash. The profile lock is held for the reset, and a held lock (a
/// running session) refuses with its pid.
pub fn reset(
    layout: &Layout,
    app: &str,
    home: &str,
    tier: ResetTier,
    rules_file: Option<&Path>,
    now: u64,
    backup: &mut dyn FnMut(&Profile) -> Result<Outcome>,
) -> Result<Reset> {
    let profile = layout.profile(app, home);
    if !profile.dir.is_dir() {
        bail!(text::no_profile(app, &profile.dir));
    }
    let lock = match ProfileLock::take(&profile) {
        Ok(l) => l,
        Err(LockError::Held { pid }) => bail!(text::profile_running(app, pid.unwrap_or(0))),
        Err(LockError::Io(e)) => return Err(e).with_context(|| text::cannot_lock(&profile.lock())),
    };
    let mut aside = None;
    if tier == ResetTier::Data {
        match backup(&profile)? {
            Outcome::Made(dir) => aside = Some(dir),
            Outcome::Skipped { needed, free } => {
                bail!(text::reset_needs_backup(app, needed, free))
            }
            Outcome::Nothing => {}
        }
    }
    if tier == ResetTier::All {
        let trash = trash_dir(layout, app, home, now);
        std::fs::create_dir_all(&layout.trash).with_context(|| text::cannot_make(&layout.trash))?;
        std::fs::set_permissions(&layout.trash, std::fs::Permissions::from_mode(0o700))?;
        std::fs::rename(&profile.dir, &trash)
            .with_context(|| text::cannot_move(&profile.dir, &trash))?;
        if let Some(rules) = rules_file {
            let into = trash.join("firewall.toml");
            if std::fs::copy(rules, &into).is_ok() {
                std::fs::set_permissions(&into, std::fs::Permissions::from_mode(0o600))?;
                std::fs::remove_file(rules).with_context(|| text::cannot_remove(rules))?;
            }
        }
        let _ = std::fs::remove_dir(profile.dir.parent().unwrap_or(&profile.dir));
        drop(lock);
        return Ok(Reset {
            removed: vec![profile.dir.clone()],
            aside: Some(trash),
        });
    }
    let mut removed = Vec::new();
    for what in reset_removals(tier) {
        removed.extend(
            remove(&profile, what)
                .with_context(|| text::cannot_remove_part(&format!("{what:?}"), &profile.dir))?,
        );
    }
    if tier == ResetTier::System {
        if let Ok(mut stamp) = Stamp::read(&profile.stamp()) {
            stamp.state_id = None;
            stamp.state_len = None;
            stamp.saved = None;
            stamp.app = None;
            stamp.clean = true;
            stamp.write(&profile.stamp())?;
        }
    }
    drop(lock);
    Ok(Reset { removed, aside })
}
