//! The prefix guard: does an operation keep the cached prompt prefix?
//!
//! Per session the guard keeps a state tuple: the DAG head, model, effort,
//! tool-set hash, system-prompt hash, cwd, harness version, native id and
//! the time of the last request. Before an operation that changes what the
//! model sees, it compares the prompt of the operation with the prompt of
//! the session, piece by piece, with the request port of the dsh codec
//! (dsh/request.rs). DeepSeek caches by content prefix, and keeps a prefix
//! unit at the end of each request input and output (kv_cache guide). The
//! rules for dsh:
//! - an edit or insert before the tail breaks from that point,
//! - a model or provider change makes the whole cache cold,
//! - a tool-set change breaks from the tools; on a model with
//!   `toolUpdate: addition-only` an added tool is appended,
//! - a system-prompt change breaks from token 0, unless the model has
//!   `systemPromptUpdate: in-history`, where dsh appends it,
//! - a cwd change rewrites the descriptor text from its first use,
//! - a fork keeps the prefix up to the last request end before the cut,
//! - unused caches go within a few hours; a request within seconds of the
//!   last reply can come before the cache is built.
//!
//! A Claude Code session takes its shape and its cache time from the
//! requests that Claude Code records (guard/claude.rs).

pub mod agy;
pub mod claude;
pub mod codex;
pub mod grok;

use super::dsh::request::{build, prompt_pieces, J};
use super::dsh::Header;
use super::store::{Hash, Store};
use super::Session;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// The low end of "a few hours to a few days" of the kv_cache guide.
pub const TTL_MS: u64 = 3 * 3600 * 1000;
/// "Cache construction takes seconds."
pub const TOO_SOON_MS: u64 = 5_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuple {
    pub head: Option<Hash>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub tools_hash: Option<Hash>,
    pub system_hash: Option<Hash>,
    pub cwd: Option<String>,
    pub harness_version: String,
    pub native_id: Option<String>,
    pub last_request_ms: Option<u64>,
}

/// An operation that changes what the model sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// A new session whose head is this node.
    Fork {
        at: Hash,
    },
    /// An entry after this node; `None` is after the head.
    Append {
        after: Option<Hash>,
    },
    Model(String),
    Effort(String),
    /// A change of the tool set. `added_only`: tools are added, none is
    /// removed or changed.
    Tools {
        added_only: bool,
    },
    System,
    Cwd(String),
    /// A DNA entry at the end: a safe append.
    DnaChange,
    /// A move of the session to horse `to`. The DNA of the target comes as
    /// one entry at the end, so a move is a safe append.
    MoveHorse {
        to: String,
    },
    /// The session continues at node `to` under the same native id: its
    /// native file is swapped for the projection of that branch. Only for
    /// harnesses whose cache is keyed by the thread id (guard/codex.rs).
    Swap {
        to: Hash,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub session: String,
    pub op: String,
    /// The cached prefix stays.
    pub keeps: bool,
    /// The size of the prompt that the cache can hold now.
    pub prefix_bytes: u64,
    pub lost_bytes: u64,
    pub lost_tokens: u64,
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Facts for the user that are no warning, for example a warm cache.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub at_ms: u64,
}

impl Verdict {
    /// The text for the user.
    pub fn message(&self) -> String {
        let mut s = if self.keeps {
            format!(
                "prefix guard: {} keeps the cached prefix of session {}",
                self.op, self.session
            )
        } else if self.lost_bytes == 0 {
            format!(
                "prefix guard: {} loses about {} tokens of the cached prefix of session {}",
                self.op, self.lost_tokens, self.session
            )
        } else {
            format!(
                "prefix guard: {} loses about {} tokens ({} bytes) of the cached prefix of session {}",
                self.op, self.lost_tokens, self.lost_bytes, self.session
            )
        };
        for r in self.reasons.iter().chain(&self.notes) {
            s += &format!("\n  {r}");
        }
        if let Some(x) = &self.suggestion {
            s += &format!("\n  instead: {x}");
        }
        s
    }
}

/// What the guard knows of a session.
struct Prompt {
    rows: Vec<J>,
    hashes: Vec<Hash>,
    /// The pieces of the prompt up to the last request output, or empty
    /// when no request was sent.
    pieces: Vec<String>,
    tuple: Tuple,
    /// Prompt tokens per byte, from the usage of the last reply.
    scale: Option<f64>,
    system_in_history: bool,
    tools_append: bool,
    claude: Option<claude::Facts>,
}

