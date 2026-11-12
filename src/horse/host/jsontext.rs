//! Edits of a JSON text that keep every other byte.
//!
//! paddock puts its hook entries into a settings file of the user. It
//! must not write that file again in its own layout: the order of the
//! keys, the spaces and the line breaks are the user's. So each change
//! here is one insertion of text at one place, or one cut of a span, and
//! the rest of the text stays byte for byte.
//!
//! [`parse`] finds the spans of the values of a text that serde_json
//! accepted already. The inserted text follows the layout around it: the
//! indent of its neighbours, and one line per value when they have one,
//! else the compact form.

use anyhow::{bail, Context as _, Result};
use serde_json::Value;

/// One JSON value of the text and where it is.
#[derive(Clone, Debug)]
pub struct Node {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Scalar,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub key: String,
    pub key_start: usize,
    pub value: Node,
}

impl Node {
    pub fn members(&self) -> Option<&[Member]> {
        match &self.kind {
            Kind::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Node]> {
        match &self.kind {
            Kind::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.members()?
            .iter()
            .find(|m| m.key == key)
            .map(|m| &m.value)
    }
}

/// The value of the whole text.
pub fn parse(text: &str) -> Result<Node> {
    let b = text.as_bytes();
    let mut i = ws(b, 0);
    let n = value(text, &mut i)?;
    if ws(b, i) != b.len() {
        bail!("text after the JSON value");
    }
    Ok(n)
}

fn ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

fn value(text: &str, i: &mut usize) -> Result<Node> {
    let b = text.as_bytes();
    let start = *i;
    match b.get(start) {
        Some(b'{') => {
            *i += 1;
            let mut members = Vec::new();
            *i = ws(b, *i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
            } else {
                loop {
                    *i = ws(b, *i);
                    let key_start = *i;
                    string(b, i)?;
                    let key: String = serde_json::from_str(&text[key_start..*i])?;
                    *i = ws(b, *i);
                    if b.get(*i) != Some(&b':') {
                        bail!("no colon after a key");
                    }
                    *i = ws(b, *i + 1);
                    let v = value(text, i)?;
                    members.push(Member {
                        key,
                        key_start,
                        value: v,
                    });
                    *i = ws(b, *i);
                    match b.get(*i) {
                        Some(b',') => *i += 1,
                        Some(b'}') => {
                            *i += 1;
                            break;
                        }
                        _ => bail!("an object without its end"),
                    }
                }
            }
            Ok(Node {
                start,
                end: *i,
                kind: Kind::Object(members),
            })
        }
        Some(b'[') => {
            *i += 1;
            let mut items = Vec::new();
            *i = ws(b, *i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
            } else {
                loop {
                    *i = ws(b, *i);
                    items.push(value(text, i)?);
                    *i = ws(b, *i);
                    match b.get(*i) {
                        Some(b',') => *i += 1,
                        Some(b']') => {
                            *i += 1;
                            break;
                        }
                        _ => bail!("a list without its end"),
                    }
                }
            }
            Ok(Node {
                start,
                end: *i,
                kind: Kind::Array(items),
            })
        }
        Some(b'"') => {
            string(b, i)?;
            Ok(Node {
                start,
                end: *i,
                kind: Kind::Scalar,
            })
        }
        Some(_) => {
            while *i < b.len()
                && !matches!(b[*i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                *i += 1;
            }
            if *i == start {
                bail!("no value");
            }
            Ok(Node {
                start,
                end: *i,
                kind: Kind::Scalar,
            })
        }
        None => bail!("the text ends before a value"),
    }
}

fn string(b: &[u8], i: &mut usize) -> Result<()> {
    if b.get(*i) != Some(&b'"') {
        bail!("no string");
    }
    *i += 1;
    while let Some(&c) = b.get(*i) {
        *i += 1;
        match c {
            b'\\' => *i += 1,
            b'"' => return Ok(()),
            _ => {}
        }
    }
    bail!("a string without its end")
}

/// One insertion: `text` goes in at byte `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Insert {
    pub at: usize,
    pub text: String,
}

impl Insert {
    pub fn apply(&self, text: &str) -> String {
        let mut s = String::with_capacity(text.len() + self.text.len());
        s.push_str(&text[..self.at]);
        s.push_str(&self.text);
        s.push_str(&text[self.at..]);
        s
    }
}

/// The leading spaces of the line that holds byte `at`.
fn line_indent(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let rest = &text[line..];
    let n = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    &rest[..n]
}

/// The indent step of the text: the smallest indent of a line, or two
/// spaces, or a tab when the lines start with tabs.
fn unit(text: &str) -> String {
    let mut best: Option<&str> = None;
    for l in text.lines() {
        let n = l.len() - l.trim_start_matches([' ', '\t']).len();
        if n == 0 || n == l.len() {
            continue;
        }
        if best.is_none_or(|b| n < b.len()) {
            best = Some(&l[..n]);
        }
    }
    match best {
        Some(b) if b.starts_with('\t') => "\t".into(),
        Some(b) => b.into(),
        None => "  ".into(),
    }
}

/// `v` in the layout of the place: with line breaks and `indent` before
/// each inner line, or compact.
fn render(v: &Value, pretty: bool, unit: &str, indent: &str) -> Result<String> {
    if !pretty {
        return Ok(serde_json::to_string(v)?);
    }
    let mut out = Vec::new();
    let f = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut out, f);
    serde::Serialize::serialize(v, &mut ser)?;
    let s = String::from_utf8(out)?;
    Ok(s.replace('\n', &format!("\n{indent}")))
}

/// The separator between the members or items of a container: the text
/// after the comma before its last element, or after its opening bracket
/// (whitespace only).
fn separator<'a>(text: &'a str, open: usize, starts: &[usize], ends: &[usize]) -> &'a str {
    let n = starts.len();
    let from = if n >= 2 {
        text[ends[n - 2]..starts[n - 1]]
            .find(',')
            .map_or(ends[n - 2], |c| ends[n - 2] + c + 1)
    } else {
        open + 1
    };
    &text[from..starts[n - 1]]
}

