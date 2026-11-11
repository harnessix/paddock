//! Where the macOS guest lives on the host: every path of the `macvm`
//! folder, and the partial rule that every writer of the subsystem follows.
//!
//! The folder is `<paddock data dir>/macvm/`, next to the Linux `vm/`
//! folder of `src/vm/assets.rs`. Spotlight stays out of the golden and the
//! profiles: those folders carry a `.noindex` suffix. Every file that
//! paddock makes under `golden/` or under a profile is written under the
//! name `<name>.partial` and renamed when complete. A `.partial` that a
//! later start finds means a crash, never a half file that boots.

use super::text;
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::io;
use std::path::{Path, PathBuf};

/// The suffix of a file that is not complete yet.
pub const PARTIAL: &str = ".partial";
/// The folder of the golden, with the suffix that keeps Spotlight out.
pub const GOLDEN_DIR: &str = "golden.noindex";
/// A golden build in progress (`paddock macvm update`).
pub const GOLDEN_NEW_DIR: &str = "golden.new.noindex";
/// The previous golden, until the first clean session on the new one.
pub const GOLDEN_PREV_DIR: &str = "golden.prev.noindex";
/// The profiles, one folder per app and home id.
pub const PROFILES_DIR: &str = "profiles.noindex";
/// The manifest of a golden.
pub const MANIFEST: &str = "manifest.toml";

/// The `macvm` folder and its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// `<paddock data dir>/macvm/`.
    pub root: PathBuf,
    /// The sealed golden, or the golden in the making.
    pub golden: Golden,
    /// A golden build in progress (`paddock macvm update`).
    pub golden_new: Golden,
    /// The previous golden, kept for `paddock macvm rollback`.
    pub golden_prev: Golden,
    /// The restore images, one per version, removable after the build.
    pub ipsw: PathBuf,
    /// The agent of each paddock version, shared read-only into sessions.
    pub agent: PathBuf,
    /// The `Running` records of the sessions.
    pub sessions: PathBuf,
    /// Profiles removed by `reset --all`, kept 7 days.
    pub trash: PathBuf,
    /// The profiles, `profiles.noindex/<app>/<home-id>/`.
    pub profiles: PathBuf,
    /// The Dock faces of the sessions.
    pub shims: PathBuf,
    /// The lock that keeps two setups apart.
    pub setup_lock: PathBuf,
}

/// The files of one golden folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Golden {
    pub dir: PathBuf,
    /// macOS version, build, hardware model, agent version, time, state.
    pub manifest: PathBuf,
    /// The raw APFS image, 64 GiB sparse, 0444 and `uchg` after the seal.
    pub disk: PathBuf,
    /// The auxiliary storage (NVRAM), sealed with the disk.
    pub aux: PathBuf,
    /// `VZMacHardwareModel.dataRepresentation`.
    pub hardware_model: PathBuf,
    /// The `VZMacMachineIdentifier` of the golden build only.
    pub machine_id: PathBuf,
    /// The serial console of the golden boots.
    pub console: PathBuf,
    /// The seed folder that the guided boot shares with the guest
    /// (`seed.rs`). Gone after the seal.
    pub seed: PathBuf,
    /// The event log of the build (`setup_events.rs`).
    pub events: PathBuf,
}

impl Golden {
    /// The golden files under `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            manifest: dir.join(MANIFEST),
            disk: dir.join("disk.img"),
            aux: dir.join("aux.img"),
            hardware_model: dir.join("hardware-model"),
            machine_id: dir.join("machine-id"),
            console: dir.join("console.log"),
            seed: dir.join("seed"),
            events: dir.join("events.jsonl"),
            dir,
        }
    }

    /// The files that a seal protects.
    pub fn sealed_files(&self) -> [&Path; 2] {
        [&self.disk, &self.aux]
    }

    /// Every `.partial` entry of the folder. Empty when the folder is
    /// missing.
    pub fn partials(&self) -> Vec<PathBuf> {
        partials_in(&self.dir)
    }

    /// Remove every `.partial` entry of the folder, files and folders.
    pub fn remove_partials(&self) -> io::Result<Vec<PathBuf>> {
        let list = self.partials();
        for p in &list {
            remove_any(p)?;
        }
        Ok(list)
    }

    /// Read the manifest. `None` when the folder has none.
    pub fn read_manifest(&self) -> Result<Option<super::golden::Manifest>> {
        match std::fs::read_to_string(&self.manifest) {
            Ok(text) => super::golden::Manifest::parse(&text, &self.manifest).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| text::cannot_read(&self.manifest)),
        }
    }

    /// Write the manifest as `.partial`, then rename it into place. A
    /// reader never sees half a manifest, and a state is on disk only
    /// when the write completed.
    pub fn write_manifest(&self, m: &super::golden::Manifest) -> Result<()> {
        crate::privfs::dir(&self.dir).with_context(|| text::cannot_make(&self.dir))?;
        let toml = m.to_toml()?;
        let tmp = partial(&self.manifest);
        crate::privfs::write(&tmp, toml).with_context(|| text::cannot_write(&tmp))?;
        commit(&tmp).with_context(|| text::cannot_commit(&self.manifest))
    }
}