fn ty(r: &J) -> &str {
    r.get("type").and_then(J::str).unwrap_or_default()
}

fn data<'a>(r: &'a J, path: &[&str]) -> Option<&'a J> {
    let mut v = r.get("data")?;
    for k in path {
        v = v.get(k)?;
    }
    Some(v)
}

fn num(v: Option<&J>) -> Option<u64> {
    match v? {
        J::Num(n) => n.as_u64(),
        _ => None,
    }
}

fn bytes(p: &[String]) -> u64 {
    p.iter().map(|s| s.len() as u64).sum()
}

/// The pieces of the request that dsh sends after these rows, if it can be
/// built. A cut inside a tool round cannot.
fn pieces(rows: &[J]) -> Option<Vec<String>> {
    build(rows).ok().map(|b| prompt_pieces(&b))
}

/// The bytes of the longest common prefix of two piece lists. A piece
/// that differs counts up to its first different byte.
fn common(a: &[String], b: &[String]) -> u64 {
    let mut n = 0;
    for (x, y) in a.iter().zip(b) {
        if x == y {
            n += x.len() as u64;
        } else {
            n += x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count() as u64;
            break;
        }
    }
    n
}

fn load(store: &Store, s: &Session) -> Result<Prompt> {
    let nodes = store.session_nodes(s)?;
    let rows: Vec<J> = nodes
        .iter()
        .map(|n| J::parse(&n.raw))
        .collect::<Result<_>>()
        .context("a session row is not JSON")?;
    let hashes = nodes.iter().map(|n| n.hash.clone()).collect();
    let last_reply = rows.iter().rposition(|r| ty(r) == "assistant/message");
    let pieces = last_reply
        .and_then(|i| pieces(&rows[..=i]))
        .unwrap_or_default();
    let header = rows.iter().rev().find(|r| ty(r) == "request/header");
    let config = header.and_then(|h| data(h, &["header", "config"]));
    let cfg = |k: &str| {
        config
            .and_then(|c| c.get(k))
            .and_then(J::str)
            .map(String::from)
    };
    let context = rows.iter().rev().find(|r| ty(r) == "request/context");
    let mode = |k: &str| context.and_then(|c| data(c, &[k])).and_then(J::str);
    let usage = last_reply.and_then(|i| data(&rows[i], &["usage"]));
    let tokens: u64 = [
        "inputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
    ]
    .iter()
    .filter_map(|k| num(usage.and_then(|u| u.get(k))))
    .sum();
    let cwd = match &s.native_header {
        Some(h) => Header::parse(&store.get(h)?.raw).ok().and_then(|h| h.cwd),
        None => None,
    };
    let mut tuple = Tuple {
        head: s.head.clone(),
        model: cfg("model"),
        effort: cfg("reasoningEffort"),
        tools_hash: header
            .and_then(|h| data(h, &["header", "tools"]))
            .map(|t| Hash::of(t.text().as_bytes())),
        system_hash: pieces.first().map(|p| Hash::of(p.as_bytes())),
        cwd,
        harness_version: if s.binding.harness == crate::horse::codex::HARNESS {
            s.codex_reader_version
                .clone()
                .unwrap_or_else(|| s.binding.version.clone())
        } else {
            s.binding.version.clone()
        },
        native_id: s.binding.native_id.clone(),
        last_request_ms: last_reply.and_then(|i| num(rows[i].get("time"))),
    };
    let claude = claude::read(&s.binding.harness, &nodes, &mut tuple);
    let total = bytes(&pieces);
    Ok(Prompt {
        scale: (tokens > 0 && total > 0).then(|| tokens as f64 / total as f64),
        system_in_history: mode("systemPromptUpdate") == Some("in-history"),
        tools_append: mode("toolUpdate") == Some("addition-only")
            || cfg("model").as_deref() == Some("deepseek-flash"),
        rows,
        hashes,
        pieces,
        tuple,
        claude,
    })
}

/// The state tuple of a session.
pub fn tuple(store: &Store, s: &Session) -> Result<Tuple> {
    if s.binding.harness == agy::HARNESS {
        return Ok(agy::tuple(s));
    }
    Ok(load(store, s)?.tuple)
}

impl Prompt {
    fn index(&self, h: &Hash) -> Result<usize> {
        self.hashes
            .iter()
            .position(|x| x == h)
            .with_context(|| format!("node {} is not in the session", h.short()))
    }

