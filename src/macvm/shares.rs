//! The virtiofs shares of a session (items 1 and 2 of the security
//! checklist), pure: the folders that never cross, the secrets check, the
//! `ws-<n>` names next to the reserved names, the guest paths under
//! `/Volumes/My Shared Files`, and the translation of a path between the
//! host and the guest for the clipboard.
//!
//! The user names the workspace folders. Everything else that the guest
//! sees is one of four folders of paddock: `drop` and `downloads` of the
//! profile, read-write; `apps` of the profile and `agent` of this paddock,
//! read-only. A root named `drop` or `agent` is still `ws-<n>`, so a name
//! of the user never shadows a folder of paddock.

use super::proto::SHARE_ROOT;
use super::session::Profile;
use super::text::{self, ShareWhy};
use anyhow::{bail, Result};
use std::path::{Component, Path, PathBuf};

/// The names that paddock keeps for its own shares.
pub const RESERVED: [&str; 4] = ["drop", "downloads", "apps", "agent"];
/// The prefix of a workspace share name.
pub const WS_PREFIX: &str = "ws-";
/// The folders under the home that hold keys, logins and app data.
pub const PROTECTED_IN_HOME: [&str; 8] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".config/gcloud",
    ".local/share/keyrings",
    "Library",
];
/// The folders whose every child is the system's.
const SYSTEM_FILES: [&str; 4] = ["/private", "/tmp", "/var", "/etc"];
const SYSTEM_APPS: [&str; 3] = ["/Library", "/System", "/Applications"];
/// The budget of the secrets walk per folder, as the flatpak app mode.
const SECRET_ENTRIES: usize = 50_000;

/// A folder the user named, with `--ro`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: PathBuf,
    pub read_only: bool,
}

/// One share of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    pub host: PathBuf,
    pub read_only: bool,
}

impl Share {
    /// Where the guest mounts it.
    pub fn guest_path(&self) -> PathBuf {
        Path::new(SHARE_ROOT).join(&self.name)
    }

    /// True for a workspace folder of the user.
    pub fn is_workspace(&self) -> bool {
        self.name.starts_with(WS_PREFIX)
    }
}

/// The shares and the warnings of a start.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Built {
    pub shares: Vec<Share>,
    pub warnings: Vec<String>,
}

/// A path without `.`, `..` and a trailing slash, with no disk access.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// Why `path` is never shared, or None when it may be. `home` is the
/// home folder of the user.
pub fn refusal(path: &Path, home: &Path) -> Option<ShareWhy> {
    let p = normalize(path);
    if !p.is_absolute() {
        return Some(ShareWhy::Relative);
    }
    let home = normalize(home);
    if p == Path::new("/") {
        return Some(ShareWhy::Root);
    }
    if p == home {
        return Some(ShareWhy::Home);
    }
    if p == Path::new("/Users") {
        return Some(ShareWhy::AllHomes);
    }
    if p == Path::new("/Volumes") {
        return Some(ShareWhy::Volumes);
    }
    if home.starts_with(&p) {
        return Some(ShareWhy::ContainsHome);
    }
    // A folder in the home is the user's, even when the home itself sits
    // under a system folder (a home under /private/var, or a test home in
    // the temporary folder): only the protected folders of the home are
    // refused there. A home that is a system folder itself gets no such
    // pass.
    if p.starts_with(&home) && !is_system_root(&home) {
        let protected = PROTECTED_IN_HOME
            .iter()
            .any(|rel| p.starts_with(home.join(rel)));
        return protected.then_some(ShareWhy::Protected);
    }
    if SYSTEM_FILES.iter().any(|s| p.starts_with(s)) {
        return Some(ShareWhy::SystemFiles);
    }
    if SYSTEM_APPS.iter().any(|s| p.starts_with(s)) {
        return Some(ShareWhy::SystemApps);
    }
    None
}

/// True for `/` and the folders that the refusal names as a whole.
fn is_system_root(p: &Path) -> bool {
    p == Path::new("/")
        || p == Path::new("/Users")
        || p == Path::new("/Volumes")
        || SYSTEM_FILES
            .iter()
            .chain(&SYSTEM_APPS)
            .any(|s| p == Path::new(s))
}

