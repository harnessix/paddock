//! Byte-exact edits of the top-level fields of a JSON object row.
//!
//! A row is never parsed and written again as a whole: numbers, escapes and
//! the bytes of opaque content (thinking signatures, encrypted blocks) stay
//! as they are. Only the named top-level fields change.

use anyhow::{bail, Context as _, Result};
use std::ops::Range;

/// One top-level member: its key, the span from the key to the end of the
/// value, and the span of the value.
#[derive(Debug, Clone)]
pub struct Member {
    pub key: String,
    pub whole: Range<usize>,
    pub value: Range<usize>,
}

fn ws(raw: &[u8], mut i: usize) -> usize {
    while i < raw.len() && matches!(raw[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// The end of the string that starts at `i` (a quote), after its closing quote.
fn string_end(raw: &[u8], i: usize) -> Result<usize> {
    let mut j = i + 1;
    while j < raw.len() {
        match raw[j] {
            b'\\' => j += 2,
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    bail!("a JSON string has no end")
}

/// The end of the value that starts at `i`.
fn value_end(raw: &[u8], i: usize) -> Result<usize> {
    match raw.get(i) {
        Some(b'"') => string_end(raw, i),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            let mut j = i;
            while j < raw.len() {
                match raw[j] {
                    b'"' => {
                        j = string_end(raw, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            bail!("a JSON value has no end")
        }
        Some(_) => {
            let mut j = i;
            while j < raw.len()
                && !matches!(raw[j], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
            {
                j += 1;
            }
            Ok(j)
        }
        None => bail!("a JSON value is missing"),
    }
}

/// The top-level members of a row that must be a JSON object.
pub fn members(raw: &[u8]) -> Result<Vec<Member>> {
    serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(raw)
        .context("a Claude Code row is not a JSON object")?;
    let mut out = Vec::new();
    let mut i = ws(raw, 0) + 1;
    loop {
        i = ws(raw, i);
        if raw.get(i) == Some(&b'}') {
            return Ok(out);
        }
        let ks = i;
        let ke = string_end(raw, ks)?;
        let key: String = serde_json::from_slice(&raw[ks..ke])?;
        i = ws(raw, ke) + 1;
        let vs = ws(raw, i);
        let ve = value_end(raw, vs)?;
        out.push(Member {
            key,
            whole: ks..ve,
            value: vs..ve,
        });
        i = ws(raw, ve);
        match raw.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return Ok(out),
            _ => bail!("a Claude Code row has a bad object"),
        }
    }
}

/// The raw bytes of a top-level value, when the row has the key.
pub fn raw_value<'a>(raw: &'a [u8], key: &str) -> Result<Option<&'a [u8]>> {
    Ok(members(raw)?
        .into_iter()
        .rev()
        .find(|m| m.key == key)
        .map(|m| &raw[m.value]))
}

/// A JSON string literal.
pub fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("a string encodes")
}

/// Change a row: `set` gives keys and raw JSON values. A key that is there
/// keeps its place, as a JavaScript assignment does. A new key goes to the
/// end. Keys in `remove` go away. Every other byte stays.
pub fn edit(raw: &[u8], set: &[(&str, String)], remove: &[&str]) -> Result<Vec<u8>> {
    let ms = members(raw)?;
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for m in &ms {
        if remove.contains(&m.key.as_str()) {
            continue;
        }
        match set.iter().find(|(k, _)| *k == m.key) {
            Some((_, v)) => {
                let mut p = raw[m.whole.start..m.value.start].to_vec();
                p.extend_from_slice(v.as_bytes());
                parts.push(p);
            }
            None => parts.push(raw[m.whole.clone()].to_vec()),
        }
    }
    for (k, v) in set {
        if !ms.iter().any(|m| m.key == *k) {
            parts.push(format!("{}:{v}", quote(k)).into_bytes());
        }
    }
    let mut out = b"{".to_vec();
    out.extend(parts.join(&b","[..]));
    out.push(b'}');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_other_bytes() {
        let raw = br#"{"a":1.50,"s":"x\"}\u00e9","o":{"k":[1,{"z":"]"}]},"n":null}"#;
        let ms = members(raw).unwrap();
        assert_eq!(
            ms.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(),
            ["a", "s", "o", "n"]
        );
        assert_eq!(
            raw_value(raw, "o").unwrap().unwrap(),
            br#"{"k":[1,{"z":"]"}]}"#
        );
        let e = edit(raw, &[("n", quote("v")), ("new", "true".into())], &["a"]).unwrap();
        assert_eq!(
            e,
            br#"{"s":"x\"}\u00e9","o":{"k":[1,{"z":"]"}]},"n":"v","new":true}"#
        );
        assert!(members(b"[1]").is_err());
        assert!(members(b"{\"a\":").is_err());
    }
}
