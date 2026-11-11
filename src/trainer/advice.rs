//! Validate the structured result of an independent trainer.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub text: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Advice {
    /// Stable across retries of the same trainer result.
    pub id: String,
    pub severe: bool,
    pub drift: Finding,
    pub waste: Finding,
    pub risks: Finding,
    pub next_steps: Vec<Finding>,
    pub stop: Finding,
}

impl Advice {
    pub fn parse(input: &str) -> Result<Self> {
        let advice: Self = serde_json::from_str(input)?;
        if advice.id.is_empty()
            || advice.id.len() > 64
            || !advice
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            bail!("invalid trainer advice id");
        }
        if !(1..=3).contains(&advice.next_steps.len()) {
            bail!("trainer advice needs one to three next steps");
        }
        for finding in [&advice.drift, &advice.waste, &advice.risks, &advice.stop]
            .into_iter()
            .chain(advice.next_steps.iter())
        {
            if finding.text.trim().is_empty()
                || finding.text.len() > 4096
                || !finding.confidence.is_finite()
                || !(0.0..=1.0).contains(&finding.confidence)
            {
                bail!("trainer advice has an invalid finding or confidence");
            }
        }
        Ok(advice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_every_finding_and_step_count() {
        let item = serde_json::json!({"text":"look again", "confidence":0.8});
        let mut value = serde_json::json!({"id":"a-1", "severe":false, "drift":item, "waste":item, "risks":item, "next_steps":[item], "stop":item});
        assert!(Advice::parse(&value.to_string()).is_ok());
        value["next_steps"] = serde_json::json!([]);
        assert!(Advice::parse(&value.to_string()).is_err());
        value["next_steps"] = serde_json::json!([item]);
        value["risks"]["confidence"] = serde_json::json!(2);
        assert!(Advice::parse(&value.to_string()).is_err());
        value["risks"]["confidence"] = serde_json::json!(0.8);
        value["extra"] = serde_json::json!(true);
        assert!(Advice::parse(&value.to_string()).is_err());
    }
}

pub mod deliver;
