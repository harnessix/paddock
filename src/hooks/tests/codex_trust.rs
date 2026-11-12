//! HOOK1: paddock's own Codex hooks are trusted by their hash in the
//! sandbox, and no other hook is. Codex's judgment is
//! `compaction::codex::judge`, after codex-rs rust-v0.167.0.

use super::*;

/// The sandbox copies of the Codex hooks file and `config.toml`, judged as
/// Codex 0.167.0 judges them, with the hooks file at `source`.
fn codex_judged(
    spec: &crate::spec::SandboxSpec,
    source: &str,
) -> Vec<crate::compaction::codex::Judged> {
    let home = &spec.home_src;
    let hooks: Value =
        serde_json::from_slice(&std::fs::read(home.join(".codex/hooks.json")).unwrap()).unwrap();
    let cfg = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap_or_default();
    crate::compaction::codex::judge(source, &hooks["hooks"], &cfg).unwrap()
}

fn command_of(j: &crate::compaction::codex::Judged) -> &str {
    j.handler_value["command"].as_str().unwrap_or("")
}

/// Write the sandbox copy of the Codex hooks file again, as an agent in the
/// sandbox can.
fn edit_codex_hooks(spec: &crate::spec::SandboxSpec, f: impl FnOnce(&mut Value)) {
    let p = spec.home_src.join(".codex/hooks.json");
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    f(&mut doc);
    std::fs::write(&p, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}

/// HOOK1: a sandbox Codex home that paddock prepared trusts each of
/// paddock's hooks (mail Stop, mail PostToolUse, compaction SessionStart),
/// so Codex runs them and shows no hooks review for them.
#[test]
fn a_prepared_codex_home_trusts_each_paddock_hook() {
    let w = world("");
    let spec = start(&w, "codex").unwrap();
    let src = format!("{}/.codex/hooks.json", w.env.home.display());
    let judged = codex_judged(&spec, &src);
    let mut cmds: Vec<&str> = judged.iter().map(command_of).collect();
    cmds.sort();
    assert_eq!(
        cmds,
        [
            "paddock-host compacted --harness codex",
            crate::mail::deliver::STOP_COMMAND,
            crate::mail::deliver::TOOL_COMMAND,
        ]
    );
    assert!(
        judged
            .iter()
            .all(|j| j.trust == crate::compaction::codex::Trust::Trusted),
        "{judged:?}"
    );
    assert!(codex_paddock_trusted(&spec).unwrap());
    // The run delivers mail through the hooks.
    assert_eq!(
        crate::mail::deliver::of_spec(&spec),
        Some(crate::mail::deliver::HOOKS)
    );
}

/// srt and seatbelt give the harness the sandbox home at its host path. The
/// trust must name that path, the one that Codex sees.
#[test]
fn the_codex_trust_names_the_home_that_the_harness_sees() {
    let w = world("");
    let spec = start_on(&w, "codex", |s| s.harness_home = s.home_src.clone()).unwrap();
    let seen = format!("{}/.codex/hooks.json", spec.home_src.display());
    let judged = codex_judged(&spec, &seen);
    assert_eq!(judged.len(), 3, "{judged:?}");
    assert!(
        judged
            .iter()
            .all(|j| j.trust == crate::compaction::codex::Trust::Trusted),
        "{judged:?}"
    );
    assert!(codex_paddock_trusted(&spec).unwrap());
    // Under the host home path, Codex would find no trust.
    let host = format!("{}/.codex/hooks.json", w.env.home.display());
    assert!(codex_judged(&spec, &host)
        .iter()
        .all(|j| j.trust == crate::compaction::codex::Trust::Untrusted));
}

/// A hook entry that paddock did not write is not trusted: one that the
/// host copy had without a listed program, and one that is added in the
/// sandbox after the start.
#[test]
fn a_codex_hook_that_paddock_did_not_write_is_not_trusted() {
    let w = world("");
    put(
        &w,
        ".codex/hooks.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/usr/bin/env my-hook stop"}]}]}}"#,
        0o644,
    );
    let spec = start(&w, "codex").unwrap();
    let src = format!("{}/.codex/hooks.json", w.env.home.display());
    assert!(!codex_judged(&spec, &src)
        .iter()
        .any(|j| command_of(j).contains("my-hook")));
    edit_codex_hooks(&spec, |doc| {
        doc["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"hooks": [{"type": "command", "command": "planted stop"}]}));
        doc["hooks"]["PreToolUse"] =
            serde_json::json!([{"hooks": [{"type": "command", "command": "planted tool"}]}]);
    });
    let judged = codex_judged(&spec, &src);
    let planted: Vec<_> = judged
        .iter()
        .filter(|j| command_of(j).starts_with("planted"))
        .collect();
    assert_eq!(planted.len(), 2, "{judged:?}");
    assert!(planted
        .iter()
        .all(|j| j.trust == crate::compaction::codex::Trust::Untrusted));
    // paddock's own hooks keep their trust.
    assert!(codex_paddock_trusted(&spec).unwrap());
}

