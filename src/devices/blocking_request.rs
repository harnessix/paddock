//! The request of a waiting device: plain text, or a JSON object with the
//! main text, a `timeout` and, for `ask`, the `choices`. Each check here
//! fails with `Invalid` before anything waits or reaches the card.

use super::{broker, waiting, DevError, MAX_CHOICE, MAX_CHOICES, MAX_QUESTION};

/// A request: plain text, or a JSON object. `field` names the main text in
/// the JSON form.
pub(super) struct Parsed {
    pub(super) text: String,
    pub(super) timeout: u64,
    pub(super) json: serde_json::Value,
}

pub(super) fn parse(request: &[u8], field: &str) -> Result<Parsed, DevError> {
    let raw = std::str::from_utf8(request).map_err(|_| DevError::Invalid)?;
    let t = raw.trim();
    if t.starts_with('{') {
        let json: serde_json::Value = serde_json::from_str(t).map_err(|_| DevError::Invalid)?;
        let obj = json.as_object().ok_or(DevError::Invalid)?;
        for k in obj.keys() {
            if k != field && k != "timeout" && k != "choices" {
                return Err(DevError::Invalid);
            }
        }
        let text = obj
            .get(field)
            .and_then(|v| v.as_str())
            .ok_or(DevError::Invalid)?
            .to_string();
        let asked = match obj.get("timeout") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or(DevError::Invalid)?),
        };
        let timeout = waiting::read_timeout(asked).map_err(|_| DevError::Invalid)?;
        return Ok(Parsed {
            text,
            timeout,
            json,
        });
    }
    Ok(Parsed {
        text: t.to_string(),
        timeout: waiting::DEFAULT_READ_TIMEOUT,
        json: serde_json::Value::Null,
    })
}

pub(super) fn check_question(p: &Parsed) -> Result<(String, Vec<String>), DevError> {
    // Control and invisible format characters never reach the card, a
    // notification or a terminal.
    let q = broker::clean_text(p.text.trim()).trim().to_string();
    if q.is_empty() || q.chars().count() > MAX_QUESTION {
        return Err(DevError::Invalid);
    }
    let mut choices = Vec::new();
    if let Some(c) = p.json.get("choices") {
        let list = c.as_array().ok_or(DevError::Invalid)?;
        if list.len() > MAX_CHOICES {
            return Err(DevError::Invalid);
        }
        for v in list {
            let s = v.as_str().ok_or(DevError::Invalid)?.trim();
            if s.is_empty()
                || s.chars().count() > MAX_CHOICE
                || broker::clean_text(s) != s
                || s.contains(['\n', '\t'])
            {
                return Err(DevError::Invalid);
            }
            choices.push(s.to_string());
        }
    }
    Ok((q, choices))
}
