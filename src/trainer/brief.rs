//! Build the bounded, read-only summary supplied to a trainer.
//!
//! The source adapter provides observations rather than inferring success
//! from natural-language claims. Unknown results remain unknown.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_TURNS: usize = 8;
pub const DEFAULT_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug)]
pub struct ChecklistItem {
    pub text: String,
    pub claimed: bool,
    pub passed: bool,
    pub elapsed_secs: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Observation {
    Turn { role: String, text: String },
    Commit(String),
    Check { name: String, passed: Option<bool> },
    Question(String),
    Status(String),
    Event(String),
}

#[derive(Clone, Debug, Default)]
pub struct Source {
    pub goal: String,
    pub checklist: Vec<ChecklistItem>,
    pub checklist_known: bool,
    pub checklist_source: String,
    pub observations: Vec<Observation>,
    pub diff_stat: String,
    pub gauge: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub text: String,
    pub claimed: bool,
    pub passed: bool,
    pub elapsed_secs: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub passed: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Turn {
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Brief {
    pub goal: String,
    pub checklist: Vec<Item>,
    pub checklist_known: bool,
    pub checklist_source: String,
    pub turns: Vec<Turn>,
    pub commits: Vec<String>,
    pub checks: Vec<Check>,
    pub repeated_failures: BTreeMap<String, usize>,
    pub repeated_questions: BTreeMap<String, usize>,
    pub statuses: Vec<String>,
    pub events: Vec<String>,
    pub diff_stat: String,
    pub gauge: BTreeMap<String, i64>,
    pub omitted: BTreeMap<String, usize>,
}

/// Fail closed for fields mentioning a protected path. Do not include even
/// the name of the path in the replacement. Matching components also catches
/// absolute paths, and punctuation surrounding a path in a command or JSON.
fn protected(text: &str, hidden: &[PathBuf]) -> bool {
    if hidden
        .iter()
        .any(|h| !h.as_os_str().is_empty() && text.contains(h.to_string_lossy().as_ref()))
    {
        return true;
    }
    if hidden
        .iter()
        .filter_map(|h| h.file_name())
        .filter_map(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .any(|name| {
            text.split(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '/' | '\"'
                            | '\''
                            | '`'
                            | ','
                            | ';'
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '='
                            | ':'
                    )
            })
            .any(|word| word == name)
        })
    {
        return true;
    }
    text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '=' | ':'
            )
    })
    .filter(|word| !word.is_empty())
    .any(|word| {
        let path = Path::new(word);
        path.components().any(|part| match part {
            Component::Normal(name) => crate::detect::is_secret_name(&name.to_string_lossy()),
            _ => false,
        }) || hidden.iter().any(|h| {
            !h.as_os_str().is_empty()
                && (path == h
                    || path.starts_with(h)
                    || word.contains(&h.to_string_lossy().into_owned()))
        })
    })
}

fn safe(text: &str, hidden: &[PathBuf]) -> String {
    if protected(text, hidden) {
        "[protected content omitted]".into()
    } else {
        text.into()
    }
}

fn repeats(values: impl Iterator<Item = String>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_insert(0) += 1;
    }
    counts.retain(|_, n| *n > 1);
    counts
}

