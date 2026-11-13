//! Whether the disk of a folder is encrypted.
//!
//! paddock does not encrypt the store itself: an encrypted volume of its
//! own cannot share data with the project, so every copy would cost its
//! full size. It relies on the encryption of the disk (LUKS, FileVault)
//! and reports what it finds.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "kind", rename_all = "snake_case")]
pub enum Encryption {
    /// The disk is encrypted, with the name of the method.
    Encrypted(String),
    /// paddock found the disk, and it is not encrypted.
    NotEncrypted,
    /// paddock cannot tell.
    Unknown,
}

impl Encryption {
    pub fn word(&self) -> String {
        match self {
            Encryption::Encrypted(k) => format!("encrypted ({k})"),
            Encryption::NotEncrypted => "not encrypted".into(),
            Encryption::Unknown => "unknown".into(),
        }
    }

    /// The line of `paddock doctor` for the folder of the stores.
    pub fn doctor_line(&self, store: &Path) -> String {
        match self {
            Encryption::Encrypted(k) => format!(
                "  copies   {}: the disk is encrypted ({k})",
                store.display()
            ),
            Encryption::NotEncrypted => format!(
                "  copies   {}: the disk is not encrypted. paddock does not encrypt copies and backups itself. Turn on the encryption of the disk (LUKS, FileVault) to protect them",
                store.display()
            ),
            Encryption::Unknown => format!(
                "  copies   {}: paddock cannot tell whether the disk is encrypted. It does not encrypt copies and backups itself",
                store.display()
            ),
        }
    }
}

/// The lines of `paddock doctor`: where the stores are, and whether their
/// disk is encrypted.
pub fn doctor_text(env: &crate::env::HostEnv) -> String {
    match super::Config::load(env) {
        Ok(cfg) => {
            let root = cfg.root(env);
            of(&root).doctor_line(&root)
        }
        Err(e) => format!("  copies   {e:#}"),
    }
}

/// The nearest folder of `path` that exists, with symlinks resolved.
fn existing(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    loop {
        if let Ok(real) = std::fs::canonicalize(&p) {
            return real;
        }
        if !p.pop() {
            return PathBuf::from("/");
        }
    }
}

/// The state of the disk that holds `path`.
pub fn of(path: &Path) -> Encryption {
    let real = existing(path);
    if cfg!(target_os = "macos") {
        return macos(&real);
    }
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Encryption::Unknown;
    };
    linux(&info, Path::new("/sys"), &real, &|src| {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(src).ok()?;
        // dev_t is u64 on Linux and i32 on macOS.
        #[allow(clippy::unnecessary_cast)]
        let r = m.rdev() as libc::dev_t;
        // SAFETY: major and minor only split the number.
        Some((libc::major(r) as u64, libc::minor(r) as u64))
    })
}

fn macos(real: &Path) -> Encryption {
    // FileVault covers the data volume, which holds the home.
    let Ok(out) = std::process::Command::new("/usr/bin/fdesetup")
        .arg("status")
        .output()
    else {
        return Encryption::Unknown;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let _ = real;
    if text.contains("FileVault is On") {
        Encryption::Encrypted("FileVault".into())
    } else if text.contains("FileVault is Off") {
        Encryption::NotEncrypted
    } else {
        Encryption::Unknown
    }
}

/// One mount of `/proc/self/mountinfo`.
struct MountLine {
    dev: (u64, u64),
    point: PathBuf,
    fstype: String,
    source: String,
}

fn unescape(s: &str) -> String {
    // mountinfo writes a space as \040, and so on, in octal.
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(u8::is_ascii_digit) {
            if let Ok(n) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(n);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse(info: &str) -> Vec<MountLine> {
    info.lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let f: Vec<&str> = left.split(' ').collect();
            let (maj, min) = f.get(2)?.split_once(':')?;
            let mut r = right.split(' ');
            Some(MountLine {
                dev: (maj.parse().ok()?, min.parse().ok()?),
                point: PathBuf::from(unescape(f.get(4)?)),
                fstype: r.next()?.to_string(),
                source: unescape(r.next()?),
            })
        })
        .collect()
}

/// The device `dev`, or a device below it, is a dm-crypt volume.
fn crypt(sys: &Path, dev: (u64, u64), depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    let dir = sys.join(format!("dev/block/{}:{}", dev.0, dev.1));
    if let Ok(uuid) = std::fs::read_to_string(dir.join("dm/uuid")) {
        if let Some(rest) = uuid.trim().strip_prefix("CRYPT-") {
            return Some(if rest.starts_with("LUKS") {
                "LUKS".into()
            } else {
                "dm-crypt".into()
            });
        }
    }
    for e in std::fs::read_dir(dir.join("slaves"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let Ok(text) = std::fs::read_to_string(e.path().join("dev")) else {
            continue;
        };
        let Some((maj, min)) = text.trim().split_once(':') else {
            continue;
        };
        let (Ok(maj), Ok(min)) = (maj.parse(), min.parse()) else {
            continue;
        };
        if let Some(k) = crypt(sys, (maj, min), depth + 1) {
            return Some(k);
        }
    }
    None
}

/// The state of `real` from the mount table `info` and the block devices
/// below `sys`. `dev_of` gives the device number of a device file.
pub fn linux(
    info: &str,
    sys: &Path,
    real: &Path,
    dev_of: &dyn Fn(&Path) -> Option<(u64, u64)>,
) -> Encryption {
    // The longest mount point above the path. The last line wins a tie:
    // it is the mount on top.
    let mounts = parse(info);
    let Some(m) = mounts
        .iter()
        .filter(|m| real.starts_with(&m.point))
        .max_by_key(|m| m.point.components().count())
    else {
        return Encryption::Unknown;
    };
    match m.fstype.as_str() {
        "ecryptfs" => return Encryption::Encrypted("eCryptfs".into()),
        t if t.starts_with("fuse.gocryptfs") => return Encryption::Encrypted("gocryptfs".into()),
        // Memory, the network and ZFS: no block device to look at.
        "tmpfs" | "ramfs" | "nfs" | "nfs4" | "cifs" | "zfs" | "overlay" => {
            return Encryption::Unknown
        }
        _ => {}
    }
    // btrfs and others give an anonymous device number: take the device
    // file of the source instead.
    let dev = if m.source.starts_with('/') {
        dev_of(Path::new(&m.source)).unwrap_or(m.dev)
    } else {
        m.dev
    };
    if !sys.join(format!("dev/block/{}:{}", dev.0, dev.1)).exists() {
        return Encryption::Unknown;
    }
    match crypt(sys, dev, 0) {
        Some(k) => Encryption::Encrypted(k),
        None => Encryption::NotEncrypted,
    }
}