impl Layout {
    /// The folder of this host: `<paddock data dir>/macvm/`.
    pub fn new(env: &HostEnv) -> Self {
        Self::at(env.paddock_data().join("macvm"))
    }

    /// The layout under one folder. Tests use a temporary one.
    pub fn at(root: PathBuf) -> Self {
        Self {
            golden: Golden::new(root.join(GOLDEN_DIR)),
            golden_new: Golden::new(root.join(GOLDEN_NEW_DIR)),
            golden_prev: Golden::new(root.join(GOLDEN_PREV_DIR)),
            ipsw: root.join("ipsw"),
            agent: root.join("agent"),
            sessions: root.join("sessions"),
            trash: root.join("trash"),
            profiles: root.join(PROFILES_DIR),
            shims: root.join("shims"),
            setup_lock: root.join("setup.lock"),
            root,
        }
    }

    /// The restore image of one version, `ipsw/<version>.ipsw`.
    pub fn ipsw_file(&self, version: &str) -> PathBuf {
        self.ipsw.join(format!("{}.ipsw", safe_name(version)))
    }

    /// The share folder of the agent of one paddock version,
    /// `agent/<version>/`.
    pub fn agent_dir(&self, paddock_version: &str) -> PathBuf {
        self.root.join(super::version::agent_dir(paddock_version))
    }

    /// The agent binary of one paddock version.
    pub fn agent_file(&self, paddock_version: &str) -> PathBuf {
        self.agent_dir(paddock_version)
            .join(super::guest_files::BINARY)
    }

    /// The files of one profile, `profiles.noindex/<app>/<home-id>/`.
    pub fn profile(&self, app: &str, home_id: &str) -> super::session::Profile {
        super::session::Profile::new(self.profiles.join(safe_name(app)).join(safe_name(home_id)))
    }

    /// The `Running` record of a session of one profile.
    pub fn record(&self, app: &str, home_id: &str) -> PathBuf {
        self.sessions
            .join(format!("{}--{}.json", safe_name(app), safe_name(home_id)))
    }

    /// Every profile folder, `(app, home id, path)`, sorted.
    pub fn profile_dirs(&self) -> Vec<(String, String, PathBuf)> {
        let mut out = Vec::new();
        let Ok(apps) = std::fs::read_dir(&self.profiles) else {
            return out;
        };
        for app in apps.flatten() {
            let Ok(homes) = std::fs::read_dir(app.path()) else {
                continue;
            };
            for home in homes.flatten() {
                if home.path().is_dir() {
                    out.push((
                        app.file_name().to_string_lossy().into_owned(),
                        home.file_name().to_string_lossy().into_owned(),
                        home.path(),
                    ));
                }
            }
        }
        out.sort();
        out
    }
}

/// The `.partial` name of a path: `disk.img` gives `disk.img.partial`.
pub fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(PARTIAL);
    path.with_file_name(name)
}

/// Rename a `.partial` file or folder to its final name. The final name
/// is the partial name without the suffix. An argument without the suffix
/// is an error: the caller mixed up the two names. A file is synced to
/// the disk before the rename, so a crash never leaves a complete name
/// on half a file.
pub fn commit(partial_path: &Path) -> io::Result<()> {
    let Some(final_path) = final_name(partial_path) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            text::not_partial_name(partial_path),
        ));
    };
    if let Ok(f) = std::fs::File::open(partial_path) {
        if f.metadata().map(|m| m.is_file()).unwrap_or(false) {
            f.sync_all()?;
        }
    }
    std::fs::rename(partial_path, final_path)
}

/// The final name of a `.partial` path, or `None` without the suffix.
pub fn final_name(partial_path: &Path) -> Option<PathBuf> {
    let name = partial_path.file_name()?.to_str()?;
    let base = name.strip_suffix(PARTIAL)?;
    if base.is_empty() {
        return None;
    }
    Some(partial_path.with_file_name(base))
}

/// Whether a path has the `.partial` suffix.
pub fn is_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(PARTIAL) && n.len() > PARTIAL.len())
}

