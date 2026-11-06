//! The README section "App mode on macOS in a guest VM": it names every
//! command of the route, every reset tier, the backups, the input method
//! rule, VoiceOver, Spaces, each row of the limits table of `limits.rs`,
//! each number of the bench baseline, and one "not confirmed" sentence per
//! open Mac check of [`OPEN_MAC_CHECKS`].

use std::collections::BTreeMap;

/// The Mac checks that are still open, as X of the README sentence
/// "paddock has not confirmed X on a Mac yet." When a check is confirmed
/// on a Mac, its line goes from here and from the README together.
const OPEN_MAC_CHECKS: &[&str] = &[
    "the build, clippy and tests of the bench on macOS (`macvm-bench.sh build`)",
    "the gates of `paddock macvm bench`, which refuse a session that started without `--bench` (`macvm-bench.sh gates`)",
    "the numbers of the bench: the baseline above holds the targets until `packaging/macos/bench.sh --baseline` writes a measured run (`macvm-bench.sh bench`)",
    "the JSON report of a bench run (`macvm-bench.sh json`)",
    "the MISS line of a target that a bench run misses (`macvm-bench.sh env`)",
    "that a key reaches the guest app",
    "the offline seed: the files on the guest disk as your user, their root owner, `fsck_apfs -n` and the agent start by the first-boot service (`macvm-offline.sh seed`)",
    "the offline trust: the Accessibility row in the permission database of the stopped guest and the report of the agent after the next boot (`macvm-offline.sh trust`)",
    "that the Accessibility permission stays after a self-update of the agent (`macvm-offline.sh update`)",
    "that every configure step ends with `ok` within its limit on a macOS 27 guest (`macvm-first-run.sh` unattended)",
    "the offline `~/Downloads` link: a link to the downloads share with the owner 501:20 in the stopped guest (`macvm-offline.sh downloads`)",
    "that the end of a session shuts the guest down with `sudo shutdown` and no permission prompt (`macvm-offline.sh shutdown`)",
    "that the unattended setup needs no click of the person before `golden ready` (`macvm-first-run.sh` clicks)",
    "the seed of the guided step: the shared seed folder, the install command in Terminal of the guest and the agent start without a new login (`macvm-golden.sh` seed, `macvm-first-run.sh` clicks)",
    "that the Type buttons of the guided window reach the account fields of Setup Assistant and Terminal of the guest (`macvm-first-run.sh` typing)",
    "that the guided window fits a 13-inch screen with the Dock visible and that a resize keeps the aspect of the guest view (`macvm-first-run.sh` window)",
    "that a refused password shows the question of the panel and that the configure step passes with the typed one (`macvm-first-run.sh` password)",
    "the window list, the titles and the moves through Accessibility in the guest",
    "the guest layout of the host key codes and Pinyin in the guest",
    "the marks on two windows of one session (`macvm-marks.sh marks`)",
    "that `paddock freeze` stops the guest and `paddock thaw` lets it go on (`macvm-marks.sh freeze`)",
    "that a window on a screen that goes away comes to the main screen (`macvm-marks.sh screen`)",
    "that a quit of a frozen session saves it (`macvm-marks.sh quit-frozen`)",
    "that the guest window follows a host drag within 50 ms at the 95th percentile, with no frame more than 2 points apart after the move (`macvm-session.sh drag`)",
    "that the clip of a session window has the corner radius that the agent measured, within 1 point, for TextEdit and a toolbar window (`macvm-session.sh radius`)",
    "that a guest window in full screen puts its session window in full screen with the whole guest display, and back (`macvm-session.sh fullscreen`)",
    "that a guest link opens through the link helper and that a login callback on the host loopback reaches a guest server, and only that port (`macvm-session.sh login`)",
];

fn readme() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap()
}

