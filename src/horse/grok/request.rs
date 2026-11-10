//! The model-facing history of a Grok session, for the offline proof.
//!
//! Grok sends the items of `chat_history.jsonl` to the model as the input
//! of a request, in file order. [`body`] builds that input array from the
//! raw items, byte for byte, so two sessions whose bodies are equal give
//! the model the same history. [`cut`] is the cut of a native fork at a
//! prompt: the items before the first user item of a later prompt.

use super::replay::parse_chat;
use anyhow::Result;

/// The input array of a request: the raw items, joined by commas.
pub fn body(chat: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"[".to_vec();
    for (i, item) in chat.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(item);
    }
    out.push(b']');
    out
}

/// How many items a native fork keeps when it keeps prompts `0..=turn`:
/// the items before the first user item whose `prompt_index` is greater.
pub fn cut(chat: &[Vec<u8>], turn: u64) -> Result<usize> {
    for (i, item) in chat.iter().enumerate() {
        if parse_chat(item)?.prompt_index.is_some_and(|p| p > turn) {
            return Ok(i);
        }
    }
    Ok(chat.len())
}
