//! Codex runs a hook from a user file only when `config.toml` has a trusted
//! hash for it. The user approves a hook with `/hooks`, and Codex writes
//!
//! ```toml
//! [hooks.state."/home/demo/.codex/hooks.json:session_start:0:0"]
//! trusted_hash = "sha256:..."
//! ```
//!
//! In a sandbox no user is there to approve paddock's own hook, so paddock
//! writes this entry into the sandbox copy of `config.toml`, for its own
//! hook entry only (`[compaction] codex_trust` of the profile). The rules
//! come from the Codex source (openai/codex, codex-rs, rust-v0.166.0):
//!
//! - `hooks/src/lib.rs`, `hook_key`: the key is `SOURCE:EVENT:GROUP:HANDLER`.
//!   SOURCE is the absolute path of the hooks file (`$CODEX_HOME/hooks.json`,
//!   and `$CODEX_HOME` is `$HOME/.codex` when it is not set), EVENT is
//!   `hook_event_key_label`, GROUP and HANDLER are the positions in the file.
//! - `hooks/src/engine/discovery.rs`, `hook_hash`: the hash covers a
//!   normalized identity, `{event_name, matcher, hooks: [handler]}`, built in
//!   `append_matcher_groups` (`normalize_command_hook` sets the timeout).
//! - `hooks/src/events/common.rs`, `matcher_pattern_for_event`: some events
//!   ignore the matcher, and then it is not in the hash.
//! - `config/src/fingerprint.rs`, `version_for_toml`: SHA-256 of the compact
//!   JSON with sorted keys, written as `sha256:HEX`.
//! - `config/src/hook_config.rs`, `HookStateToml`, and
//!   `hooks/src/config_rules.rs`, `hook_states_from_stack`: the state is read
//!   from `[hooks.state]` of the user's `config.toml`.

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// Codex's default for `additionalContextLimit`, in tokens
/// (`hooks/src/output_spill.rs`). This value is left out of the hash.
const DEFAULT_CONTEXT_LIMIT: u64 = 2_500;

/// `hook_event_key_label`: the event in the key and in the hash.
pub fn event_key(event: &str) -> Option<&'static str> {
    Some(match event {
        "PreModelRequest" => "pre_model_request",
        "PostModelRequest" => "post_model_request",
        "PreToolUse" => "pre_tool_use",
        "PermissionRequest" => "permission_request",
        "PostToolUse" => "post_tool_use",
        "PreCompact" => "pre_compact",
        "PostCompact" => "post_compact",
        "SessionStart" => "session_start",
        "SessionEnd" => "session_end",
        "UserPromptSubmit" => "user_prompt_submit",
        "SubagentStart" => "subagent_start",
        "SubagentStop" => "subagent_stop",
        "Stop" => "stop",
        "Interrupt" => "interrupt",
        _ => return None,
    })
}

/// `matcher_pattern_for_event`: these events ignore the matcher.
fn ignores_matcher(event: &str) -> bool {
    matches!(
        event,
        "PreModelRequest" | "PostModelRequest" | "UserPromptSubmit" | "Stop" | "Interrupt"
    )
}

/// The command handler as Codex normalizes it before the hash.
fn normalized_handler(event: &str, h: &Value) -> Result<Value> {
    if h.get("type").and_then(Value::as_str) != Some("command") {
        bail!("only command hooks can be trusted");
    }
    let command = h
        .get("command")
        .and_then(Value::as_str)
        .filter(|c| !c.trim().is_empty())
        .context("the hook has no command")?;
    let timeout = h.get("timeout").and_then(Value::as_u64);
    let timeout = match event {
        "SessionEnd" | "Interrupt" => timeout.unwrap_or(1).clamp(1, 3),
        _ => timeout.unwrap_or(600).max(1),
    };
    let mut out = Map::new();
    out.insert("type".into(), json!("command"));
    out.insert("command".into(), json!(command));
    out.insert("timeout".into(), json!(timeout));
    out.insert(
        "async".into(),
        json!(h.get("async").and_then(Value::as_bool).unwrap_or(false)),
    );
    if let Some(m) = h.get("statusMessage").and_then(Value::as_str) {
        out.insert("statusMessage".into(), json!(m));
    }
    let limit_counts = matches!(
        event,
        "PreToolUse" | "PostToolUse" | "SessionStart" | "UserPromptSubmit" | "SubagentStart"
    );
    if let Some(l) = h
        .get("additionalContextLimit")
        .and_then(Value::as_u64)
        .filter(|l| limit_counts && *l != DEFAULT_CONTEXT_LIMIT)
    {
        out.insert("additionalContextLimit".into(), json!(l));
    }
    Ok(Value::Object(out))
}

/// The same value with the keys of every object in sorted order.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sorted(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// `hook_hash` for one handler of a matcher group.
pub fn hook_hash(event: &str, matcher: Option<&str>, handler: &Value) -> Result<String> {
    let label = event_key(event).with_context(|| format!("Codex has no hook event {event}"))?;
    let mut identity = Map::new();
    identity.insert("event_name".into(), json!(label));
    if let Some(m) = matcher.filter(|_| !ignores_matcher(event)) {
        identity.insert("matcher".into(), json!(m));
    }
    identity.insert("hooks".into(), json!([normalized_handler(event, handler)?]));
    let bytes = serde_json::to_vec(&sorted(&Value::Object(identity)))?;
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("sha256:{hex}"))
}

