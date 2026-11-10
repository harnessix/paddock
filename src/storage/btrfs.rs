//! Btrfs allocation information that statvfs cannot provide.
use crate::env::HostEnv;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, PartialEq, Eq)]
struct Usage {
    unallocated: u64,
    metadata_size: u64,
    metadata_used: u64,
}

fn number_after(line: &str, label: &str) -> Option<u64> {
    line.split_once(label)?
        .1
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn parse(text: &str) -> Option<Usage> {
    let mut unallocated = None;
    let mut metadata_size = 0u64;
    let mut metadata_used = 0u64;
    let mut metadata_found = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("Device unallocated:") {
            unallocated = number_after(line, "Device unallocated:");
        }
        if line.starts_with("Metadata,") || line.starts_with("Data+Metadata,") {
            metadata_size = metadata_size.checked_add(number_after(line, "Size:")?)?;
            metadata_used = metadata_used.checked_add(number_after(line, "Used:")?)?;
            metadata_found = true;
        }
    }
    if !metadata_found || metadata_size == 0 || metadata_used > metadata_size {
        return None;
    }
    Some(Usage {
        unallocated: unallocated?,
        metadata_size,
        metadata_used,
    })
}

fn line(path: &Path, total: u64, usage: &Usage) -> String {
    let low = usage.unallocated < (1u64 << 30).min(total / 20);
    let metadata_high =
        u128::from(usage.metadata_used) * 100 >= u128::from(usage.metadata_size) * 90;
    let mark = if low || metadata_high {
        "WARNING"
    } else {
        "ok"
    };
    format!(
        "  {mark:<8} {}: Btrfs unallocated {}, metadata used {} of {}{}{}",
        path.display(),
        super::human(usage.unallocated),
        super::human(usage.metadata_used),
        super::human(usage.metadata_size),
        if low {
            "; unallocated space is low"
        } else {
            ""
        },
        if metadata_high {
            "; metadata is at least 90% used"
        } else {
            ""
        }
    )
}

fn probe(env: &HostEnv, path: &Path, total: u64, is_btrfs: bool) -> Option<String> {
    if !is_btrfs {
        return None;
    }
    let failure = || {
        format!(
            "  note     {}: could not read Btrfs unallocated space and metadata use",
            path.display()
        )
    };
    let Some(exe) = env.which("btrfs") else {
        return Some(failure());
    };
    let output = Command::new(exe)
        .args(["filesystem", "usage", "--raw"])
        .arg(path)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Some(usage) = output
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse(&String::from_utf8_lossy(&o.stdout)))
    else {
        return Some(failure());
    };
    Some(line(path, total, &usage))
}

pub(super) fn doctor(env: &HostEnv, path: &Path, total: u64) -> Option<String> {
    let at = path.ancestors().find(|p| p.exists())?;
    probe(env, at, total, super::sizes::on_btrfs(at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    const USAGE: &str = "Overall:\n    Device unallocated: 1024\nMetadata,DUP: Size:1000, Used:960 (96.00%)\nMetadata,single: Size:100, Used:90 (90.00%)\n";

    #[test]
    fn reads_unallocated_and_all_metadata_profiles() {
        let u = parse(USAGE).unwrap();
        assert_eq!(
            u,
            Usage {
                unallocated: 1024,
                metadata_size: 1100,
                metadata_used: 1050
            }
        );
        let output = line(Path::new("/fake"), 100 << 30, &u);
        assert!(
            output.contains("WARNING")
                && output.contains("unallocated space is low")
                && output.contains("metadata is at least 90%")
        );
        assert!(parse("Device unallocated: 3\n").is_none());
        let healthy = Usage {
            unallocated: 20 << 30,
            metadata_size: 100,
            metadata_used: 50,
        };
        assert!(line(Path::new("/fake"), 100 << 30, &healthy).contains("ok"));
    }

    #[test]
    fn doctor_probe_uses_raw_output_and_reports_unavailable_data() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("btrfs");
        std::fs::write(&script, format!("#!/bin/sh\n[ \"$1 $2 $3\" = 'filesystem usage --raw' ] || exit 1\n/bin/cat <<'DATA'\n{USAGE}DATA\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = HostEnv::from_parts(
            t.path().join("home"),
            "fake".into(),
            1000,
            1000,
            BTreeMap::from([("PATH".into(), bin.to_string_lossy().into_owned())]),
        );
        let output = probe(&env, t.path(), 100 << 30, true).unwrap();
        assert!(
            output.contains("WARNING") && output.contains("metadata"),
            "{output}"
        );
        assert!(probe(&env, t.path(), 100 << 30, false).is_none());
        std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(probe(&env, t.path(), 100 << 30, true)
            .unwrap()
            .contains("could not read"));
    }
}
