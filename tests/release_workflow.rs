//! The release workflow runs only pinned action code, checks the tag
//! before it signs anything, and says which tags a manual run can build.

use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// The workflow files and the local actions, as (path, text).
fn workflow_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github");
    let mut out = Vec::new();
    for dir in ["workflows", "actions"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "yml" || x == "yaml") {
                    let rel = p.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap();
                    out.push((
                        rel.display().to_string(),
                        std::fs::read_to_string(&p).unwrap(),
                    ));
                }
            }
        }
    }
    assert!(out.len() >= 4, "{out:?}");
    out
}

/// The value of every `uses:` key, without a trailing comment.
fn uses(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start().trim_start_matches("- ");
            let v = l.strip_prefix("uses:")?;
            Some(v.split(" #").next().unwrap().trim().to_string())
        })
        .collect()
}

fn pinned(action: &str) -> bool {
    action
        .rsplit_once('@')
        .is_some_and(|(_, r)| r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[test]
fn outside_actions_are_pinned_by_commit() {
    let mut bad = Vec::new();
    for (path, text) in workflow_files() {
        for a in uses(&text) {
            if a.starts_with("./") {
                continue;
            }
            // Every outside action needs a commit, GitHub's own ones too:
            // a tag such as `@v4` can move, and release.yml runs ci.yml.
            if !pinned(&a) {
                bad.push(format!("{path}: {a}"));
            }
        }
    }
    assert!(bad.is_empty(), "not pinned by a full commit hash: {bad:#?}");
}

#[test]
fn every_workflow_pins_an_action_to_the_same_commit() {
    let mut seen = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (path, text) in workflow_files() {
        for a in uses(&text) {
            if let Some((name, rev)) = a.rsplit_once('@').filter(|_| !a.starts_with("./")) {
                let revs = seen.entry(name.to_string()).or_default();
                if !revs.iter().any(|r| r.starts_with(&format!("{rev} "))) {
                    revs.push(format!("{rev} ({path})"));
                }
            }
        }
    }
    seen.retain(|_, revs| revs.len() > 1);
    assert!(seen.is_empty(), "one action, several commits: {seen:#?}");
}

#[test]
fn the_tag_is_checked_before_the_attestation() {
    let text = read(".github/workflows/release.yml");
    let check = text
        .find("name: Check that the tag did not move")
        .expect("tag check");
    let attest = text
        .find("uses: actions/attest-build-provenance@")
        .expect("attestation");
    let publish = text.find("name: Publish").expect("publish");
    assert!(check < attest, "the attestation comes before the tag check");
    assert!(attest < publish);
}

#[test]
fn the_header_says_which_tags_a_manual_run_can_build() {
    let text = read(".github/workflows/release.yml");
    let header: String = text
        .lines()
        .take_while(|l| !l.starts_with("on:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        header.contains("Run it by hand") && header.contains(".github/actions/rust"),
        "{header}"
    );
}