/// This function does not write to the session or filesystem. Limits apply to
/// recent turns; other facts describe the whole supplied observation stream.
pub fn build(source: &Source, hidden: &[PathBuf], turn_limit: usize, byte_limit: usize) -> Brief {
    let mut turns = Vec::new();
    let mut remaining = byte_limit;
    let mut completed_turns = 0;
    for observation in source.observations.iter().rev() {
        let Observation::Turn { role, text } = observation else {
            continue;
        };
        if completed_turns == turn_limit || remaining == 0 {
            break;
        }
        let text = safe(text, hidden);
        let mut start = text.len().saturating_sub(remaining);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let part = text[start..].to_string();
        remaining -= part.len();
        turns.push(Turn {
            role: safe(role, hidden),
            text: part,
        });
        if role == "user" {
            completed_turns += 1;
        }
    }
    turns.reverse();
    let checks: Vec<Check> = source
        .observations
        .iter()
        .filter_map(|o| match o {
            Observation::Check { name, passed } => Some(Check {
                name: safe(name, hidden),
                passed: *passed,
            }),
            _ => None,
        })
        .collect();
    let failures = repeats(
        checks
            .iter()
            .filter(|c| c.passed == Some(false) && c.name != "[protected content omitted]")
            .map(|c| c.name.clone()),
    );
    let questions = repeats(source.observations.iter().filter_map(|o| match o {
        Observation::Question(q) if !protected(q, hidden) => Some(q.trim().to_owned()),
        _ => None,
    }));
    let collect = |select: fn(&Observation) -> Option<&String>| {
        source
            .observations
            .iter()
            .filter_map(select)
            .map(|s| safe(s, hidden))
            .collect()
    };
    Brief {
        goal: safe(&source.goal, hidden),
        checklist_known: source.checklist_known,
        checklist_source: safe(&source.checklist_source, hidden),
        checklist: source
            .checklist
            .iter()
            .map(|i| Item {
                text: safe(&i.text, hidden),
                claimed: i.claimed,
                passed: i.passed,
                elapsed_secs: i.elapsed_secs,
            })
            .collect(),
        turns,
        commits: collect(|o| {
            if let Observation::Commit(s) = o {
                Some(s)
            } else {
                None
            }
        }),
        checks,
        repeated_failures: failures,
        repeated_questions: questions,
        statuses: collect(|o| {
            if let Observation::Status(s) = o {
                Some(s)
            } else {
                None
            }
        }),
        events: collect(|o| {
            if let Observation::Event(s) = o {
                Some(s)
            } else {
                None
            }
        }),
        diff_stat: safe(&source.diff_stat, hidden),
        omitted: BTreeMap::new(),
        gauge: source
            .gauge
            .iter()
            .filter(|(k, _)| !protected(k, hidden))
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
    }
}

