//! The seed of `paddock macvm setup`, platform-neutral. The host writes
//! nothing into the guest disk: it prepares a seed folder in the golden
//! folder (`seed/`), and the guided boot shares it read-only with the
//! guest as `paddock-seed`. The person runs one command in Terminal of the
//! guest ([`command`]), and `install.sh` puts the agent folder and the
//! launchd plist in place with the owners that launchd wants. No step
//! needs root on the host.
//!
//! The seed folder:
//!
//! ```text
//! seed/install.sh                  packaging/macos/guest/seed-install.sh
//! seed/dev.paddock.guest.plist     the launchd plist
//! seed/agent/launch                the launch script
//! seed/agent/current/paddock-guest the agent, when this build has one
//! ```
//!
//! `agent/` is the same tree as `/Library/Application Support/paddock`
//! in the guest. The folder is written as `seed.partial` and renamed, so
//! a crash leaves a partial that the next setup removes.

use super::guest_files;
use super::layout;
use super::proto::SHARE_ROOT;
use super::text;
use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The name of the share in the guest: `/Volumes/My Shared Files/paddock-seed`.
pub const SHARE: &str = "paddock-seed";
/// The script that the person runs with sudo in the guest.
pub const INSTALL: &str = include_str!("../../packaging/macos/guest/seed-install.sh");
/// Its name in the seed folder.
pub const INSTALL_FILE: &str = "install.sh";
/// The folder that becomes `/Library/Application Support/paddock`.
pub const AGENT_DIR: &str = "agent";

/// The name of the plist in the seed folder.
pub fn plist_file() -> String {
    format!("{}.plist", guest_files::LABEL)
}

/// The path of `install.sh` in the guest.
pub fn guest_install_path() -> String {
    format!("{SHARE_ROOT}/{SHARE}/{INSTALL_FILE}")
}

/// The one command that the guided panel shows.
pub fn command() -> String {
    format!("sudo /bin/sh \"{}\"", guest_install_path()) // not user text
}

/// The files that a complete seed folder holds.
fn required(dir: &Path) -> [PathBuf; 3] {
    [
        dir.join(INSTALL_FILE),
        dir.join(plist_file()),
        dir.join(AGENT_DIR).join("launch"),
    ]
}

/// True when `dir` is a complete seed folder with the files of this
/// paddock: the guided boot can share it as it is.
pub fn ready(dir: &Path) -> bool {
    let same = |p: &Path, want: &str| std::fs::read_to_string(p).is_ok_and(|t| t == want);
    let [install, plist, launch] = required(dir);
    same(&install, INSTALL)
        && same(&plist, guest_files::PLIST)
        && same(&launch, guest_files::LAUNCH)
}

/// Write the seed folder `dir` from scratch: `dir.partial` first, then
/// the rename. An earlier folder or partial goes. `agent` is the
/// `paddock-guest` binary, `None` when this build has none.
pub fn prepare(dir: &Path, agent: Option<&Path>) -> Result<()> {
    let tmp = layout::partial(dir);
    layout::remove_any(&tmp).with_context(|| text::cannot_remove(&tmp))?;
    let current = tmp.join(AGENT_DIR).join("current");
    std::fs::create_dir_all(&current).with_context(|| text::cannot_make(&current))?;
    write(&tmp.join(INSTALL_FILE), INSTALL, 0o644)?;
    write(&tmp.join(plist_file()), guest_files::PLIST, 0o644)?;
    write(
        &tmp.join(AGENT_DIR).join("launch"),
        guest_files::LAUNCH,
        0o755,
    )?;
    if let Some(agent) = agent {
        let to = current.join(guest_files::BINARY);
        std::fs::copy(agent, &to).with_context(|| text::cannot_copy(agent, &to))?;
        set_mode(&to, 0o755)?;
        std::fs::File::open(&to)
            .and_then(|f| f.sync_all())
            .with_context(|| text::cannot_write(&to))?;
    }
    for d in [&tmp, &tmp.join(AGENT_DIR), &current] {
        set_mode(d, 0o755)?;
    }
    layout::remove_any(dir).with_context(|| text::cannot_remove(dir))?;
    layout::commit(&tmp).with_context(|| text::cannot_commit(dir))?;
    Ok(())
}

/// Remove the seed folder after the seal: the image holds the files now.
pub fn remove(dir: &Path) -> Result<()> {
    layout::remove_any(dir).with_context(|| text::cannot_remove(dir))?;
    layout::remove_any(&layout::partial(dir)).with_context(|| text::cannot_remove(dir))
}

fn write(path: &Path, body: &str, mode: u32) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path).with_context(|| text::cannot_write(path))?;
    f.write_all(body.as_bytes())
        .and_then(|()| f.sync_all())
        .with_context(|| text::cannot_write(path))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| text::cannot_write(path))
}

#[cfg(test)]
#[path = "seed_tests.rs"]
mod tests;