/// The text between a key and its value in the object, `": "` by default.
fn colon(text: &str, members: &[Member]) -> String {
    members
        .first()
        .map(|m| {
            let key_end = m.key_start + serde_json::to_string(&m.key).map_or(0, |k| k.len());
            text.get(key_end..m.value.start).unwrap_or(": ").to_string()
        })
        .filter(|c| c.trim() == ":")
        .unwrap_or_else(|| ": ".into())
}

/// The insertion that adds `element` (the text of a value, or of a
/// `"key": value` pair, from `make`) at the end of the container `node`.
fn append(
    text: &str,
    node: &Node,
    starts: &[usize],
    ends: &[usize],
    make: impl Fn(bool, &str) -> Result<String>,
) -> Result<Insert> {
    let u = unit(text);
    if starts.is_empty() {
        // An empty container: the element goes on its own line, one step
        // in, unless the container is part of a one-line text.
        let base = line_indent(text, node.start);
        let pretty = text.contains('\n');
        let inner = format!("{base}{u}");
        let el = make(pretty, &inner)?;
        let t = if pretty {
            format!("\n{inner}{el}\n{base}")
        } else {
            el
        };
        return Ok(Insert {
            at: node.start + 1,
            text: t,
        });
    }
    let sep = separator(text, node.start, starts, ends);
    let pretty = sep.contains('\n');
    let indent = match sep.rfind('\n') {
        Some(n) => sep[n + 1..].to_string(),
        None => String::new(),
    };
    let el = make(pretty, &indent)?;
    Ok(Insert {
        at: *ends.last().expect("an element"),
        text: format!(",{sep}{el}"),
    })
}

/// The insertion that adds `value` at the end of the list `node`.
pub fn append_item(text: &str, node: &Node, value: &Value) -> Result<Insert> {
    let items = node.items().context("not a list")?;
    let starts: Vec<usize> = items.iter().map(|n| n.start).collect();
    let ends: Vec<usize> = items.iter().map(|n| n.end).collect();
    let u = unit(text);
    append(text, node, &starts, &ends, |pretty, indent| {
        render(value, pretty, &u, indent)
    })
}

/// The insertion that adds the member `key` with `value` at the end of the
/// object `node`.
pub fn append_member(text: &str, node: &Node, key: &str, value: &Value) -> Result<Insert> {
    let members = node.members().context("not an object")?;
    if members.iter().any(|m| m.key == key) {
        bail!("the object has {key} already");
    }
    let starts: Vec<usize> = members.iter().map(|m| m.key_start).collect();
    let ends: Vec<usize> = members.iter().map(|m| m.value.end).collect();
    let u = unit(text);
    let c = colon(text, members);
    append(text, node, &starts, &ends, |pretty, indent| {
        let k = serde_json::to_string(key)?;
        let c = if pretty {
            c.clone()
        } else {
            c.trim_end().to_string()
        };
        Ok(format!("{k}{c}{}", render(value, pretty, &u, indent)?))
    })
}

/// The text without the element at `[start, end)` of a container whose
/// elements start at `starts` and end at `ends` (`open`, `close`: its
/// brackets). The comma and the space that went with the element go too.
fn cut_element(
    text: &str,
    open: usize,
    close: usize,
    starts: &[usize],
    ends: &[usize],
    n: usize,
) -> String {
    let (from, to) = if n > 0 {
        (ends[n - 1], ends[n])
    } else if starts.len() > 1 {
        (starts[0], starts[1])
    } else {
        (open + 1, close - 1)
    };
    format!("{}{}", &text[..from], &text[to..])
}

/// The text without item `n` of the list `node`.
pub fn cut_item(text: &str, node: &Node, n: usize) -> Result<String> {
    let items = node.items().context("not a list")?;
    let starts: Vec<usize> = items.iter().map(|x| x.start).collect();
    let ends: Vec<usize> = items.iter().map(|x| x.end).collect();
    anyhow::ensure!(n < items.len(), "no item {n}");
    Ok(cut_element(text, node.start, node.end, &starts, &ends, n))
}

/// The text without the member `key` of the object `node`.
pub fn cut_member(text: &str, node: &Node, key: &str) -> Result<String> {
    let members = node.members().context("not an object")?;
    let n = members
        .iter()
        .position(|m| m.key == key)
        .with_context(|| format!("no member {key}"))?;
    let starts: Vec<usize> = members.iter().map(|m| m.key_start).collect();
    let ends: Vec<usize> = members.iter().map(|m| m.value.end).collect();
    Ok(cut_element(text, node.start, node.end, &starts, &ends, n))
}

#[cfg(test)]
#[path = "jsontext_tests.rs"]
mod tests;