/// Every `.partial` entry directly in `dir`, sorted. Empty when the folder
/// is missing.
pub fn partials_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| is_partial(p))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Remove a file or a folder tree. A missing path is fine.
pub fn remove_any(path: &Path) -> io::Result<()> {
    let r = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => Err(e),
    };
    match r {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Remove a golden folder, sealed or not. The seal makes the images
/// read-only and immutable, so the flags and the mode go first.
pub fn remove_golden_dir(g: &Golden) -> Result<()> {
    for f in g.sealed_files() {
        if f.exists() {
            unseal_file(f).with_context(|| text::cannot_unseal(f))?;
        }
    }
    remove_any(&g.dir).with_context(|| text::cannot_remove(&g.dir))
}

/// Seal one file: mode 0444 and the `uchg` flag (macOS), so that no
/// process of the user writes or removes it by accident. A file that is
/// sealed already is unsealed first, because `chmod` fails on an
/// immutable file.
pub fn seal_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    unseal_file(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444))?;
    set_immutable(path, true)
}

/// Take the `uchg` flag and the read-only mode away from one file.
pub fn unseal_file(path: &Path) -> io::Result<()> {
    set_immutable(path, false)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// The `uchg` flag of a file, as `ls -lO` shows it. Always false where
/// the flag does not exist.
pub fn is_immutable(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        std::fs::metadata(path).is_ok_and(|m| m.st_flags() & UF_IMMUTABLE != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

/// `UF_IMMUTABLE` of <sys/stat.h>: the `uchg` flag.
#[cfg(target_os = "macos")]
const UF_IMMUTABLE: u32 = 0x0000_0002;

/// Set or clear the `uchg` flag. No flags exist on Linux, so there it
/// does nothing.
fn set_immutable(path: &Path, on: bool) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL in the path"))?;
        let flags = std::fs::metadata(path)?.st_flags();
        let flags = if on {
            flags | UF_IMMUTABLE
        } else {
            flags & !UF_IMMUTABLE
        };
        // SAFETY: a NUL-terminated path and the flags of <sys/stat.h>.
        if unsafe { libc::chflags(c.as_ptr(), flags) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, on);
        Ok(())
    }
}

/// A version or an id as one path component: letters, digits, `.`, `-`
/// and `_` stay, everything else becomes one `_`, and the name has no
/// `_` at its end and no `.` at its start. A path never escapes its
/// folder this way: `15.3 (24D1)` is `15.3_24D1`, `../x` is `_x`.
pub fn safe_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.starts_with('.') {
        out.remove(0);
    }
    while out.len() > 1 && out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// The SHA-256 of a file as lower-case hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = io::Read::read(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

impl Layout {
    /// Make the share folder `agent/<version>/` of this paddock version
    /// from the agent binary of the install, `paddock-guest` next to
    /// `paddock-vm` (`install_dir`), with its `.sha256` file. `paddock app
    /// run` calls it before every session. The binary of the install is
    /// checked against its hash file when there is one, and the hash is
    /// computed when there is none. A folder whose binary already has that
    /// hash is left as it is. `None` when the install has no agent: the
    /// session runs without the share.
    pub fn agent_share(&self, install_dir: &Path, version: &str) -> Result<Option<PathBuf>> {
        use super::guest_files::{BINARY, BINARY_SHA256};
        let source = install_dir.join(BINARY);
        if !source.is_file() {
            return Ok(None);
        }
        let real = sha256_file(&source).with_context(|| text::cannot_read(&source))?;
        let sha_file = install_dir.join(BINARY_SHA256);
        if let Ok(said) = std::fs::read_to_string(&sha_file) {
            let said = said.trim();
            if !said.eq_ignore_ascii_case(&real) {
                bail!(text::agent_hash_mismatch(&source, said, &real));
            }
        }
        let dir = self.agent_dir(version);
        let binary = dir.join(BINARY);
        let sha = dir.join(BINARY_SHA256);
        let same = std::fs::read_to_string(&sha)
            .is_ok_and(|s| s.trim().eq_ignore_ascii_case(&real))
            && binary.is_file()
            && sha256_file(&binary).is_ok_and(|h| h == real);
        if same {
            return Ok(Some(dir));
        }
        crate::privfs::dir(&dir).with_context(|| text::cannot_make(&dir))?;
        for p in partials_in(&dir) {
            remove_any(&p).with_context(|| text::cannot_remove(&p))?;
        }
        let tmp = partial(&binary);
        std::fs::copy(&source, &tmp).with_context(|| text::cannot_copy(&source, &tmp))?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        commit(&tmp).with_context(|| text::cannot_commit(&binary))?;
        let tmp = partial(&sha);
        crate::privfs::write(&tmp, format!("{real}\n"))
            .with_context(|| text::cannot_write(&tmp))?;
        commit(&tmp).with_context(|| text::cannot_commit(&sha))?;
        Ok(Some(dir))
    }

    /// The agent share folders of other paddock versions, for `paddock gc`.
    pub fn other_agent_dirs(&self, version: &str) -> Vec<(String, PathBuf)> {
        let keep = safe_name(version);
        let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(&self.agent)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                    .filter(|(name, _)| *name != keep)
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
