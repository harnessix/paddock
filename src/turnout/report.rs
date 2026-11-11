//! One health record per wake and one mailbox report for admitted work.
use crate::{env::HostEnv, mail};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Row {
    pub scheduled: Option<u64>,
    pub started: u64,
    pub finished: u64,
    pub session: Option<String>,
    pub compactions: u64,
    pub custody_events: u64,
    pub mail_in: u64,
    pub mail_out: Option<u64>,
    pub model_turns: u64,
    pub stop_reason: String,
    pub error: Option<String>,
}

pub fn path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.jsonl"))
}

pub fn append(dir: &Path, name: &str, row: &Row) -> Result<()> {
    crate::privfs::dir(dir)?;
    let mut f = crate::privfs::append(&path(dir, name))?;
    let mut line = serde_json::to_vec(row)?;
    line.push(b'\n');
    f.write_all(&line)?;
    Ok(())
}

pub fn read(dir: &Path, name: &str) -> Result<Vec<Row>> {
    let text = match std::fs::read_to_string(path(dir, name)) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    text.lines()
        .map(|line| serde_json::from_str(line).context("invalid turnout health row"))
        .collect()
}

pub fn send(env: &HostEnv, name: &str, job: &super::Job, text: &str) -> Result<u64> {
    let default = format!("turnout-{name}");
    let mailbox = job.mailbox.as_deref().unwrap_or(&default);
    mail::create(env, mailbox, &mail::Starter::host())?;
    let msg = mail::Message::new("turnout", mail::Kind::Say, text);
    Ok(mail::append(env, mailbox, mail::Dir::Up, msg)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .seq)
}

/// Record every wake; send mail only if the local check admitted work.
pub fn wake(
    env: &HostEnv,
    name: &str,
    job: &super::Job,
    now: u64,
    load: f64,
    work: impl FnOnce(Option<&str>, &super::pause::Generation) -> Result<String>,
) -> Result<()> {
    wake_with_retry(env, name, job, (now, load), || Ok(None), work)
}

pub fn wake_with_retry(
    env: &HostEnv,
    name: &str,
    job: &super::Job,
    initial: (u64, f64),
    retry: impl FnMut() -> Result<Option<(u64, f64)>>,
    work: impl FnOnce(Option<&str>, &super::pause::Generation) -> Result<String>,
) -> Result<()> {
    let now = initial.0;
    let dir = env.paddock_state().join("turnout");
    let admitted = std::cell::Cell::new(false);
    let result =
        super::wake::run_with_retry(&dir, name, job, initial, retry, |head, generation| {
            admitted.set(true);
            work(head, generation)
        });
    let (reason, error, body) = match &result {
        Ok(Some(body)) => ("completed", None, Some(format!("Turnout {name}: {body}"))),
        Ok(None) => ("not admitted", None, None),
        Err(e) if admitted.get() => (
            "failed",
            Some(format!("{e:#}")),
            Some(format!("Turnout {name} failed: {e:#}")),
        ),
        Err(e) => ("failed before work", Some(format!("{e:#}")), None),
    };
    let mut mail_error = None;
    let seq = body
        .as_ref()
        .and_then(|body| match send(env, name, job, body) {
            Ok(seq) => Some(seq),
            Err(e) => {
                mail_error = Some(format!("{e:#}"));
                None
            }
        });
    let row = Row {
        scheduled: None,
        started: now,
        finished: crate::state::now(),
        session: None,
        compactions: 0,
        custody_events: 0,
        mail_in: 0,
        mail_out: seq,
        model_turns: 0,
        stop_reason: reason.into(),
        error: error.or_else(|| mail_error.clone()),
    };
    append(&dir, name, &row)?;
    if let Some(e) = mail_error {
        anyhow::bail!("cannot send turnout report: {e}");
    }
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn reports_and_health_rows_are_recorded() {
        let d = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(d.path().to_owned(), "test".into(), 1, 1, BTreeMap::new());
        let job = super::super::parse(
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n",
        )
        .unwrap()
        .remove("x")
        .unwrap();
        let seq = send(&env, "x", &job, "completed").unwrap();
        assert_eq!(mail::read(&env, "turnout-x", mail::Dir::Up).len(), 1);
        let row = Row {
            scheduled: Some(1),
            started: 1,
            finished: 2,
            session: None,
            compactions: 0,
            custody_events: 0,
            mail_in: 0,
            mail_out: Some(seq),
            model_turns: 0,
            stop_reason: "completed".into(),
            error: None,
        };
        let dir = d.path().join("log");
        append(&dir, "x", &row).unwrap();
        assert_eq!(read(&dir, "x").unwrap()[0].mail_out, Some(seq));
    }

    #[test]
    fn failed_checks_are_silent_but_logged_and_work_is_mailed_once() {
        let d = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(d.path().to_owned(), "test".into(), 1, 1, BTreeMap::new());
        let mut job = super::super::parse(
            "[turnout.x]\nharness='none'\ncheck='exit 1'\ntrigger='timer'\nschedule='daily'\n",
        )
        .unwrap()
        .remove("x")
        .unwrap();
        wake(&env, "x", &job, 1, 0.0, |_, _| panic!("failed check")).unwrap();
        assert!(!mail::exists(&env, "turnout-x"));
        job.check = "echo work".into();
        wake(&env, "x", &job, 2, 0.0, |_, _| Ok("done".into())).unwrap();
        assert_eq!(mail::read(&env, "turnout-x", mail::Dir::Up).len(), 1);
        let rows = read(&env.paddock_state().join("turnout"), "x").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].stop_reason, "not admitted");
        assert_eq!(rows[1].stop_reason, "completed");
    }

    #[test]
    fn a_fake_budget_timeout_is_mailed() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(d.path().to_owned(), "test".into(), 1, 1, BTreeMap::new());
        let run = d.path().join("systemd-run");
        let ctl = d.path().join("systemctl");
        std::fs::write(&run, "#!/bin/sh\ntimeout 0.1s sleep 10\n").unwrap();
        std::fs::write(
            &ctl,
            "#!/bin/sh\ncase \"$*\" in *show*) echo timeout;; esac\n",
        )
        .unwrap();
        for path in [&run, &ctl] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let job = super::super::parse(
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n",
        )
        .unwrap()
        .remove("x")
        .unwrap();
        assert!(wake(&env, "x", &job, 1, 0.0, |_, _| {
            super::super::budget::run(
                &run,
                &ctl,
                &|| false,
                Path::new("/fake/paddock"),
                "x",
                &job,
                &[],
            )?;
            Ok("done".into())
        })
        .is_err());
        let messages = mail::read(&env, "turnout-x", mail::Dir::Up);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].text.contains("timeout"));
        assert_eq!(
            read(&env.paddock_state().join("turnout"), "x").unwrap()[0].stop_reason,
            "failed"
        );
    }
}