/// The section, up to the next section of the same level.
fn section() -> String {
    let text = readme();
    let start = text
        .find("\n### App mode on macOS in a guest VM\n")
        .expect("the README has the section");
    let rest = &text[start + 1..];
    let end = rest[4..]
        .find("\n### ")
        .map(|i| i + 4)
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// One subsection of the section, up to the next `####`.
fn sub(section: &str, title: &str) -> String {
    let head = format!("\n#### {title}\n");
    let start = section
        .find(&head)
        .unwrap_or_else(|| panic!("no #### {title}"));
    let rest = &section[start + head.len()..];
    let end = rest.find("\n#### ").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn json(path: &str) -> serde_json::Value {
    let p = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// `p50 6 s, p95 10 s`: the keys of a target, with the values of `m`.
fn cell(m: &serde_json::Value, keys: &[&str], unit: &str) -> String {
    let unit = match unit {
        "s" => " s",
        "ms" => " ms",
        "pct" => " %",
        "gib" => " GiB",
        _ => "",
    };
    keys.iter()
        .filter_map(|k| {
            let v = m.get(*k)?.as_f64()?;
            let v = if v.fract() == 0.0 {
                format!("{}", v as i64)
            } else {
                format!("{v}")
            };
            Some(format!("{k} {v}{unit}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The rows of a markdown table: the first cell to the other cells.
fn rows(text: &str) -> BTreeMap<String, Vec<String>> {
    text.lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| ---"))
        .map(|l| {
            let cells: Vec<String> = l
                .trim_matches('|')
                .split(" | ")
                .map(|c| c.trim().to_string())
                .collect();
            (cells[0].clone(), cells[1..].to_vec())
        })
        .collect()
}

#[test]
fn macvm_section_names_commands_tiers_and_numbers() {
    let s = section();
    // Every command of the route.
    for cmd in [
        "paddock macvm setup",
        "paddock macvm status",
        "paddock macvm update | rollback | remove [--profiles]",
        "paddock macvm bench CASE --json",
        "paddock app run chatgpt-desktop",
        "paddock app install chatgpt-desktop",
        "paddock app status NAME [--json]",
        "paddock app stop | files | rules NAME",
        "paddock app log NAME [--events | --shim | --guest] [--follow]",
        "paddock app reset NAME [--data | --all]",
        "paddock app backup NAME [--out FILE] [--stop]",
        "paddock app restore NAME FILE|TIME",
        "paddock app backups NAME",
        "paddock app install --drop",
    ] {
        assert!(s.contains(cmd), "the section lacks the command {cmd:?}");
    }
    // Every reset tier, in the table.
    let reset = rows(&sub(&s, "Reset and backups"));
    for tier in [
        "`paddock app reset NAME`",
        "`paddock app reset NAME --data`",
        "`paddock app reset NAME --all`",
    ] {
        assert!(reset.contains_key(tier), "no row for the tier {tier}");
    }
    for word in [
        "backup",
        "input method",
        "VoiceOver",
        "Spaces",
        "full screen",
        "drag",
        "paddock freeze",
        "vm",
    ] {
        assert!(s.contains(word), "the section lacks {word:?}");
    }
    // The failure behavior in the user's words: a crash, sleep, a full
    // disk, an update.
    let fails = sub(&s, "When something fails");
    for word in ["crashes", "sleeps", "disk gets full", "is updated"] {
        assert!(fails.contains(word), "the failure part lacks {word:?}");
    }
    // Each row of the limits table, as `limits::TABLE` has it.
    let limits = rows(&sub(&s, "Limits"));
    for r in super::limits::TABLE {
        assert_eq!(
            limits.get(r.what),
            Some(&vec![r.value.to_string(), r.told.to_string()]),
            "the limits row {:?}",
            r.what
        );
    }
    // Each number of the targets and of the baseline.
    let targets = json("packaging/macos/bench/targets.json");
    let baseline = json("packaging/macos/bench/baseline.json");
    let perf = rows(&sub(&s, "Performance"));
    let mut n = 0;
    for (name, t) in targets.as_object().unwrap() {
        if name.starts_with('_') {
            continue;
        }
        let keys: Vec<&str> = ["p50", "p95", "mean"]
            .into_iter()
            .filter(|k| t.get(*k).is_some())
            .collect();
        let unit = t["unit"].as_str().unwrap();
        let want = vec![
            cell(t, &keys, unit),
            cell(&baseline["measures"][name], &keys, unit),
        ];
        assert_eq!(
            perf.get(&format!("`{name}`")),
            Some(&want),
            "the performance row of {name}"
        );
        n += 1;
    }
    assert!(n >= 13, "every measure of the targets");
}

#[test]
fn unconfirmed_lines_match_open_mac_checks() {
    let open = OPEN_MAC_CHECKS;
    assert!(!open.is_empty());
    let part = sub(&section(), "Not confirmed on a Mac yet");
    for x in open {
        let line = format!("- paddock has not confirmed {x} on a Mac yet.");
        assert!(part.contains(&line), "the README lacks {line:?}");
    }
    // No sentence for a check that is not open.
    let count = part.matches("paddock has not confirmed ").count();
    assert_eq!(count, open.len(), "one sentence per open check");
}