/// The `[hooks.state]` key of handler `handler` of group `group` of an
/// event in the hooks file `source`.
pub fn hook_key(source: &str, event: &str, group: usize, handler: usize) -> Result<String> {
    let label = event_key(event).with_context(|| format!("Codex has no hook event {event}"))?;
    Ok(format!("{source}:{label}:{group}:{handler}"))
}

/// The `[hooks.state]` key and the trusted hash of each handler of a group
/// at `group_index` in the array of `event`, in the hooks file `source` (an
/// absolute path inside the sandbox).
pub fn trust_entries(
    source: &str,
    event: &str,
    group_index: usize,
    group: &Value,
) -> Result<Vec<(String, String)>> {
    let label = event_key(event).with_context(|| format!("Codex has no hook event {event}"))?;
    let matcher = group.get("matcher").and_then(Value::as_str);
    let handlers = group
        .get("hooks")
        .and_then(Value::as_array)
        .context("the hook group has no hooks")?;
    handlers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            Ok((
                format!("{source}:{label}:{group_index}:{i}"),
                hook_hash(event, matcher, h)?,
            ))
        })
        .collect()
}

/// After the settings of a start are written: find paddock's hook group in
/// the hooks file, and write the trust of each of its handlers into
/// Codex's `config.toml` in the sandbox home. Only this group is trusted.
pub fn trust(home: &crate::safefs::Root, s: &crate::spec::HomeSetting) -> Result<()> {
    let Some(t) = &s.codex_trust else {
        return Ok(());
    };
    let event = s.path.last().context("the hook has no event")?;
    let text = home
        .read(&s.file, crate::safefs::MAX_READ)?
        .context("the hooks file is missing")?;
    let doc: Value = serde_json::from_slice(&text).context("the hooks file is not JSON")?;
    let mut groups = &doc;
    for k in &s.path {
        groups = &groups[k];
    }
    let index = groups
        .as_array()
        .and_then(|a| a.iter().position(|g| g == &s.value))
        .context("paddock's hook is not in the hooks file")?;
    let entries = trust_entries(&t.source, event, index, &s.value)?;
    let config = home
        .read(&t.config, crate::safefs::MAX_READ)?
        .map(|b| String::from_utf8(b).unwrap_or_default())
        .unwrap_or_default();
    let values: Vec<crate::spec::HomeSetting> = entries
        .into_iter()
        .map(|(key, hash)| crate::spec::HomeSetting {
            file: t.config.clone(),
            format: "toml".into(),
            path: vec!["hooks".into(), "state".into(), key, "trusted_hash".into()],
            value: json!(hash),
            origin: s.origin.clone(),
            append: false,
            codex_trust: None,
            no_pty: false,
            chain: false,
        })
        .collect();
    let refs: Vec<&crate::spec::HomeSetting> = values.iter().collect();
    let out = crate::privacy::set_toml(&config, &refs)
        .with_context(|| format!("~/{}", t.config.display()))?;
    home.write(&t.config, out.as_bytes(), 0o600)
}

/// What Codex makes of one handler of a hooks file (`HookTrustStatus` of
/// codex-rs `protocol`), for a handler from a user file: neither managed
/// nor from a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// `[hooks.state]` has the hash of the handler as it is now. It runs.
    Trusted,
    /// `[hooks.state]` has another hash: the handler changed. It does not
    /// run, and the hooks review counts it.
    Modified,
    /// No trusted hash. It does not run, and the hooks review counts it.
    Untrusted,
}

/// One handler of a hooks file and what Codex makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub event: String,
    pub group: usize,
    pub handler: usize,
    pub handler_value: Value,
    pub trust: Trust,
}

/// Every handler of the `hooks` object of a Codex hooks file `doc` at
/// `source` (the path that Codex sees), judged against the `[hooks.state]`
/// of `config` (the text of `config.toml`), as Codex 0.167.0 does it
/// (codex-rs hooks/src/engine/discovery.rs, `append_matcher_groups` and
/// `hook_trust_status`). A handler that paddock cannot hash counts as
/// untrusted.
pub fn judge(source: &str, hooks: &Value, config: &str) -> Result<Vec<Judged>> {
    let cfg: toml::Value = if config.trim().is_empty() {
        toml::Value::Table(Default::default())
    } else {
        toml::from_str(config).context("config.toml is not valid TOML")?
    };
    let state = cfg.get("hooks").and_then(|h| h.get("state"));
    let mut out = Vec::new();
    let Some(events) = hooks.as_object() else {
        return Ok(out);
    };
    for (event, groups) in events {
        if event_key(event).is_none() {
            continue;
        }
        for (g, group) in groups.as_array().into_iter().flatten().enumerate() {
            let matcher = group.get("matcher").and_then(Value::as_str);
            for (h, handler) in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let key = hook_key(source, event, g, h)?;
                let trusted = state
                    .and_then(|s| s.get(&key))
                    .and_then(|s| s.get("trusted_hash"))
                    .and_then(toml::Value::as_str);
                let trust = match (trusted, hook_hash(event, matcher, handler)) {
                    (None, _) => Trust::Untrusted,
                    (Some(t), Ok(now)) if t == now => Trust::Trusted,
                    (Some(_), Ok(_)) => Trust::Modified,
                    (Some(_), Err(_)) => Trust::Untrusted,
                };
                out.push(Judged {
                    event: event.clone(),
                    group: g,
                    handler: h,
                    handler_value: handler.clone(),
                    trust,
                });
            }
        }
    }
    Ok(out)
}