/// The real path of `path` when it exists, else the path without `.`
/// and `..`.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
}

/// The credential-shaped files of a folder, as the secrets detector of
/// the sandbox finds them (`.env`, key files, token files).
pub fn secrets_in(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut left = SECRET_ENTRIES;
    crate::detect::walk_secrets(dir, 0, &mut left, &mut found);
    found.sort();
    found
}

/// The shares of a session: the roots as `ws-<n>`, then the four folders
/// of paddock. Every refused root gives one line that names the path and
/// the reason, and all the lines come back together, so the user fixes
/// the command once. A secrets file in a root stops the start with the
/// file named, unless `unprotected` (`[apps.NAME] unprotected_workspaces`),
/// which turns each into a warning.
pub fn build(
    app: &str,
    roots: &[Root],
    profile: &Profile,
    agent_dir: Option<&Path>,
    home: &Path,
    unprotected: bool,
) -> Result<Built> {
    let mut shares = Vec::new();
    let mut lines = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let real_home = real(home);
    for r in roots {
        let path = normalize(&r.path);
        if let Some(why) = refusal(&path, home) {
            lines.push(text::share_refused(app, &path, &text::share_reason(why)));
            continue;
        }
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        if !path.is_dir() {
            lines.push(text::no_folder_exists(&path));
            continue;
        }
        // A symlink on the way can point a named folder into a refused
        // one, and macOS has /tmp, /var and /etc as links into /private:
        // the real path is checked too, and it is the one that is shared.
        let host = real(&path);
        if let Some(why) = refusal(&host, &real_home) {
            lines.push(text::share_refused(app, &host, &text::share_reason(why)));
            continue;
        }
        if shares.iter().any(|s: &Share| s.host == host) {
            continue;
        }
        shares.push(Share {
            name: format!("{WS_PREFIX}{}", shares.len() + 1), // not user text
            host,
            read_only: r.read_only,
        });
    }
    if !lines.is_empty() {
        bail!(lines.join("\n")); // not user text
    }
    let mut warnings = Vec::new();
    let mut secrets = Vec::new();
    for s in &shares {
        for f in secrets_in(&s.host) {
            if unprotected {
                warnings.push(text::env_file_visible(app, &f));
            } else {
                secrets.push(text::env_file(app, &f));
            }
        }
    }
    if !secrets.is_empty() {
        bail!(secrets.join("\n")); // not user text
    }
    shares.push(Share {
        name: "drop".into(),
        host: profile.drop_dir(),
        read_only: false,
    });
    shares.push(Share {
        name: "downloads".into(),
        host: profile.downloads(),
        read_only: false,
    });
    shares.push(Share {
        name: "apps".into(),
        host: profile.apps(),
        read_only: true,
    });
    if let Some(agent) = agent_dir {
        shares.push(Share {
            name: "agent".into(),
            host: agent.to_path_buf(),
            read_only: true,
        });
    }
    Ok(Built { shares, warnings })
}

/// The host path of a guest path inside a share, or None outside.
pub fn to_host(shares: &[Share], guest: &Path) -> Option<PathBuf> {
    let guest = normalize(guest);
    shares.iter().find_map(|s| {
        guest
            .strip_prefix(s.guest_path())
            .ok()
            .map(|rest| s.host.join(rest))
    })
}

/// The guest path of a host path inside a share, or None outside. The
/// longest share path wins when one share is inside another.
pub fn to_guest(shares: &[Share], host: &Path) -> Option<PathBuf> {
    let host = normalize(host);
    shares
        .iter()
        .filter(|s| host.starts_with(&s.host))
        .max_by_key(|s| s.host.as_os_str().len())
        .and_then(|s| {
            host.strip_prefix(&s.host)
                .ok()
                .map(|rest| s.guest_path().join(rest))
        })
}

#[cfg(test)]
#[path = "shares_tests.rs"]
pub(crate) mod tests;
