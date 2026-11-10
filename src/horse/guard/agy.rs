//! The prefix guard for agy conversations.
//!
//! agy relies on the implicit prefix cache of the model API. Whether that
//! cache is scoped by the conversation id (the `session_id` of a Code
//! Assist request) is unknown. So paddock treats a fork, which runs under
//! a new conversation id, as a cold start, also when its steps are the
//! same bytes. An append at the end keeps the prefix. A change of the
//! model, the effort, the tools, the system prompt or the workspace
//! changes the request before the history, so it breaks the cache. The
//! model sees the model-facing steps (agy/request.rs), about four bytes
//! per token.

use super::{Op, Verdict};
pub use crate::horse::agy::HARNESS;
use crate::horse::agy::{replay, request};
use crate::horse::store::{Node, Store};
use crate::horse::Session;
use anyhow::{bail, Context as _, Result};

const SCOPE: &str = "the agy cache scope is unknown (the model API may key its prompt cache by the conversation id), so paddock treats a new conversation id as cold";

/// The verdict of an operation on an agy session, or `None` for another
/// harness.
pub fn check(store: &Store, s: &Session, op: &Op, now_ms: u64) -> Result<Option<Verdict>> {
    if s.binding.harness != HARNESS {
        return Ok(None);
    }
    let nodes = store.session_nodes(s)?;
    let total = seen(&nodes);
    let mut v = Verdict {
        session: s.id.clone(),
        op: String::new(),
        keeps: true,
        prefix_bytes: total,
        lost_bytes: 0,
        lost_tokens: 0,
        reasons: Vec::new(),
        suggestion: None,
        notes: Vec::new(),
        at_ms: now_ms,
    };
    let breaks = |v: &mut Verdict, why: &str| {
        v.keeps = false;
        v.lost_bytes = total;
        v.lost_tokens = total.div_ceil(4);
        v.reasons.push(why.into());
    };
    match op {
        Op::Fork { at } => {
            let k = nodes
                .iter()
                .position(|n| &n.hash == at)
                .with_context(|| format!("node {} is not in the session", at.short()))?;
            let lost = seen(&nodes[..=k]);
            v.op = format!("a fork at {}", at.short());
            v.keeps = lost == 0;
            v.lost_bytes = lost;
            v.lost_tokens = lost.div_ceil(4);
            v.reasons.push(format!(
                "an agy fork runs under a new conversation id; {SCOPE}, and the fork starts cold"
            ));
        }
        Op::Swap { .. } => bail!("agy sessions have no swap under the same conversation id"),
        Op::Append { .. } => v.op = "an append".into(),
        Op::DnaChange => v.op = "a DNA entry".into(),
        Op::MoveHorse { to } => v.op = format!("a move to horse {to}"),
        Op::Model(m) => {
            v.op = format!("a model change to {m}");
            breaks(&mut v, "the prompt cache is per model");
        }
        Op::Effort(e) => {
            v.op = format!("an effort change to {e}");
            breaks(
                &mut v,
                "an effort change may change the generation configuration of the request",
            );
        }
        Op::Tools { .. } => {
            v.op = "a tool change".into();
            breaks(&mut v, "the tool declarations come before the history");
        }
        Op::System => {
            v.op = "a system prompt change".into();
            breaks(&mut v, "the system instruction comes before the history");
        }
        Op::Cwd(c) => {
            v.op = format!("a working directory change to {c}");
            breaks(&mut v, "the workspace is part of the system instruction");
        }
    }
    Ok(Some(v))
}

/// The bytes of the model-facing steps of a chain.
fn seen(nodes: &[Node]) -> u64 {
    replay(nodes)
        .map(|rows| request::body(&rows, i64::MAX).len() as u64)
        .unwrap_or(0)
}

/// The state tuple of an agy session: its head, version and conversation
/// id. agy records no request shape that paddock reads.
pub fn tuple(s: &Session) -> super::Tuple {
    super::Tuple {
        head: s.head.clone(),
        harness_version: s.binding.version.clone(),
        native_id: s.binding.native_id.clone(),
        ..Default::default()
    }
}
