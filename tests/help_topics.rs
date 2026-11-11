use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn topics() -> Vec<(String, String)> {
    let mut entries: Vec<_> = std::fs::read_dir(root().join("src/help"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| {
            (
                topic_name(p.file_stem().unwrap().to_str().unwrap()),
                std::fs::read_to_string(p).unwrap(),
            )
        })
        .collect();
    entries.sort();
    entries
}
fn topic_name(stem: &str) -> String {
    let name = stem.strip_prefix("guide-").unwrap_or(stem);
    match name {
        "host" => "host-requests",
        "horse" => "horse-interface",
        "undo" => "undo-runs",
        "files" => "saved-files",
        "logins" => "agent-logins",
        "loft" => "private-notes",
        "turnout" => "scheduled-jobs",
        "trainer" => "agent-review",
        "forage" => "usage-limits",
        "launchers" => "app-launchers",
        "devices" => "device-guide",
        _ => name,
    }
    .to_owned()
}
fn headings(text: &str) -> Vec<&str> {
    let mut fenced = false;
    text.lines()
        .filter(|line| {
            if line.starts_with("```") {
                fenced = !fenced;
                return false;
            }
            !fenced
                && line.starts_with('#')
                && line
                    .split_once(' ')
                    .is_some_and(|(hash, _)| hash.len() <= 6 && hash.chars().all(|c| c == '#'))
        })
        .collect()
}
fn words(text: &str) -> BTreeMap<String, usize> {
    let mut clean = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        clean.push(c);
        if c == ']' && chars.peek() == Some(&'(') {
            chars.next();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
            }
        }
    }
    let mut counts = BTreeMap::new();
    for word in clean
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
    {
        *counts.entry(word.to_owned()).or_insert(0) += 1;
    }
    counts
}
#[test]
fn old_headings_and_words_are_preserved() {
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    assert!((150..=200).contains(&readme.lines().count()));
    let topics = topics();
    let all = format!(
        "{}\n{}",
        readme,
        topics
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let counts = |lines: Vec<&str>| {
        let mut map = BTreeMap::new();
        for line in lines {
            *map.entry(line.to_owned()).or_insert(0usize) += 1;
        }
        map
    };
    let actual = counts(headings(&all));
    let expected = counts(
        include_str!("../src/help/old-headings.txt")
            .lines()
            .collect(),
    );
    for (heading, count) in expected {
        assert_eq!(actual.get(&heading), Some(&count), "{heading}");
    }
    let actual = words(&all);
    for line in include_str!("../src/help/old-words.txt").lines() {
        let (count, word) = line.split_once(' ').unwrap();
        let count: usize = count.parse().unwrap();
        assert!(
            actual.get(word).copied().unwrap_or(0) >= count,
            "lost word {word}: expected {count}, got {:?}",
            actual.get(word)
        );
    }
}
#[test]
fn topic_help_and_command_help() {
    let temp = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_paddock"))
            .args(args)
            .env("HOME", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .output()
            .unwrap()
    };
    let list = run(&["help"]);
    assert!(list.status.success());
    let list = String::from_utf8(list.stdout).unwrap();
    for group in [
        "Start:",
        "Everyday use:",
        "Safety and privacy:",
        "Agents working together:",
        "Platforms and reference:",
    ] {
        assert!(list.contains(group), "{group}");
    }
    for (name, content) in topics() {
        assert!(list.contains(&name));
        let output = run(&["help", &name]);
        assert!(output.status.success(), "{name}");
        let rendered = String::from_utf8(output.stdout).unwrap();
        assert!(
            !rendered.contains("<img ")
                && !rendered.contains("<p ")
                && !rendered.contains("<a ")
                && !rendered.contains("</a>"),
            "{name}"
        );
        assert!(!rendered.is_empty(), "{name}");
        assert!(!content.is_empty());
    }
    let demo = String::from_utf8(run(&["help", "demo"]).stdout).unwrap();
    assert!(demo.contains("A terminal recording:"));
    assert!(demo.contains("docs/assets/demo.gif"));
    let network = String::from_utf8(run(&["help", "network"]).stdout).unwrap();
    assert!(!network.lines().any(|line| line.starts_with("| ")));
    let unknown = run(&["help", "unknown-topic-example"]);
    assert!(!unknown.status.success());
    let errors = String::from_utf8(unknown.stderr).unwrap();
    for (name, _) in topics() {
        assert!(errors.contains(&name), "{name}");
    }
    let command = run(&["help", "run"]);
    assert!(command.status.success());
    assert!(String::from_utf8(command.stdout)
        .unwrap()
        .contains("Usage:"));
}
#[test]
fn readme_anchor_links_resolve() {
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    let anchors: BTreeSet<_> = headings(&readme)
        .iter()
        .map(|h| {
            h.split_once(' ')
                .unwrap()
                .1
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ' ' || *c == '_')
                .map(|c| if c == ' ' { '-' } else { c })
                .collect::<String>()
        })
        .collect();
    let output = Command::new("git")
        .current_dir(root())
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    for name in output.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let path = Path::new(std::str::from_utf8(name).unwrap());
        let Ok(text) = std::fs::read_to_string(root().join(path)) else {
            continue;
        };
        let prefixes: &[&str] = if path == Path::new("README.md") {
            &["README.md#", "](#"]
        } else {
            &["README.md#"]
        };
        for part in prefixes
            .iter()
            .flat_map(|prefix| text.split(prefix).skip(1))
        {
            let anchor: String = part
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !anchor.is_empty() {
                assert!(
                    anchors.contains(&anchor),
                    "{}: missing README anchor {anchor}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn closed_stdout_is_quiet_for_help_paths() {
    use std::process::Stdio;
    for args in [
        &["help", "macos"][..],
        &["help"][..],
        &["help", "run"][..],
        &["--help"][..],
        &["run", "--help"][..],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_paddock"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{args:?}: {:?}", output.status);
        assert!(
            output.stderr.is_empty(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