/// A changed paddock hook is not trusted: the hash binds the trust to the
/// content of the entry.
#[test]
fn a_changed_paddock_hook_is_not_trusted() {
    let w = world("");
    let spec = start(&w, "codex").unwrap();
    let src = format!("{}/.codex/hooks.json", w.env.home.display());
    let changes: [fn(&mut Value); 3] = [
        |h| h["command"] = serde_json::json!("paddock-host mail hook stop; evil"),
        |h| h["timeout"] = serde_json::json!(600),
        |h| h["async"] = serde_json::json!(true),
    ];
    for change in changes {
        let before = std::fs::read(spec.home_src.join(".codex/hooks.json")).unwrap();
        edit_codex_hooks(&spec, |doc| {
            change(&mut doc["hooks"]["Stop"][0]["hooks"][0])
        });
        let judged = codex_judged(&spec, &src);
        let stop = judged
            .iter()
            .find(|j| j.event == "Stop" && j.group == 0)
            .unwrap();
        assert_eq!(
            stop.trust,
            crate::compaction::codex::Trust::Modified,
            "{stop:?}"
        );
        assert!(!codex_paddock_trusted(&spec).unwrap());
        std::fs::write(spec.home_src.join(".codex/hooks.json"), before).unwrap();
        assert!(codex_paddock_trusted(&spec).unwrap());
    }
    // A trust in config.toml for other content does not count either.
    let cfg = spec.home_src.join(".codex/config.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    let key = format!("{src}:stop:0:0");
    let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
    doc["hooks"]["state"][key.as_str()]["trusted_hash"] = toml_edit::value("sha256:00");
    std::fs::write(&cfg, doc.to_string()).unwrap();
    assert!(!codex_paddock_trusted(&spec).unwrap());
}

/// Review of HOOK1, item b: the run promises delivery through the mail
/// hooks only while Codex trusts them. Before the home is written there is
/// no trust, and a changed hook or trust takes it away.
#[test]
fn the_run_delivers_through_the_hooks_only_while_codex_trusts_them() {
    use crate::mail::deliver::{of_spec, HOOKS};
    let w = world("");
    // A spec whose sandbox home paddock did not write yet.
    let h = crate::profile::Harness {
        profile: crate::profile::builtin("codex").unwrap(),
        exe: "/usr/bin/true".into(),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = crate::build::Options {
        harness: "codex".into(),
        project: Some(w.project.clone()),
        assume_yes: true,
        ..Default::default()
    };
    let unwritten = crate::build::build_with(&w.env, &opts, h).unwrap().spec;
    assert!(unwritten.home_settings.iter().any(|s| s
        .value
        .to_string()
        .contains(crate::mail::deliver::STOP_COMMAND)));
    assert_eq!(of_spec(&unwritten), None);
    let spec = start(&w, "codex").unwrap();
    assert_eq!(of_spec(&spec), Some(HOOKS));
    // A changed paddock hook.
    let before = std::fs::read(spec.home_src.join(".codex/hooks.json")).unwrap();
    edit_codex_hooks(&spec, |doc| {
        doc["hooks"]["Stop"][0]["hooks"][0]["timeout"] = serde_json::json!(600)
    });
    assert_eq!(of_spec(&spec), None);
    std::fs::write(spec.home_src.join(".codex/hooks.json"), before).unwrap();
    assert_eq!(of_spec(&spec), Some(HOOKS));
    // The trust under a key that Codex does not look up: the harness sees
    // another home than the one that the trust names.
    let mut other = spec.clone();
    other.harness_home = spec.home_src.clone();
    assert_eq!(of_spec(&other), None);
}