    /// The cached bytes that a prompt of rows `..=k` reuses: the common
    /// prefix with the session prompt, up to the last request end at or
    /// before `k`.
    fn kept_until(&self, k: usize) -> (u64, bool) {
        let end = self.rows[..=k]
            .iter()
            .rposition(|r| ty(r) == "assistant/message");
        let at_end = self.rows[..=k]
            .iter()
            .rev()
            .find(|r| r.get("surfaceOp").is_some())
            .is_some_and(|r| ty(r) == "assistant/message");
        let mut j = end;
        while let Some(i) = j {
            if let Some(p) = pieces(&self.rows[..=i]) {
                return (common(&self.pieces, &p).min(bytes(&p)), at_end);
            }
            j = self.rows[..i]
                .iter()
                .rposition(|r| ty(r) == "assistant/message");
        }
        (0, at_end)
    }
}

/// The verdict of the guard for an operation on a session.
pub fn check(store: &Store, session: &str, op: &Op, now_ms: u64) -> Result<Verdict> {
    let s = store.session(session)?;
    if let Some(v) = codex::check(store, &s, op, now_ms)? {
        return Ok(v);
    }
    if let Some(v) = grok::check(store, &s, op, now_ms)? {
        return Ok(v);
    }
    if let Some(v) = agy::check(store, &s, op, now_ms)? {
        return Ok(v);
    }
    let p = load(store, &s)?;
    let total = bytes(&p.pieces);
    let mut reasons = Vec::new();
    let mut suggestion = None;
    let head = p.rows.len().checked_sub(1);
    let (name, lost) = match op {
        Op::Fork { at } => {
            let k = p.index(at)?;
            if Some(k) == head {
                ("a fork at the head".to_string(), 0)
            } else {
                let child = pieces(&p.rows[..=k]).unwrap_or_default();
                let (kept, at_end) = p.kept_until(k);
                if !at_end {
                    reasons.push(format!(
                        "the cut at {} is not at the end of a reply, so the cache holds only the prompt up to the last reply before it",
                        at.short()
                    ));
                }
                if ty(&p.rows[k]) != "turn/end" {
                    reasons.push("dsh takes a projected fork only at a turn/end row".to_string());
                    suggestion = Some(format!(
                        "fork at the turn/end of the same turn, for example --at seq:{}",
                        p.rows[k..]
                            .iter()
                            .position(|r| ty(r) == "turn/end")
                            .map_or(k, |d| k + d)
                    ));
                }
                (
                    format!("a fork at {}", at.short()),
                    bytes(&child).saturating_sub(kept),
                )
            }
        }
        Op::Append { after } => match after.as_ref().map(|h| p.index(h)).transpose()? {
            None => ("an append at the end".to_string(), 0),
            Some(k) if Some(k) == head => ("an append at the end".to_string(), 0),
            Some(k) => {
                let (kept, _) = p.kept_until(k);
                reasons.push(format!(
                    "an entry before the tail moves every later message, so the cache misses from {}",
                    p.hashes[k].short()
                ));
                suggestion = Some(format!(
                    "paddock horse append {session} --role ROLE --text TEXT (at the end)"
                ));
                (
                    format!("an append after {}", p.hashes[k].short()),
                    total.saturating_sub(kept),
                )
            }
        },
        Op::Model(m) if p.tuple.model.as_deref() == Some(m.as_str()) => (format!("model {m}"), 0),
        Op::Model(m) => {
            reasons.push(format!(
                "model {} to {m}: another model is another cache, so the whole prefix is cold",
                p.tuple.model.as_deref().unwrap_or("?")
            ));
            suggestion = Some(format!(
                "paddock horse fork {session} --at <head> and run the fork on {m}, so this session keeps its cache"
            ));
            (format!("a switch to model {m}"), total)
        }
        Op::Effort(e) if p.tuple.effort.as_deref() == Some(e.as_str()) => {
            (format!("effort {e}"), 0)
        }
        Op::Effort(e) => {
            reasons.push(
                "the effort is a request field before the messages; its cache effect in dsh is not verified".into(),
            );
            let before = p.pieces.iter().take(3).map(|x| x.len() as u64).sum::<u64>();
            (format!("effort {e}"), total.saturating_sub(before))
        }
        Op::Tools { added_only } if *added_only && p.tools_append => {
            reasons.push("on this model dsh appends added tools as a developer message".into());
            ("an added tool".to_string(), 0)
        }
        Op::Tools { .. } => {
            reasons
                .push("the tool list comes after the system prompt and before all messages".into());
            if p.tools_append {
                suggestion = Some(
                    "only add tools: on this model dsh appends additions to the history".into(),
                );
            }
            let sys = p.pieces.first().map_or(0, |x| x.len() as u64);
            ("a tool-set change".to_string(), total.saturating_sub(sys))
        }
        Op::System if p.system_in_history => {
            reasons.push("this model takes system-prompt updates in the history (systemPromptUpdate: in-history)".into());
            ("a system-prompt change".to_string(), 0)
        }
        Op::System => {
            reasons.push("the system prompt is the first piece of the prompt".into());
            ("a system-prompt change".to_string(), total)
        }
        Op::DnaChange => ("a DNA entry at the end".to_string(), 0),
        Op::MoveHorse { to } => {
            reasons.push(format!(
                "the DNA of horse {to} comes as one entry at the end; nothing before the head changes"
            ));
            (format!("a move to horse {to}"), 0)
        }
        Op::Swap { .. } => anyhow::bail!(
            "a swap under the same id is for Codex sessions, whose cache is keyed by the thread id; a {} session forks instead",
            s.binding.harness
        ),
        Op::Cwd(c) => {
            let old = p.tuple.cwd.clone().unwrap_or_default();
            let first = (!old.is_empty() && old != *c)
                .then(|| p.pieces.iter().position(|x| x.contains(&old)))
                .flatten();
            match first {
                Some(i) => {
                    reasons.push(format!("dsh rewrites the descriptor text that names {old}"));
                    let before = p.pieces[..i].iter().map(|x| x.len() as u64).sum::<u64>();
                    (format!("a cwd change to {c}"), total.saturating_sub(before))
                }
                None => (format!("a cwd change to {c}"), 0),
            }
        }
    };
    let mut lost = lost;
    if let Some(t) = p.tuple.last_request_ms.filter(|_| p.claude.is_none()) {
        let idle = now_ms.saturating_sub(t);
        if idle > TTL_MS {
            reasons.push(format!(
                "the last request was {} min ago; DeepSeek clears unused caches within a few hours",
                idle / 60_000
            ));
            lost = lost.max(total);
        } else if idle < TOO_SOON_MS {
            reasons.push(
                "the last reply came less than 5 s ago; the cache may not be built yet".into(),
            );
        }
    }
    let keeps = lost == 0;
    let lost_tokens = match p.scale {
        Some(r) => (lost as f64 * r).round() as u64,
        None => lost.div_ceil(4),
    };
    let mut v = Verdict {
        session: session.into(),
        op: name,
        keeps,
        prefix_bytes: total,
        lost_bytes: lost,
        lost_tokens,
        reasons,
        suggestion,
        notes: Vec::new(),
        at_ms: now_ms,
    };
    if let Some(f) = &p.claude {
        let saved = store
            .guard_state(session)
            .map(|x| x.tuple)
            .unwrap_or_else(|e| {
                log::warn!("horse guard {session}: {e:#}");
                Tuple::default()
            });
        claude::judge(&mut v, op, f, &saved, &p.tuple, now_ms);
    }
    Ok(v)
}

