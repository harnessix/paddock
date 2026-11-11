//! Synthetic Grok Build sessions for tests, in the shape that Grok 1.0.70
//! writes. No real session file is read.

use super::{session_dir, CHAT_FILE, SUMMARY_FILE, UPDATES_FILE};
use serde_json::json;
use std::path::{Path, PathBuf};

pub(crate) const ID: &str = "019a4b20-1111-7777-8888-123456789abc";
pub(crate) const CWD: &str = "/home/demo/project";

/// A session that a test builds turn by turn.
pub(crate) struct Fixture {
    pub id: String,
    pub cwd: String,
    pub updates: Vec<String>,
    pub chat: Vec<String>,
    event: u64,
    time: u64,
}

impl Fixture {
    pub fn new(id: &str, cwd: &str) -> Fixture {
        let mut f = Fixture {
            id: id.into(),
            cwd: cwd.into(),
            updates: Vec::new(),
            chat: Vec::new(),
            event: 0,
            time: 1_793_750_400,
        };
        f.chat.push(
            json!({"type": "system", "content": "You are Grok, a coding agent. \u{e9}"})
                .to_string(),
        );
        f
    }

    pub(crate) fn update(&mut self, xai: bool, update: serde_json::Value) {
        self.event += 1;
        self.time += 1;
        let method = if xai {
            "_x.ai/session/update"
        } else {
            "session/update"
        };
        self.updates.push(
            json!({
                "timestamp": self.time,
                "method": method,
                "params": {
                    "sessionId": self.id,
                    "update": update,
                    "_meta": {
                        "eventId": format!("{}-{}", self.id, self.event),
                        "agentTimestampMs": self.time * 1000,
                    },
                },
            })
            .to_string(),
        );
    }

    /// A user prompt of turn `p`.
    pub fn prompt(&mut self, p: u64, text: &str) -> &mut Self {
        self.update(
            false,
            json!({"sessionUpdate": "user_message_chunk",
                   "content": {"type": "text", "text": text},
                   "_meta": {"promptIndex": p}}),
        );
        self.chat.push(
            json!({"type": "user", "content": [{"type": "text", "text": text}], "prompt_index": p})
                .to_string(),
        );
        self
    }

    /// Reasoning: a thought chunk, and a reasoning item with a summary and
    /// encrypted content.
    pub fn reasoning(&mut self, text: &str) -> &mut Self {
        self.update(
            false,
            json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": text}}),
        );
        let n = self.chat.len();
        self.chat.push(
            json!({"type": "reasoning", "id": format!("rs_{n}"),
                   "summary": [{"type": "summary_text", "text": text}],
                   "encrypted_content": "gAAAA\u{2028}x/y+z=="})
            .to_string(),
        );
        self
    }

