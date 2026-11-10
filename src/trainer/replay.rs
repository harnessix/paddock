//! Offline replay helpers for a recorded observation stream.
use super::{
    brief::Source,
    triggers::{Config, Signal, State},
};

/// Replay records without starting a harness or contacting a vendor.
pub fn signals(source: &Source, config: &Config) -> Vec<Signal> {
    let mut state = State::default();
    super::triggers::observe_records(&mut state, source);
    let mut out = Vec::new();
    let mut now = 0u64;
    while let Some(signal) = state.take(now, config) {
        out.push(signal);
        now = now.saturating_add(config.min_interval_ms);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::{
        advice::{deliver, Advice, Finding},
        brief::Observation,
    };
    use crate::{horse::Store, mail};

    // A deterministic fake reviewer makes the recorded issues observable
    // without contacting a model. It only reports patterns present in input.
    fn fake_review(source: &Source) -> Vec<&'static str> {
        let joined = source
            .observations
            .iter()
            .filter_map(|observation| match observation {
                Observation::Turn { text, .. } | Observation::Status(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let mut findings = Vec::new();
        for (needles, name) in [
            (["lock", "slow write"], "lock held across a slow write"),
            (["over an hour", "item"], "over an hour on one item"),
            (
                ["base64-encoded", "unread"],
                "base64-encoded failure left unread",
            ),
        ] {
            if needles.iter().all(|needle| joined.contains(needle)) {
                findings.push(name);
            }
        }
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 0,
            budget: 32,
        };
        for signal in signals(source, &cfg) {
            let finding = match signal {
                Signal::PrematureStatus { .. } => "partial done",
                Signal::RepeatedQuestion { .. } => "repeated question",
                Signal::RepeatedFailure { .. } => "repeated failing check",
                _ => continue,
            };
            findings.push(finding);
        }
        findings
    }

    #[test]
    fn synthetic_replay_names_the_known_failure_patterns_without_mutating_input(
    ) -> anyhow::Result<()> {
        let input = include_bytes!("../../tests/fixtures/trainer/synthetic-trajectory.json");
        let before = input.to_vec();
        let source = Source {
            observations: serde_json::from_slice(input)?,
            ..Default::default()
        };
        let findings = fake_review(&source);
        for expected in [
            "lock held across a slow write",
            "over an hour on one item",
            "base64-encoded failure left unread",
            "partial done",
            "repeated question",
            "repeated failing check",
        ] {
            assert!(
                findings.contains(&expected),
                "missing {expected}: {findings:?}"
            );
        }
        assert_eq!(
            findings
                .iter()
                .filter(|item| **item == "partial done")
                .count(),
            3
        );
        assert_eq!(input.as_slice(), before);
        Ok(())
    }

    // Accept observation JSON as well as sanitized Codex and Claude JSONL.
    fn recording(input: &[u8]) -> anyhow::Result<Source> {
        if let Ok(observations) = serde_json::from_slice::<Vec<Observation>>(input) {
            return Ok(Source {
                observations,
                ..Default::default()
            });
        }
        let mut source = Source::default();
        let mut pending = std::collections::HashMap::new();
        for line in std::str::from_utf8(input)?
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            let row: serde_json::Value = serde_json::from_str(line)?;
            let harness = if row.get("payload").is_some() {
                "codex"
            } else {
                "claude"
            };
            if let Some((role, text)) = super::super::records::message(harness, &row) {
                super::super::records::add_message(&mut source, role, text);
            }
            if harness == "codex" {
                if let Some(check) = super::super::records::codex_check(&row, &mut pending) {
                    source.observations.push(check);
                }
            } else {
                source
                    .observations
                    .extend(super::super::records::claude_checks(&row, &mut pending));
            }
        }
        anyhow::ensure!(
            !source.observations.is_empty(),
            "recording has no supported observations"
        );
        Ok(source)
    }

    #[test]
    fn external_replay_accepts_native_jsonl() -> anyhow::Result<()> {
        let raw = b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Which branch?\"}]}}\n";
        let source = recording(raw)?;
        assert!(source
            .observations
            .iter()
            .any(|item| matches!(item, Observation::Question(text) if text == "Which branch?")));
        Ok(())
    }

    /// Supply a sanitized observation array or native JSONL outside the repo.
    #[test]
    #[ignore = "requires an explicitly supplied external sanitized recording"]
    fn replay_external_sanitized_recording() -> anyhow::Result<()> {
        let path = std::env::var_os("PADDOCK_TRAINER_REPLAY").ok_or_else(|| {
            anyhow::anyhow!("set PADDOCK_TRAINER_REPLAY to a sanitized recording")
        })?;
        let source = recording(&std::fs::read(path)?)?;
        let _ = fake_review(&source);
        Ok(())
    }

    #[test]
    fn scripted_run_delivers_each_signal_once_with_interrupt_for_partial_done() -> anyhow::Result<()>
    {
        let mut source = Source::default();
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
        source
            .observations
            .push(Observation::Status("done: partial".into()));
        let cfg = Config {
            enabled: true,
            periodic_ms: 0,
            no_commit_ms: 0,
            min_interval_ms: 0,
            budget: 8,
        };
        let signals = signals(&source, &cfg);
        assert_eq!(signals.len(), 3);
        let temp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(temp.path());
        let store = Store::open_host(&env)?;
        mail::create(&env, "fake-run", &mail::Starter::host())?;
        for (index, signal) in signals.iter().enumerate() {
            let item = Finding {
                text: format!("{signal:?}"),
                confidence: 0.9,
            };
            let advice = Advice {
                id: format!("review-{index}"),
                severe: matches!(signal, Signal::PrematureStatus { .. }),
                drift: item.clone(),
                waste: item.clone(),
                risks: item.clone(),
                next_steps: vec![item.clone()],
                stop: item,
            };
            deliver::submit(&env, &store, "fake-run", &advice)?;
            deliver::submit(&env, &store, "fake-run", &advice)?;
        }
        let urgent = mail::read(&env, "fake-run", mail::Dir::Down);
        assert_eq!(urgent.len(), 1);
        assert!(urgent[0].interrupt);
        deliver::on_turn_end(&env, &store, "fake-run")?;
        let all = mail::read(&env, "fake-run", mail::Dir::Down);
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().filter(|message| message.interrupt).count(), 1);
        assert!(all
            .iter()
            .any(|message| message.text.contains("cargo test")));
        assert!(all
            .iter()
            .any(|message| message.text.contains("Which branch?")));
        Ok(())
    }
}