/// The guard state of a session: its tuple and the last verdict.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub tuple: Tuple,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Verdict>,
}

impl Store {
    pub fn guard_state(&self, session: &str) -> Result<State> {
        match self.read_ref(&format!("guard/{session}.json"))? {
            Some(b) => serde_json::from_slice(&b)
                .with_context(|| format!("the guard state of {session} is damaged")),
            None => Ok(State::default()),
        }
    }

    /// Save the current tuple of a session, and keep its last verdict. A
    /// Claude Code shape change since the saved tuple is a break: its
    /// verdict becomes the last one.
    pub fn refresh_guard(&self, session: &str) -> Result<()> {
        let mut st = self.guard_state(session)?;
        let s = self.session(session)?;
        let new = tuple(self, &s)?;
        if claude::shape_changed(&s.binding.harness, &st.tuple, &new) {
            let v = check(self, session, &Op::Append { after: None }, super::now_ms())?;
            log::info!("{}", v.message());
            st.last = Some(v);
        }
        st.tuple = new;
        self.write_ref(
            &format!("guard/{session}.json"),
            &serde_json::to_vec_pretty(&st)?,
        )
    }

    /// Save a verdict with the current tuple of its session.
    pub fn save_verdict(&self, v: &Verdict) -> Result<()> {
        let s = self.session(&v.session)?;
        let st = State {
            tuple: tuple(self, &s)?,
            last: Some(v.clone()),
        };
        self.write_ref(
            &format!("guard/{}.json", v.session),
            &serde_json::to_vec_pretty(&st)?,
        )
    }
}