/// Serialize within a byte budget. Recent details win over old details;
/// repeated-signal counts, the checklist and its states remain present.
/// Fail rather than silently drop those essential facts if they cannot fit.
pub fn render(mut brief: Brief, max_bytes: usize) -> anyhow::Result<String> {
    loop {
        let encoded = serde_json::to_string(&brief)?;
        if encoded.len() <= max_bytes {
            return Ok(encoded);
        }
        let candidates = [
            brief.turns.len(),
            brief.checks.len(),
            brief.statuses.len(),
            brief.events.len(),
            brief.commits.len(),
        ];
        let Some((which, len)) = candidates
            .into_iter()
            .enumerate()
            .max_by_key(|(_, len)| *len)
            .filter(|(_, len)| *len > 0)
        else {
            anyhow::bail!(
                "trainer brief's goal, checklist and repeated facts exceed the byte limit"
            );
        };
        let count = len.div_ceil(2);
        let name = match which {
            0 => {
                brief.turns.drain(..count);
                "turns"
            }
            1 => {
                brief.checks.drain(..count);
                "checks"
            }
            2 => {
                brief.statuses.drain(..count);
                "statuses"
            }
            3 => {
                brief.events.drain(..count);
                "events"
            }
            _ => {
                brief.commits.drain(..count);
                "commits"
            }
        };
        *brief.omitted.entry(name.into()).or_insert(0) += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_run_summarizes_full_history_and_limits_recent_turns() {
        let mut source = Source {
            goal: "finish the ticket".into(),
            ..Default::default()
        };
        source.checklist_known = true;
        source.checklist.push(ChecklistItem {
            text: "prove the test".into(),
            claimed: false,
            passed: false,
            elapsed_secs: Some(90),
        });
        source
            .observations
            .push(Observation::Commit("abc123".into()));
        for _ in 0..3 {
            source.observations.push(Observation::Check {
                name: "cargo test".into(),
                passed: Some(false),
            });
        }
        for _ in 0..2 {
            source
                .observations
                .push(Observation::Question("Which branch?".into()));
        }
        source.observations.push(Observation::Check {
            name: "sandbox test".into(),
            passed: None,
        });
        source
            .observations
            .push(Observation::Status("done: partial".into()));
        source.observations.push(Observation::Event("idle".into()));
        for n in 0..10 {
            source.observations.push(Observation::Turn {
                role: "user".into(),
                text: format!("question {n}"),
            });
            source.observations.push(Observation::Turn {
                role: "assistant".into(),
                text: format!("turn {n}"),
            });
        }
        source.diff_stat = "2 files changed".into();
        source.gauge.insert("open".into(), 1);
        let brief = build(&source, &[], 8, 1000);
        assert_eq!(brief.turns.len(), 16);
        assert_eq!(brief.turns[0].text, "question 2");
        assert_eq!(brief.repeated_failures["cargo test"], 3);
        assert!(brief
            .checks
            .iter()
            .any(|check| check.name == "sandbox test" && check.passed.is_none()));
        assert_eq!(brief.repeated_questions["Which branch?"], 2);
        assert_eq!(brief.commits, ["abc123"]);
        assert_eq!(brief.statuses, ["done: partial"]);
        assert_eq!(brief.events, ["idle"]);
        assert!(brief.checklist_known);
        assert_eq!(brief.checklist[0].elapsed_secs, Some(90));
        assert_eq!(brief.gauge["open"], 1);
        assert_eq!(brief.diff_stat, "2 files changed");
    }

    #[test]
    fn protected_paths_are_omitted_from_every_serialized_field() {
        let hidden = vec![
            PathBuf::from("private/config"),
            PathBuf::from("private key/value"),
        ];
        let mut source = Source {
            goal: "read private/config".into(),
            diff_stat: "a/.env | 1".into(),
            ..Default::default()
        };
        source.checklist.push(ChecklistItem {
            text: "private/config".into(),
            claimed: false,
            passed: false,
            elapsed_secs: None,
        });
        source.observations.push(Observation::Turn {
            role: "user".into(),
            text: "cat /tmp/.env".into(),
        });
        source.observations.push(Observation::Check {
            name: "test private/config".into(),
            passed: Some(false),
        });
        source
            .observations
            .push(Observation::Status("blocked: private/config".into()));
        source.gauge.insert("private/config".into(), 1);
        source
            .observations
            .push(Observation::Event("read private key/value".into()));
        let json = serde_json::to_string(&build(&source, &hidden, 8, 1000)).unwrap();
        assert!(!json.contains("private/config"));
        assert!(!json.contains("private key/value"));
        assert!(!json.contains(".env"));
    }

    #[test]
    fn serialized_brief_has_a_hard_limit_and_reports_omissions() {
        let mut source = Source {
            goal: "ship it".into(),
            ..Default::default()
        };
        for n in 0..100 {
            source.observations.push(Observation::Status(format!(
                "working {n}: {}",
                "x".repeat(100)
            )));
        }
        let brief = build(&source, &[], 8, 4096);
        let json = render(brief, 700).unwrap();
        assert!(json.len() <= 700);
        assert!(json.contains("omitted"));
        assert!(json.contains("working 99"));
        assert!(!json.contains("working 0:"));
        let source = Source {
            goal: "x".repeat(1000),
            ..Default::default()
        };
        assert!(render(build(&source, &[], 8, 4096), 256).is_err());
    }

    #[test]
    fn turn_limit_preserves_utf8() {
        let source = Source {
            observations: vec![Observation::Turn {
                role: "user".into(),
                text: "abcé".into(),
            }],
            ..Default::default()
        };
        assert_eq!(build(&source, &[], 1, 1).turns[0].text, "");
        assert_eq!(build(&source, &[], 1, 2).turns[0].text, "é");
    }
}
