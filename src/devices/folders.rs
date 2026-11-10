//! One level of folders in `/dev/paddock`: the device `mail/in` is the
//! file `in` in the folder `mail`. A folder exists while a device is in it.
//! Its inode lies between the fixed inodes and the first device.

/// The inode of the first folder. Folders stay below the first device.
pub const FIRST_DIR_INO: u64 = 4;
/// The most folders: their inodes end before `FIRST_DEVICE_INO` (16).
pub const MAX_DIRS: usize = 12;

/// The folder part of a device name, if it has one.
fn folder(name: &str) -> Option<&str> {
    name.split_once('/').map(|(d, _)| d)
}

/// The folder names, in the order of their first device.
pub fn dirs<'a>(names: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for d in names.iter().filter_map(|n| folder(n)) {
        if !out.contains(&d) && out.len() < MAX_DIRS {
            out.push(d);
        }
    }
    out
}

pub fn dir_ino(names: &[&str], dir: &str) -> Option<u64> {
    dirs(names)
        .iter()
        .position(|d| *d == dir)
        .map(|i| FIRST_DIR_INO + i as u64)
}

pub fn dir_name(names: &[&str], ino: u64) -> Option<String> {
    let i = usize::try_from(ino.checked_sub(FIRST_DIR_INO)?).ok()?;
    dirs(names).get(i).map(|d| d.to_string())
}

/// The index of the device `name` in the folder `dir` (None: the top).
pub fn find(names: &[&str], dir: Option<&str>, name: &str) -> Option<usize> {
    names.iter().position(|n| match dir {
        Some(d) => n.strip_prefix(d).and_then(|r| r.strip_prefix('/')) == Some(name),
        None => *n == name && folder(n).is_none(),
    })
}

/// The devices of one folder (None: the top), with their file names.
pub fn entries(names: &[&str], dir: Option<&str>) -> Vec<(usize, String)> {
    names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match (dir, n.split_once('/')) {
            (Some(d), Some((f, base))) if f == d => Some((i, base.to_string())),
            (None, None) => Some((i, n.to_string())),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slash_puts_a_device_in_a_folder() {
        let names = ["wait", "mail/in", "mail/out", "ask"];
        assert_eq!(dirs(&names), ["mail"]);
        assert_eq!(dir_ino(&names, "mail"), Some(FIRST_DIR_INO));
        assert_eq!(dir_name(&names, FIRST_DIR_INO).as_deref(), Some("mail"));
        assert_eq!(dir_name(&names, FIRST_DIR_INO + 1), None);
        assert_eq!(find(&names, Some("mail"), "out"), Some(2));
        assert_eq!(find(&names, None, "in"), None);
        assert_eq!(find(&names, None, "mail/in"), None);
        assert_eq!(find(&names, None, "ask"), Some(3));
        assert_eq!(
            entries(&names, Some("mail")),
            [(1, "in".to_string()), (2, "out".to_string())]
        );
        assert_eq!(entries(&names, None).len(), 2);
    }
}