    /// A tool call and its result.
    pub fn tool(&mut self, call: &str, command: &str, output: &str) -> &mut Self {
        self.update(
            false,
            json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": command,
                   "kind": "execute", "status": "pending"}),
        );
        self.update(
            false,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": call, "status": "completed",
                   "content": [{"type": "content", "content": {"type": "text", "text": output}}]}),
        );
        self.chat.push(
            json!({"type": "assistant", "content": "",
                   "tool_calls": [{"id": call, "type": "function",
                                   "function": {"name": "run_terminal_cmd",
                                                "arguments": json!({"command": command}).to_string()}}],
                   "model_id": "grok-build"})
            .to_string(),
        );
        self.chat.push(
            json!({"type": "tool_result", "tool_call_id": call, "content": output}).to_string(),
        );
        self
    }

    /// An interjection: a message that the user typed while the turn ran.
    pub fn interject(&mut self, text: &str) -> &mut Self {
        self.update(
            true,
            json!({"sessionUpdate": "interjection", "content": {"type": "text", "text": text}}),
        );
        self.chat.push(
            json!({"type": "user", "content": [{"type": "text", "text": text}],
                   "synthetic_reason": "interjection"})
            .to_string(),
        );
        self
    }

    /// The reply that ends a turn.
    pub fn reply(&mut self, text: &str) -> &mut Self {
        self.update(
            false,
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}),
        );
        self.update(
            true,
            json!({"sessionUpdate": "turn_completed", "stopReason": "end_turn",
                   "usage": {"totalTokens": 1200}}),
        );
        self.chat.push(
            json!({"type": "assistant", "content": text, "model_id": "grok-build"}).to_string(),
        );
        self
    }

    /// A rewind to prompt `target`: a marker in the log, and the chat
    /// history cut before that prompt (a rewrite).
    pub fn rewind(&mut self, target: u64) -> &mut Self {
        self.update(
            true,
            json!({"sessionUpdate": "rewind_marker", "target_prompt_index": target}),
        );
        let rows: Vec<Vec<u8>> = self.chat.iter().map(|c| c.clone().into_bytes()).collect();
        let keep = super::request::cut(&rows, target.saturating_sub(1)).unwrap();
        let keep = if target == 0 { 1 } else { keep };
        self.chat.truncate(keep);
        self
    }

    /// A compaction: the chat history becomes the system prompt and a summary.
    pub fn compact(&mut self, summary: &str) -> &mut Self {
        let system = self.chat[0].clone();
        self.chat = vec![
            system,
            json!({"type": "user", "content": [{"type": "text", "text": summary}],
                   "synthetic_reason": "compaction_meta"})
            .to_string(),
        ];
        self
    }

    pub fn summary(&self) -> String {
        let v = json!({
            "info": {"id": self.id, "cwd": self.cwd},
            "session_summary": "",
            "created_at": "2026-11-04T00:00:00.000000000Z",
            "updated_at": "2026-11-04T00:10:00.000000000Z",
            "num_messages": self.updates.len(),
            "num_chat_messages": self.chat.len(),
            "current_model_id": "grok-build",
            "next_trace_turn": 0,
            "chat_format_version": 1,
            "generated_title": "Fix the tests",
            "last_turn_summary": "fixed",
        });
        serde_json::to_string_pretty(&v).unwrap()
    }

    /// Write the session folder under the Grok home `home`.
    pub fn write(&self, home: &Path) -> PathBuf {
        let dir = session_dir(home, &self.cwd, &self.id);
        std::fs::create_dir_all(&dir).unwrap();
        let lines = |v: &[String]| v.iter().map(|l| format!("{l}\n")).collect::<String>();
        std::fs::write(dir.join(UPDATES_FILE), lines(&self.updates)).unwrap();
        std::fs::write(dir.join(CHAT_FILE), lines(&self.chat)).unwrap();
        std::fs::write(dir.join(SUMMARY_FILE), self.summary()).unwrap();
        dir
    }

    /// The session folder of `id` under the Grok home `home`, read back to
    /// go on with it.
    pub fn read(home: &Path, cwd: &str, id: &str) -> Fixture {
        let dir = session_dir(home, cwd, id);
        let lines = |f: &str| -> Vec<String> {
            std::fs::read_to_string(dir.join(f))
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect()
        };
        let mut f = Fixture::new(id, cwd);
        f.updates = lines(UPDATES_FILE);
        f.chat = lines(CHAT_FILE);
        f.event = f.updates.len() as u64;
        f.time += f.event;
        f
    }

    /// The end of a turn that `session/cancel` stopped.
    pub fn cancelled(&mut self) -> &mut Self {
        self.update(
            true,
            json!({"sessionUpdate": "turn_completed", "stopReason": "cancelled"}),
        );
        self
    }

    /// A session with two turns: reasoning, a tool call, an interjection.
    pub fn two_turns() -> Fixture {
        let mut f = Fixture::new(ID, CWD);
        f.prompt(0, "Run the tests")
            .reasoning("The tests are in tests/.")
            .tool("call_1", "cargo test", "ok: 3 passed")
            .interject("Also run clippy")
            .reply("The tests pass.");
        f.prompt(1, "Now fix the warning")
            .reasoning("One warning in main.rs.")
            .tool("call_2", "cargo clippy", "warning: unused")
            .reply("Fixed.");
        f
    }
}
