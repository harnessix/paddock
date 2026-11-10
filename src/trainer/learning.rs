//! Encrypted review outcomes. Adoption is recorded only from explicit feedback.
use super::{advice::Advice, brief::Observation};
use crate::{env::HostEnv, horse::Store};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::fd::AsRawFd as _;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Adoption {
    Unknown,
    Adopted,
    NotAdopted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub advice: Advice,
    pub created_ms: u64,
    pub baseline: Option<BTreeMap<String, usize>>,
    pub reason: String,
    pub adoption: Adoption,
    pub feedback: Option<String>,
    pub followed: Vec<Observation>,
    pub elapsed_to_done_ms: Option<u64>,
}

fn key(run: &str, id: &str) -> Result<String> {
    if !crate::view::valid_id(run) || !crate::view::valid_id(id) {
        bail!("invalid run or advice id");
    }
    Ok(format!("trainer/learning/{run}/{id}.json"))
}

fn lock(store: &Store, run: &str) -> Result<std::fs::File> {
    if !crate::view::valid_id(run) {
        bail!("invalid run id");
    }
    let directory = store.root().join("trainer/learning-locks");
    crate::privfs::dir(&directory)?;
    let file = crate::privfs::append(&directory.join(format!("{run}.lock")))?;
    // SAFETY: the file owns its descriptor until the guard is dropped.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

fn digest(observation: &Observation) -> Option<String> {
    use sha2::{Digest as _, Sha256};
    serde_json::to_vec(observation)
        .ok()
        .map(|raw| crate::manifest::hex(&Sha256::digest(raw)))
}
fn counts(observations: &[Observation]) -> BTreeMap<String, usize> {
    let mut result = BTreeMap::new();
    for observation in observations {
        if let Some(key) = digest(observation) {
            *result.entry(key).or_default() += 1;
        }
    }
    result
}
fn after(baseline: &BTreeMap<String, usize>, observations: &[Observation]) -> Vec<Observation> {
    let mut remaining = baseline.clone();
    let mut result = Vec::new();
    for observation in observations {
        let Some(key) = digest(observation) else {
            continue;
        };
        if let Some(count) = remaining.get_mut(&key) {
            if *count > 0 {
                *count -= 1;
                continue;
            }
        }
        if matches!(
            observation,
            Observation::Check { .. } | Observation::Commit(_) | Observation::Status(_)
        ) && result.len() < 256
        {
            result.push(observation.clone());
        }
    }
    result
}

/// Save the advice at delivery, along with a boundary for subsequent facts.
pub fn record(
    env: &HostEnv,
    store: &Store,
    run: &str,
    advice: &Advice,
    reason: &str,
) -> Result<()> {
    let path = key(run, &advice.id)?;
    let _guard = lock(store, run)?;
    if store.read_ref(&path)?.is_some() {
        return Ok(());
    }
    let baseline = super::records::read(env, store, run, None)
        .ok()
        .map(|source| counts(&source.observations));
    let record = Record {
        advice: advice.clone(),
        created_ms: crate::horse::now_ms(),
        baseline,
        reason: reason.to_owned(),
        adoption: Adoption::Unknown,
        feedback: None,
        followed: Vec::new(),
        elapsed_to_done_ms: None,
    };
    store.write_ref(&path, &serde_json::to_vec(&record)?)
}

/// Explicit feedback avoids inferring a worker's intent from loose text matches.
pub fn feedback(store: &Store, run: &str, id: &str, adopted: bool, note: &str) -> Result<()> {
    if note.len() > 4096 {
        bail!("feedback is too long");
    }
    let path = key(run, id)?;
    let _guard = lock(store, run)?;
    let bytes = store.read_ref(&path)?.context("no such trainer advice")?;
    let mut record: Record = serde_json::from_slice(&bytes)?;
    record.adoption = if adopted {
        Adoption::Adopted
    } else {
        Adoption::NotAdopted
    };
    record.feedback = (!note.is_empty()).then(|| note.to_owned());
    store.write_ref(&path, &serde_json::to_vec(&record)?)
}

/// A trigger is reduced only after repeated explicit negative feedback.
pub fn unhelpful(store: &Store, run: &str) -> Result<std::collections::BTreeSet<String>> {
    if !crate::view::valid_id(run) {
        bail!("invalid run id");
    }
    let directory = format!("trainer/learning/{run}");
    if !store.root().join(&directory).exists() {
        return Ok(Default::default());
    }
    let mut totals = BTreeMap::<String, (usize, usize)>::new();
    for name in store.list_refs(&directory)? {
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let Some(bytes) = store.read_ref(&key(run, id)?)? else {
            continue;
        };
        let record: Record = serde_json::from_slice(&bytes)?;
        let count = totals.entry(record.reason).or_default();
        match record.adoption {
            Adoption::Adopted => count.0 += 1,
            Adoption::NotAdopted => count.1 += 1,
            Adoption::Unknown => {}
        }
    }
    Ok(totals
        .into_iter()
        .filter_map(|(reason, (positive, negative))| {
            (negative >= 3 && negative > positive).then_some(reason)
        })
        .collect())
}

pub fn stats(env: &HostEnv, store: &Store, run: &str) -> Result<Vec<Record>> {
    if !crate::view::valid_id(run) {
        bail!("invalid run id");
    }
    let directory = format!("trainer/learning/{run}");
    if !store.root().join(&directory).exists() {
        return Ok(Vec::new());
    }
    let _guard = lock(store, run)?;
    let source = super::records::read(env, store, run, None).ok();
    let messages = crate::mail::read(env, run, crate::mail::Dir::Up);
    let mut result = Vec::new();
    for name in store.list_refs(&directory)? {
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let Some(bytes) = store.read_ref(&key(run, id)?)? else {
            continue;
        };
        let mut record: Record = serde_json::from_slice(&bytes)?;
        if let (Some(source), Some(baseline)) = (&source, &record.baseline) {
            record.followed = after(baseline, &source.observations);
        }
        if let Some(elapsed) = messages
            .iter()
            .find(|message| {
                message.kind == crate::mail::Kind::Status
                    && message.ts >= record.created_ms / 1000
                    && message.text.trim().to_ascii_lowercase().starts_with("done")
            })
            .map(|message| {
                message
                    .ts
                    .saturating_mul(1000)
                    .saturating_sub(record.created_ms)
            })
        {
            record.elapsed_to_done_ms = Some(elapsed);
        }
        store.write_ref(&key(run, id)?, &serde_json::to_vec(&record)?)?;
        result.push(record);
    }
    result.sort_by_key(|record| record.created_ms);
    Ok(result)
}

/// Read all runs with trainer learning records.
pub fn all_stats(env: &HostEnv, store: &Store) -> Result<BTreeMap<String, Vec<Record>>> {
    let root = store.root().join("trainer/learning");
    let mut output = BTreeMap::new();
    if !root.exists() {
        return Ok(output);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(run) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if crate::view::valid_id(&run) {
            output.insert(run.clone(), stats(env, store, &run)?);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::advice::Finding;

    #[test]
    fn learning_keeps_unknown_distinct_from_feedback_and_tracks_later_facts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(temp.path());
        let store = Store::open_host(&env)?;
        let item = Finding {
            text: "retry the check".into(),
            confidence: 0.8,
        };
        let advice = Advice {
            id: "review-1".into(),
            severe: false,
            drift: item.clone(),
            waste: item.clone(),
            risks: item.clone(),
            next_steps: vec![item.clone()],
            stop: item,
        };
        record(&env, &store, "run-1", &advice, "test")?;
        let records = stats(&env, &store, "run-1")?;
        assert_eq!(records[0].adoption, Adoption::Unknown);
        feedback(&store, "run-1", "review-1", true, "ran it")?;
        record(&env, &store, "run-1", &advice, "test")?;
        let records = stats(&env, &store, "run-1")?;
        assert_eq!(records[0].adoption, Adoption::Adopted);
        assert_eq!(records[0].feedback.as_deref(), Some("ran it"));
        for i in 0..3 {
            let mut next = advice.clone();
            next.id = format!("review-negative-{i}");
            record(&env, &store, "run-1", &next, "repeated question")?;
            feedback(&store, "run-1", &next.id, false, "")?;
        }
        assert!(unhelpful(&store, "run-1")?.contains("repeated question"));
        assert_eq!(all_stats(&env, &store)?["run-1"].len(), 4);

        let before = vec![
            Observation::Status("working".into()),
            Observation::Check {
                name: "test".into(),
                passed: Some(false),
            },
        ];
        let later = vec![
            Observation::Check {
                name: "test".into(),
                passed: Some(false),
            },
            Observation::Check {
                name: "test".into(),
                passed: Some(true),
            },
            Observation::Status("working".into()),
        ];
        let followed = after(&counts(&before), &later);
        assert!(matches!(
            &followed[..],
            [Observation::Check {
                passed: Some(true),
                ..
            }]
        ));
        Ok(())
    }
}
