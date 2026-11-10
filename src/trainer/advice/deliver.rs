//! Persist normal advice until a turn-end notification, and deliver severe
//! advice immediately. The mailbox log is the durable idempotency record.
use super::Advice;
use crate::{env::HostEnv, horse::Store, mail};
use anyhow::{bail, Context as _, Result};
use std::os::fd::AsRawFd as _;

fn lock(env: &HostEnv, run: &str) -> Result<std::fs::File> {
    let directory = mail::dir(env, run)?;
    let file = crate::privfs::append(&directory.join("trainer-advice.lock"))?;
    // SAFETY: file owns this descriptor for the life of the guard.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

fn path(run: &str, id: &str) -> String {
    format!("trainer/pending/{run}/{id}.json")
}

fn already_sent(env: &HostEnv, run: &str, advice: &Advice) -> Result<Option<mail::Message>> {
    for message in mail::read(env, run, mail::Dir::Down)
        .into_iter()
        .filter(|m| m.from == "trainer")
    {
        let Ok(old) = Advice::parse(&message.text) else {
            continue;
        };
        if old.id == advice.id {
            if old != *advice {
                bail!("trainer advice id is already used by different advice");
            }
            return Ok(Some(message));
        }
    }
    Ok(None)
}

fn send(env: &HostEnv, run: &str, advice: &Advice) -> Result<mail::Message> {
    if let Some(message) = already_sent(env, run, advice)? {
        return Ok(message);
    }
    let mut message =
        mail::Message::new("trainer", mail::Kind::Say, serde_json::to_string(advice)?);
    message.interrupt = advice.severe;
    mail::send_down(env, run, message).map_err(|error| anyhow::anyhow!("{error}"))
}

/// Queue exactly one advice item. A normal item is durable but is not put
/// in the mailbox until the target reports a turn end.
pub fn submit(env: &HostEnv, store: &Store, run: &str, advice: &Advice) -> Result<()> {
    submit_for(env, store, run, advice, "unspecified")
}

pub fn submit_for(
    env: &HostEnv,
    store: &Store,
    run: &str,
    advice: &Advice,
    reason: &str,
) -> Result<()> {
    let advice = Advice::parse(&serde_json::to_string(advice)?)?;
    if !mail::exists(env, run) {
        bail!("target has no mailbox");
    }
    if mail::end(env, run).is_some() {
        bail!("target run ended");
    }
    let _guard = lock(env, run)?;
    if already_sent(env, run, &advice)?.is_some() {
        return Ok(());
    }
    let key = path(run, &advice.id);
    if let Some(old) = store.read_ref(&key)? {
        let old: Advice = serde_json::from_slice(&old)?;
        if old != advice {
            bail!("trainer advice id is already pending with different advice");
        }
        return Ok(());
    }
    super::super::learning::record(env, store, run, &advice, reason)?;
    if advice.severe {
        send(env, run, &advice)?;
    } else {
        store.write_ref(&key, &serde_json::to_vec(&advice)?)?;
    }
    Ok(())
}

/// Horse custody writes human-oriented progress to captured stdout. Read the
/// final assistant message from the trainer's own independent session instead.
fn session_advice(source: &super::super::brief::Source) -> Result<Advice> {
    let text = source
        .observations
        .iter()
        .rev()
        .find_map(|observation| match observation {
            super::super::brief::Observation::Turn { role, text } if role == "assistant" => {
                Some(text.as_str())
            }
            _ => None,
        })
        .context("trainer session has no assistant result")?;
    Advice::parse(text.trim())
}

/// Read the successful trainer's result from its private output folder.
pub fn from_result(
    env: &HostEnv,
    store: &Store,
    run: &str,
    scratch: &std::path::Path,
    status: &crate::spawn::Status,
    reason: &str,
) -> Result<()> {
    if status.state != crate::spawn::State::Exited || status.exit_code != Some(0) {
        bail!("trainer did not exit successfully");
    }
    let relative = status
        .inbox
        .as_deref()
        .context("trainer has no result folder")?;
    let root = crate::safefs::Root::open(scratch)?;
    let directory = std::path::Path::new(relative);
    let summary = root
        .read(&directory.join("result.json"), 64 * 1024)?
        .context("trainer returned no result metadata")?;
    let summary: serde_json::Value = serde_json::from_slice(&summary)?;
    if summary["sandbox"].as_str() != Some(status.sandbox.as_str())
        || summary["output_truncated"].as_bool() != Some(false)
    {
        bail!("trainer result is truncated or does not match its sandbox");
    }
    let output = root
        .read(
            &directory.join("output.txt"),
            crate::spawn::MAX_OUTPUT as u64,
        )?
        .context("trainer returned no output")?;
    let text = std::str::from_utf8(&output).context("trainer output is not UTF-8")?;
    let advice = match Advice::parse(text.trim()) {
        Ok(advice) => advice,
        Err(_) => {
            let source = super::super::records::read(env, store, &status.sandbox, None)
                .context("cannot read the trainer's independent session")?;
            session_advice(&source).context("trainer did not return valid structured advice")?
        }
    };
    if advice.id != status.sandbox {
        bail!("trainer returned the wrong advice id");
    }
    submit_for(env, store, run, &advice, reason)
}

/// Flush only if this run has pending advice; a non-trainer run does not
/// create or initialize a horse store merely by reaching a stop hook.
pub fn flush_if_pending(env: &HostEnv, run: &str) -> Result<usize> {
    mail::dir(env, run)?;
    if !crate::horse::root(env)
        .join("trainer/pending")
        .join(run)
        .is_dir()
    {
        return Ok(0);
    }
    let store = Store::open_host(env)?;
    on_turn_end(env, &store, run)
}

/// Called when the target actually reaches its turn end, never merely on a
/// tool hook or an interrupt request.
pub fn on_turn_end(env: &HostEnv, store: &Store, run: &str) -> Result<usize> {
    let _guard = lock(env, run)?;
    let directory = format!("trainer/pending/{run}");
    if !store.root().join(&directory).is_dir() {
        return Ok(0);
    }
    let mut sent = 0;
    for name in store.list_refs(&directory)? {
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let key = path(run, id);
        let Some(raw) = store.read_ref(&key)? else {
            continue;
        };
        let advice =
            Advice::parse(std::str::from_utf8(&raw).context("pending advice is not UTF-8")?)?;
        if advice.id != id || advice.severe {
            bail!("invalid pending trainer advice");
        }
        send(env, run, &advice)?;
        store.remove_ref(&key)?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::advice::Finding;

    fn advice(id: &str, severe: bool) -> Advice {
        let finding = Finding {
            text: "check the evidence".into(),
            confidence: 0.8,
        };
        Advice {
            id: id.into(),
            severe,
            drift: finding.clone(),
            waste: finding.clone(),
            risks: finding.clone(),
            next_steps: vec![finding.clone()],
            stop: finding,
        }
    }

    #[test]
    fn extracts_only_the_final_assistant_result_from_the_trainers_session() -> Result<()> {
        use crate::trainer::brief::{Observation, Source};
        let expected = advice("trainer-one", true);
        let mut source = Source::default();
        source.observations.push(Observation::Turn {
            role: "user".into(),
            text: serde_json::to_string(&expected)?,
        });
        source.observations.push(Observation::Turn {
            role: "assistant".into(),
            text: "working".into(),
        });
        assert!(session_advice(&source).is_err());
        source.observations.push(Observation::Turn {
            role: "assistant".into(),
            text: serde_json::to_string(&expected)?,
        });
        assert_eq!(session_advice(&source)?, expected);
        source.observations.push(Observation::Turn {
            role: "assistant".into(),
            text: "invalid last message".into(),
        });
        assert!(session_advice(&source).is_err());
        Ok(())
    }

    #[test]
    fn successful_trainer_output_must_match_the_sandbox_before_delivery() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(tmp.path());
        let store = Store::open_host(&env)?;
        mail::create(&env, "test-run", &mail::Starter::host())?;
        let scratch = tmp.path().join("scratch");
        let out = scratch.join(".paddock/inbox/spawn-1");
        std::fs::create_dir_all(&out)?;
        let status: crate::spawn::Status = serde_json::from_value(
            serde_json::json!({"id":"1","sandbox":"trainer-one","harness":"fake","branch":"unused","state":"exited","started":1,"exit_code":0,"inbox":".paddock/inbox/spawn-1"}),
        )?;
        std::fs::write(
            out.join("result.json"),
            serde_json::to_vec(
                &serde_json::json!({"sandbox":"trainer-one","output_truncated":false}),
            )?,
        )?;
        std::fs::write(
            out.join("output.txt"),
            serde_json::to_vec(&advice("wrong", true))?,
        )?;
        assert!(from_result(&env, &store, "test-run", &scratch, &status, "test").is_err());
        std::fs::write(
            out.join("output.txt"),
            serde_json::to_vec(&advice("trainer-one", true))?,
        )?;
        from_result(&env, &store, "test-run", &scratch, &status, "test")?;
        from_result(&env, &store, "test-run", &scratch, &status, "test")?;
        let messages = mail::read(&env, "test-run", mail::Dir::Down);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].interrupt);
        Ok(())
    }

    #[test]
    fn normal_waits_for_turn_end_even_when_severe_advice_interrupts() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(tmp.path());
        let store = Store::open_host(&env)?;
        mail::create(&env, "test-run", &mail::Starter::host())?;
        let normal = advice("normal-1", false);
        let urgent = advice("urgent-1", true);
        submit(&env, &store, "test-run", &normal)?;
        submit(&env, &store, "test-run", &normal)?;
        assert!(mail::read(&env, "test-run", mail::Dir::Down).is_empty());
        submit(&env, &store, "test-run", &urgent)?;
        submit(&env, &store, "test-run", &urgent)?;
        let before = mail::read(&env, "test-run", mail::Dir::Down);
        assert_eq!(before.len(), 1);
        assert!(before[0].interrupt);
        let tool = crate::mail::deliver::take(
            &env,
            "test-run",
            crate::mail::deliver::Via::Tool,
            64 * 1024,
        )?;
        assert_eq!(tool.messages.len(), 1);
        assert!(tool.messages[0].interrupt);
        let stop = crate::mail::deliver::take(
            &env,
            "test-run",
            crate::mail::deliver::Via::Stop,
            64 * 1024,
        )?;
        assert_eq!(stop.messages.len(), 1);
        assert!(!stop.messages[0].interrupt);
        assert_eq!(on_turn_end(&env, &store, "test-run")?, 0);
        let after = mail::read(&env, "test-run", mail::Dir::Down);
        assert_eq!(after.len(), 2);
        assert!(!after[1].interrupt);
        assert_eq!(Advice::parse(&after[1].text)?.id, "normal-1");
        Ok(())
    }
}
