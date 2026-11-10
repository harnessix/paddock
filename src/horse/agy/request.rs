//! The model-facing steps of an agy conversation, for the offline proof.
//!
//! agy builds a request from the stored steps: it converts the trajectory
//! step by step, from the step payload, the step metadata and the render
//! info. [`body`] puts these bytes of every model-facing step in index
//! order into one byte string. Two conversations with equal bodies give
//! the model the same history. The model-facing types are the ones that
//! session-migrate replayed through a native agy: user input, planner
//! response, checkpoint, conversation history and the tool steps.

use super::db::Rows;
use super::row::{Cell, Row};
use super::{steps, CHECKPOINT, PLANNER_RESPONSE, USER_INPUT};

/// Step types that agy turns into request content.
pub const MODEL_FACING: [i64; 12] = [
    8, // view file
    9, // list directory
    USER_INPUT,
    PLANNER_RESPONSE,
    21, // run command
    CHECKPOINT,
    38,  // MCP tool
    86,  // file change
    98,  // conversation history
    112, // shell exec
    128, // write blob
    132, // generic tool
];

const PARTS: [&str; 5] = [
    "idx",
    "step_type",
    "metadata",
    "render_info",
    "step_payload",
];

/// The model-facing steps of a view with an index below `k`.
pub fn model_steps(rows: &Rows, k: i64) -> Vec<&Row> {
    steps(rows)
        .filter(|r| r.idx().is_ok_and(|i| i < k))
        .filter(|r| {
            r.get("step_type")
                .and_then(Cell::int)
                .is_some_and(|t| MODEL_FACING.contains(&t))
        })
        .collect()
}

/// The request body of the model-facing steps below index `k`.
pub fn body(rows: &Rows, k: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for r in model_steps(rows, k) {
        let part = Row(PARTS
            .iter()
            .map(|p| (p.to_string(), r.get(p).cloned().unwrap_or(Cell::Null)))
            .collect());
        out.extend_from_slice(&part.encode());
    }
    out
}
