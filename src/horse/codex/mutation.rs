//! Conservative validation before changing or delivering into a rollout.
use anyhow::{bail, Context as _, Result};
use serde::Deserialize;
use serde_json::Value;

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    InputText {
        text: String,
    },
    OutputText {
        text: String,
    },
    InputImage {
        #[serde(default)]
        image_url: Option<String>,
        #[serde(default)]
        file_id: Option<String>,
        #[serde(default)]
        detail: Option<ImageDetail>,
    },
    InputAudio {
        #[serde(default)]
        audio_url: Option<String>,
        #[serde(default)]
        file_id: Option<String>,
    },
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ImageDetail {
    Auto,
    Low,
    High,
    Original,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Commentary,
    PartialAnswer,
    FinalAnswer,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Summary {
    SummaryText { text: String },
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReasoningContent {
    ReasoningText { text: String },
    Text { text: String },
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Message {
        role: String,
        content: Vec<Content>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        phase: Option<Phase>,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    FunctionCall {
        name: String,
        arguments: String,
        call_id: String,
    },
    FunctionCallOutput {
        output: String,
    },
    CustomToolCall {
        name: String,
        input: String,
        call_id: String,
    },
    CustomToolCallOutput {
        call_id: String,
        output: String,
    },
    Reasoning {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        summary: Vec<Summary>,
        #[serde(default)]
        content: Option<Vec<ReasoningContent>>,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
}

fn response(value: &Value) -> Result<()> {
    if value
        .get("id")
        .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        bail!("invalid response id");
    }
    if value
        .get("internal_chat_message_metadata_passthrough")
        .is_some_and(|v| !v.is_null())
    {
        bail!("unverified response metadata");
    }
    match serde_json::from_value::<Response>(value.clone()).context("unsupported response item")? {
        Response::Message {
            role, content, id, ..
        } => {
            if !matches!(role.as_str(), "user" | "assistant" | "system" | "developer") {
                bail!("unknown message role {role}");
            }
            let _ = id;
            for item in content {
                match item {
                    Content::InputText { text } | Content::OutputText { text } => {
                        let _ = text;
                    }
                    Content::InputImage {
                        image_url, file_id, ..
                    } => {
                        if image_url.is_some() == file_id.is_some() {
                            bail!("invalid image reference");
                        }
                    }
                    Content::InputAudio { audio_url, file_id } => {
                        if audio_url.is_some() == file_id.is_some() {
                            bail!("invalid audio reference");
                        }
                    }
                }
            }
        }
        Response::Reasoning { id, .. } => {
            let _ = id;
        }
        Response::FunctionCall { .. }
        | Response::FunctionCallOutput { .. }
        | Response::CustomToolCall { .. }
        | Response::CustomToolCallOutput { .. } => {}
    }
    Ok(())
}

fn check_multi_agent(value: Option<&Value>) -> Result<()> {
    if let Some(value) = value.filter(|v| !v.is_null()) {
        if !matches!(value.as_str(), Some("disabled" | "v1" | "v2")) {
            bail!("unknown multi-agent version");
        }
    }
    Ok(())
}

pub(super) fn check(raw: &[u8], index: usize) -> Result<()> {
    inner(raw).map_err(|e| {
        anyhow::anyhow!(
            "Codex rollout line {} has {e:#}; cannot mutate history",
            index + 1
        )
    })
}

fn inner(raw: &[u8]) -> Result<()> {
    let row = super::parse(raw)?;
    let kind = row["type"].as_str().context("missing row type")?;
    let payload = row
        .get("payload")
        .and_then(Value::as_object)
        .context("payload must be an object")?;
    if row.get("timestamp").and_then(Value::as_str).is_none() {
        bail!("missing timestamp");
    }
    if row
        .get("ordinal")
        .is_some_and(|v| !v.is_null() && v.as_u64().is_none())
    {
        bail!("invalid ordinal");
    }
    match kind {
        "session_meta" => {
            let id = payload
                .get("id")
                .and_then(Value::as_str)
                .context("missing thread id")?;
            crate::horse::claude::check_session_id(id)?;
            if payload
                .get("history_mode")
                .is_some_and(|v| !v.is_null() && v != "legacy")
            {
                bail!("unknown history mode");
            }
            if payload.get("dynamic_tools").is_some_and(|v| !v.is_null()) {
                bail!("unverified session metadata dynamic_tools");
            }
            if let Some(window) = payload.get("context_window").filter(|v| !v.is_null()) {
                window
                    .get("window_id")
                    .and_then(Value::as_str)
                    .context("invalid context window")?;
            }
            check_multi_agent(payload.get("multi_agent_version"))?;
            if payload
                .get("selected_capability_roots")
                .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
            {
                bail!("unverified selected capability roots");
            }
            if let Some(source) = payload.get("source") {
                if !matches!(
                    source.as_str(),
                    Some("cli" | "vscode" | "exec" | "mcp" | "unknown")
                ) {
                    bail!("unverified session source");
                }
            }
            if let Some(v) = payload.get("session_id") {
                crate::horse::claude::check_session_id(v.as_str().context("invalid session id")?)?;
            }
            if let Some(v) = payload.get("forked_from_id").filter(|v| !v.is_null()) {
                crate::horse::claude::check_session_id(v.as_str().context("invalid fork parent")?)?;
            }
        }
        "response_item" => response(&row["payload"])?,
        "event_msg" => {
            let event = payload
                .get("type")
                .and_then(Value::as_str)
                .context("missing event type")?;
            if event == "thread_settings_applied" {
                bail!("unverified event type {event}");
            }
            if matches!(event, "task_complete" | "turn_complete" | "turn_aborted")
                && payload.get("error").is_some_and(|v| !v.is_null())
            {
                bail!("unverified turn error");
            }
            if event == "turn_aborted" {
                if let Some(reason) = payload.get("reason").filter(|v| !v.is_null()) {
                    if !matches!(
                        reason.as_str(),
                        Some("interrupted" | "replaced" | "review_ended" | "budget_limited")
                    ) {
                        bail!("unknown turn abort reason");
                    }
                }
            }
            if matches!(event, "task_started" | "turn_started") {
                if let Some(mode) = payload
                    .get("collaboration_mode_kind")
                    .filter(|v| !v.is_null())
                {
                    if !matches!(
                        mode.as_str(),
                        Some(
                            "plan" | "default" | "code" | "pair_programming" | "execute" | "custom"
                        )
                    ) {
                        bail!("unknown collaboration mode");
                    }
                }
                if let Some(attribution) = payload.get("turn_attribution").filter(|v| !v.is_null())
                {
                    let attribution = attribution
                        .as_object()
                        .context("invalid turn attribution")?;
                    attribution
                        .get("turn_id")
                        .and_then(Value::as_str)
                        .context("invalid attributed turn id")?;
                    for field in [
                        "turn_trigger",
                        "parent_turn_id",
                        "initiating_agent_path",
                        "root_turn_id",
                    ] {
                        if attribution
                            .get(field)
                            .is_some_and(|v| !v.is_null() && !v.is_string())
                        {
                            bail!("invalid turn attribution {field}");
                        }
                    }
                }
            }
            if event == "token_count" {
                if let Some(info) = payload.get("info").filter(|v| !v.is_null()) {
                    let info = info.as_object().context("invalid token usage info")?;
                    if let Some(last) = info.get("last_token_usage").filter(|v| !v.is_null()) {
                        let last = last.as_object().context("invalid last token usage")?;
                        if let Some(input) = last.get("input_tokens") {
                            input.as_i64().context("invalid input token count")?;
                        }
                    }
                }
            }
            if event == "token_count" && payload.get("rate_limits").is_some_and(|v| !v.is_null()) {
                bail!("unverified rate limits");
            }
            if event == "agent_message" {
                if let Some(phase) = payload.get("phase").filter(|v| !v.is_null()) {
                    serde_json::from_value::<Phase>(phase.clone())
                        .context("unknown agent message phase")?;
                }
                for field in ["memory_citation", "delivery", "questions"] {
                    if payload.get(field).is_some_and(|v| !v.is_null()) {
                        bail!("unverified agent message {field}");
                    }
                }
            }
            if event == "user_message" {
                for field in [
                    "audio_references",
                    "audio_order",
                    "image_order",
                    "image_details",
                    "file_id_details",
                    "local_image_details",
                    "text_elements",
                ] {
                    if payload
                        .get(field)
                        .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                    {
                        bail!("unverified user message {field}");
                    }
                }
            }
            if !matches!(
                event,
                "user_message"
                    | "task_started"
                    | "task_complete"
                    | "turn_started"
                    | "turn_complete"
                    | "turn_aborted"
                    | "context_compacted"
                    | "token_count"
                    | "agent_message"
                    | "agent_reasoning"
                    | "thread_settings_applied"
            ) {
                bail!("unknown or unverified event type {event}");
            }
        }
        "compacted" => {
            for field in [
                "guardian_history",
                "retained_context",
                "mcp_resource_origins",
                "latest_token_usage_record",
                "resume_metadata",
                "replacement_history_metadata",
            ] {
                if payload.get(field).is_some_and(|v| !v.is_null()) {
                    bail!("unverified compaction field {field}");
                }
            }
            payload
                .get("message")
                .and_then(Value::as_str)
                .context("missing compaction message")?;
            if let Some(replacement) = payload.get("replacement_history").filter(|v| !v.is_null()) {
                for item in replacement
                    .as_array()
                    .context("invalid replacement history")?
                {
                    response(item)?;
                }
            }
        }
        "turn_context" => {
            for field in [
                "collaboration_mode",
                "permission_profile",
                "active_permission_profile",
                "file_system_sandbox_policy",
                "cyber_access_program",
                "effort",
                "summary",
            ] {
                if payload.get(field).is_some_and(|v| !v.is_null()) {
                    bail!("unverified turn context field {field}");
                }
            }
            check_multi_agent(payload.get("multi_agent_version"))?;
            for field in ["cwd", "model", "approval_policy"] {
                payload
                    .get(field)
                    .and_then(Value::as_str)
                    .with_context(|| format!("missing {field}"))?;
            }
            let approval = payload
                .get("approval_policy")
                .and_then(Value::as_str)
                .unwrap();
            if !matches!(
                approval,
                "never" | "untrusted" | "on-request" | "on-failure"
            ) {
                bail!("unknown approval policy {approval}");
            }
            let sandbox = payload
                .get("sandbox_policy")
                .and_then(|v| v.get("type"))
                .and_then(Value::as_str)
                .context("missing sandbox policy")?;
            if !matches!(
                sandbox,
                "danger-full-access" | "read-only" | "workspace-write" | "external-sandbox"
            ) {
                bail!("unknown sandbox policy {sandbox}");
            }
            let policy = &payload["sandbox_policy"];
            if let Some(network) = policy.get("network_access") {
                let valid = match sandbox {
                    "external-sandbox" => {
                        matches!(network.as_str(), Some("restricted" | "enabled"))
                    }
                    "read-only" | "workspace-write" => network.is_boolean(),
                    _ => false,
                };
                if !valid {
                    bail!("unknown sandbox network policy");
                }
            }
        }
        "dynamic_tools_updated"
        | "inter_agent_communication"
        | "inter_agent_communication_metadata"
        | "token_usage_record"
        | "world_state"
        | "security_risk_score"
        | "retained_context"
        | "realtime_item" => bail!("unverified record type {kind}"),
        _ => bail!("unknown type {kind}"),
    }
    Ok(())
}

pub(crate) fn check_session(store: &crate::horse::Store, session: &str) -> Result<()> {
    let session = store.session(session)?;
    for (i, node) in store.session_nodes(&session)?.iter().enumerate() {
        check(&node.raw, i)?;
    }
    Ok(())
}

pub(crate) fn check_file(path: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    if !bytes.ends_with(b"\n") {
        bail!("Codex rollout has a torn tail; cannot deliver");
    }
    let rows: Vec<_> = bytes.split(|b| *b == b'\n').collect();
    if rows.len() <= 1 {
        bail!("Codex rollout is empty; cannot deliver");
    }
    for (i, row) in rows[..rows.len() - 1].iter().enumerate() {
        check(row, i)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_unknowns_and_bad_types_fail_with_line_number() {
        for raw in [
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"new_response"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"new_content"}]}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":42}]}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"future_summary"}]}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[],"phase":"future"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"user_message","audio_order":["future"]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"turn_aborted","reason":"future"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","error":{"type":"future"}}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"agent_message","phase":"future"}}"#,
            r#"{"timestamp":"t","type":"turn_context","payload":{"cwd":"/work","model":"m","approval_policy":"never","sandbox_policy":{"type":"external-sandbox","network_access":"future"}}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":"bad"}}}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","rate_limits":{"plan_type":"future"}}}"#,
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"01900000-0000-7000-8000-000000000001","cli_version":"0.166.0","context_window":{"type":"future"}}}"#,
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"01900000-0000-7000-8000-000000000001","cli_version":"0.166.0","multi_agent_version":"v99"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"new_event"}}"#,
            r#"{"timestamp":"t","type":"compacted","payload":{"message":"x","replacement_history":[{"type":"new_response"}]}}"#,
        ] {
            let error = check(raw.as_bytes(), 4).unwrap_err().to_string();
            assert!(error.contains("line 5"), "{error}");
        }
    }

    #[test]
    fn plain_tool_calls_and_outputs_are_accepted() {
        for raw in [
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{}","call_id":"c"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"done"}}"#,
        ] {
            check(raw.as_bytes(), 0).unwrap();
        }
        let unknown = br#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","output":[{"type":"future"}]}}"#;
        assert!(check(unknown, 0).is_err());
    }

    #[test]
    fn known_context_and_multi_agent_versions_are_accepted() {
        let raw = br#"{"timestamp":"t","type":"session_meta","payload":{"id":"01900000-0000-7000-8000-000000000001","cli_version":"0.166.0","context_window":{"window_id":"window"},"multi_agent_version":"v2"}}"#;
        check(raw, 0).unwrap();
    }

    #[test]
    fn delivery_file_refuses_unknown_records_and_torn_or_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let good = "{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        std::fs::write(&path, good).unwrap();
        check_file(&path).unwrap();
        for tail in ["{\"type\":\"future\"}\n", "{broken}\n", "{\"type\":"] {
            std::fs::write(&path, format!("{good}{tail}")).unwrap();
            assert!(check_file(&path).is_err(), "{tail}");
        }
    }
}
