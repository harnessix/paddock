//! Wait for mailbox and detached-run outcomes without advancing a mail cursor.
use anyhow::{bail, Result};
use paddock::{
    detach,
    env::HostEnv,
    mail::{self, Dir, Kind},
};
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub(crate) struct WaitArgs {
    /// Run IDs or unique short IDs.
    #[arg(required = true, num_args = 1..)]
    pub ids: Vec<String>,
    /// Return when any run reaches the selected condition.
    #[arg(long, conflicts_with = "all")]
    pub any: bool,
    /// Wait for every run (the default).
    #[arg(long)]
    pub all: bool,
    /// Also return when the agent reports done or blocked.
    #[arg(long)]
    pub status: bool,
    /// The longest wait, for example 90s, 30m or 2h.
    #[arg(long, value_name = "DURATION")]
    pub timeout: Option<String>,
}

pub(crate) fn run(
    env: &HostEnv,
    inputs: Vec<String>,
    any: bool,
    status: bool,
    timeout: Option<String>,
) -> Result<i32> {
    let mut known = paddock::ids::known(env);
    known.extend(mail::list(env));
    let mut ids = Vec::new();
    for input in inputs {
        let id = paddock::ids::resolve_from(&input, known.iter().map(String::as_str))?;
        if !mail::exists(env, &id) && detach::Record::load(env, &id).is_none() {
            bail!("{id} has no run record or mailbox");
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let timeout = timeout
        .map(|t| paddock::timer::parse_duration(&t).map(Duration::from_secs))
        .transpose()?;
    let started = Instant::now();
    let mut results = vec![None; ids.len()];
    loop {
        for (i, id) in ids.iter().enumerate() {
            if results[i].is_none() {
                results[i] = outcome(env, id, status);
            }
        }
        if (any && results.iter().any(Option::is_some)) || results.iter().all(Option::is_some) {
            let mut code = 0;
            for (id, result) in ids.iter().zip(&results) {
                if let Some((text, c)) = result {
                    if let Some(record) = detach::Record::load(env, id) {
                        for note in record.notes {
                            eprintln!("paddock: {note}");
                        }
                    }
                    println!("{id}: {}", mail::text::clean(text).replace('\n', " ⏎ "));
                    code = code.max(*c);
                }
            }
            return Ok(code);
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            return Ok(124);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn outcome(env: &HostEnv, id: &str, status: bool) -> Option<(String, i32)> {
    let record = detach::Record::load(env, id);
    let messages = mail::read(env, id, Dir::Up);
    let latest_status = messages.iter().rev().find(|m| m.kind == Kind::Status);
    if status {
        if let Some(m) = latest_status {
            let t = m.text.trim();
            if t.starts_with("done:") {
                return Some((m.text.clone(), 0));
            }
            if t.starts_with("blocked:") {
                return Some((m.text.clone(), 1));
            }
        }
    }
    // The end of the run first, its end event after it: the paddock of a
    // run writes the event before it saves the record and before it exits.
    // In the other order, an event that is written between the two reads
    // looks like a run that ended without one.
    let record_ended = record
        .as_ref()
        .is_some_and(|r| detach::ended(env, id, Some(r), paddock::state::now()));
    mail::events::check_gone(env, id);
    let end_state = mail::inspect_end(env, id);
    match &end_state {
        mail::EndState::Conflict => {
            return Some(("conflicting end evidence; outcome unknown".into(), 2))
        }
        // A writer holds the mailbox: its end event can be on the way.
        mail::EndState::Pending => return None,
        _ => {}
    }
    let incomplete = matches!(&end_state, mail::EndState::Incomplete);
    let end = match end_state {
        mail::EndState::Complete(m) => Some(*m),
        _ => None,
    };
    // An older paddock writes the end event after the record: wait for the
    // event while the paddock of the run still runs.
    if end.is_none() && record.as_ref().is_some_and(detach::finishing) {
        return None;
    }
    if end.is_none() && !record_ended {
        if incomplete {
            return Some(("incomplete end event; outcome unknown".into(), 2));
        }
        return None;
    }
    // A view run saves its stopped flag before removing the marker. Reading
    // the marker first and then the record keeps a concurrent removal visible.
    let stopped = detach::was_stopped(env, id);
    let record = detach::Record::load(env, id).or(record);
    let messages = mail::read(env, id, Dir::Up);
    let latest_status = messages.iter().rev().find(|m| m.kind == Kind::Status);
    if let Some(m) = latest_status {
        let t = m.text.trim();
        if t.starts_with("done:") {
            return Some((m.text.clone(), 0));
        }
        if t.starts_with("blocked:") {
            return Some((m.text.clone(), 1));
        }
    }
    if let Some(d) = record.as_ref().and_then(|r| r.done.as_ref()) {
        return Some((
            format!("{}: {}", d.status.name(), d.text),
            if d.status == detach::Status::Ok { 0 } else { 1 },
        ));
    }
    if record.as_ref().is_some_and(|r| r.stopped) || stopped {
        return Some(("stopped before a final result".into(), 1));
    }
    match end {
        Some(e) => {
            let code = match e.code {
                Some(0) => 0,
                Some(_) => 1,
                None => 2,
            };
            Some((e.text, code))
        }
        None => Some(("ended without a final status".into(), 2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env_in(t: &std::path::Path) -> HostEnv {
        let mut vars = BTreeMap::new();
        for (k, d) in [
            ("XDG_RUNTIME_DIR", "run"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "cfg"),
        ] {
            vars.insert(k.to_string(), t.join(d).display().to_string());
        }
        paddock::privfs::dir(&t.join("run")).unwrap();
        HostEnv::from_parts(t.join("home"), "demo".into(), 1000, 1000, vars)
    }

    /// The pane's paddock saved its ended record, and its end event is not
    /// in the mailbox yet: wait sees a run that still ends, not one that
    /// lost its end (exit 2).
    #[test]
    fn an_ended_record_before_its_end_event_is_not_an_outcome() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(t.path());
        let id = "fake-p-7";
        mail::create(&env, id, &mail::Starter::host()).unwrap();
        let mut r = detach::Record {
            id: id.into(),
            harness: "fake".into(),
            started: 1,
            pid: Some(std::process::id()),
            ended: Some(2),
            exit_code: Some(2),
            ..Default::default()
        };
        r.save(&env).unwrap();
        assert_eq!(outcome(&env, id, false), None);
        // The event follows: its code is the outcome.
        mail::events::ended(&env, id, Some(2));
        assert_eq!(
            outcome(&env, id, false),
            Some(("the sandbox ended with exit code 2".into(), 1))
        );
        // With no paddock left to write it, a missing event is an unknown end.
        let other = "fake-p-8";
        mail::create(&env, other, &mail::Starter::host()).unwrap();
        r.id = other.into();
        r.pid = None;
        r.save(&env).unwrap();
        assert_eq!(
            outcome(&env, other, false),
            Some(("ended without a final status".into(), 2))
        );
    }

    /// The paddock of the run holds the mailbox while it writes the end
    /// event, and exits after it. A wait that meets the held mailbox and
    /// then the ended run must not report an end without its event.
    #[test]
    fn a_held_mailbox_is_an_end_on_the_way() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(t.path());
        let id = "fake-p-9";
        mail::create(&env, id, &mail::Starter::host()).unwrap();
        detach::Record {
            id: id.into(),
            harness: "fake".into(),
            started: 1,
            ended: Some(2),
            exit_code: Some(2),
            ..Default::default()
        }
        .save(&env)
        .unwrap();
        let held = mail::lock(&mail::dir(&env, id).unwrap()).unwrap();
        assert_eq!(outcome(&env, id, false), None);
        drop(held);
        mail::events::ended(&env, id, Some(2));
        assert_eq!(
            outcome(&env, id, false),
            Some(("the sandbox ended with exit code 2".into(), 1))
        );
    }
}
