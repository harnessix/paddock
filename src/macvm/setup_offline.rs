//! The offline states of `paddock macvm setup`: the offline seed in
//! `Seed` (`offline.rs`) and the offline trust in `Trust` (`tcc.rs`).
//! Each falls back to the person in the guided window on any error, and
//! each writes its outcome to the event log of the golden. The trust also
//! makes the `~/Downloads` link of the stopped guest (`offline_link.rs`);
//! `Configure` makes it first when no earlier run did.

use super::diskimage::Fsck;
use super::golden::Outcome;
use super::layout::{Golden, Layout};
use super::provision::{self, Route};
use super::setup::{agent_binary, end_to_outcome, Backend, Opts, Ui};
use super::text;
use super::{offline, offline_link, seed, setup_events, tcc};
use anyhow::{anyhow, bail, Result};

/// `Seed`: the seed folder in the golden folder, as the user, then the
/// offline seed of the disk when the backend can mount it. A failed
/// offline seed leaves the disk untouched and the state done: the guided
/// window shows the typed command.
pub(super) fn seed_state(
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    ui: &mut Ui,
) -> Outcome {
    let agent = agent_binary(layout, opts);
    if agent.is_none() {
        ui.line(&text::agent_missing("seed"));
    }
    if let Err(e) = seed::prepare(&golden.seed, agent.as_deref()) {
        return Outcome::Failed(format!("{e:#}"));
    }
    let line = text::seed_prepared(&golden.seed, agent.is_some());
    setup_events::record(&golden.events, setup_events::SEED_PREPARED, false, &line);
    ui.line(&line);
    let tool = if opts.guided {
        Err(text::offline_off_by_flag())
    } else {
        backend.disk_tool().ok_or_else(text::offline_no_tool)
    };
    let mut tool = match tool {
        Ok(t) => t,
        Err(why) => {
            let line = format!("skipped: {why}"); // not user text
            setup_events::record(&golden.events, setup_events::SEED_OFFLINE, false, &line);
            ui.line(&text::seed_offline_skipped(&why));
            return Outcome::Done;
        }
    };
    let files = match offline::files(&golden.seed) {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    match offline::seed_disk(tool.as_mut(), &golden.disk, &golden.dir.join("mnt"), &files) {
        Ok(report) => {
            let fsck = match &report.fsck {
                Fsck::Clean => "fsck clean".to_string(),
                Fsck::Skipped(why) => format!("fsck skipped: {why}"), // not user text
            };
            let line = format!("done: {} files, {fsck}", report.owned.len()); // not user text
            setup_events::record(&golden.events, setup_events::SEED_OFFLINE, false, &line);
            ui.line(&text::seed_offline_done(report.owned.len(), &report.fsck));
            Outcome::Done
        }
        Err(e) => {
            let why = format!("{e:#}");
            // The clone is still there only when it could not be put
            // back: the disk is not known to be whole, so stop.
            if offline::backup_of(&golden.disk).exists() {
                return Outcome::Failed(why);
            }
            let line = format!("failed: {why}"); // not user text
            setup_events::record(&golden.events, setup_events::SEED_OFFLINE, false, &line);
            ui.line(&text::seed_offline_failed(&why));
            Outcome::Done
        }
    }
}

/// `Trust`: a golden whose agent reported the permission in the guided
/// window passes. Else the guest stops, the TCC database gets the row,
/// and the guest boots again until the agent reports the permission. Any
/// failure shows the guided window, where the person turns the switch on.
pub(super) fn trust(
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    password: &str,
    ui: &mut Ui,
) -> Outcome {
    let events = setup_events::read(&golden.events);
    if events.iter().any(|l| l.kind == setup_events::TRUSTED) {
        return Outcome::Done;
    }
    let why = match trust_offline(layout, golden, backend, opts, password, ui) {
        Ok(patched) => {
            let word = match patched {
                tcc::Patched::Already => "already",
                tcc::Patched::Updated { .. } => "updated",
                tcc::Patched::Inserted => "inserted",
            };
            setup_events::record(&golden.events, setup_events::TRUST_OFFLINE, false, word);
            ui.line(&text::trust_offline_done());
            match backend.trusted(golden, password) {
                Ok(Some(true)) => {
                    setup_events::record_once(&golden.events, setup_events::TRUSTED, "offline");
                    ui.line(&text::trust_reported());
                    return Outcome::Done;
                }
                Ok(Some(false)) => text::trust_not_reported(),
                Ok(None) => return Outcome::UserStopped,
                Err(e) => format!("{e:#}"),
            }
        }
        Err(e) => {
            let why = format!("{e:#}");
            let line = format!("failed: {why}"); // not user text
            setup_events::record(&golden.events, setup_events::TRUST_OFFLINE, false, &line);
            why
        }
    };
    ui.line(&text::trust_offline_failed(&why));
    // The person turns the switch on: the window ends when the agent
    // reports the permission.
    let route = Route {
        seeded: true,
        ..Route::guided(provision::Why::Forced)
    };
    end_to_outcome(backend.guided(golden, password, &route))
}

/// Stop the guest and set the row offline.
fn trust_offline(
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    password: &str,
    ui: &mut Ui,
) -> Result<tcc::Patched> {
    if opts.guided {
        bail!(text::offline_off_by_flag());
    }
    let mut tool = backend
        .disk_tool()
        .ok_or_else(|| anyhow!(text::offline_no_tool()))?;
    backend.shutdown(golden, password)?;
    let agent = agent_binary(layout, opts);
    let req = backend.agent_requirement(agent.as_deref());
    let patched = tcc::trust_disk(
        tool.as_mut(),
        &golden.disk,
        &golden.dir.join("mnt"),
        move || req,
    );
    // The guest is stopped already: the link costs one more attach, and
    // its failure is its own event, never a failure of the trust.
    let linked = offline_link::link_disk(tool.as_mut(), &golden.disk, &golden.dir.join("mnt"));
    record_link(golden, linked, ui);
    patched
}

/// Whether the event log says that the `~/Downloads` link is there.
pub(super) fn linked(golden: &Golden) -> bool {
    setup_events::read(&golden.events)
        .iter()
        .rev()
        .find(|l| l.kind == setup_events::DOWNLOADS_OFFLINE)
        .is_some_and(|l| {
            [offline_link::Linked::Made, offline_link::Linked::Already]
                .iter()
                .any(|w| l.text == w.word())
        })
}

/// The event and the terminal line of an offline link.
fn record_link(golden: &Golden, linked: Result<offline_link::Linked>, ui: &mut Ui) {
    match linked {
        Ok(l) => {
            setup_events::record(
                &golden.events,
                setup_events::DOWNLOADS_OFFLINE,
                false,
                l.word(),
            );
            ui.line(&text::downloads_offline_done(
                l == offline_link::Linked::Made,
            ));
        }
        Err(e) => {
            let why = format!("{e:#}");
            let line = format!("failed: {why}"); // not user text
            setup_events::record(
                &golden.events,
                setup_events::DOWNLOADS_OFFLINE,
                false,
                &line,
            );
            ui.line(&text::downloads_offline_failed(&why));
        }
    }
}

/// Before `Configure`: the `~/Downloads` link, when no earlier run made
/// it (a golden trusted in the window, or by an older paddock). The guest
/// stops, the link goes onto the disk, and the configure request boots
/// the guest again. A failure is an event and a line: the configure step
/// `downloads-link` then fails and names the link.
pub(super) fn downloads_before_configure(
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    password: &str,
    ui: &mut Ui,
) {
    if linked(golden) {
        return;
    }
    let tool = if opts.guided {
        Err(text::offline_off_by_flag())
    } else {
        backend.disk_tool().ok_or_else(text::offline_no_tool)
    };
    let mut tool = match tool {
        Ok(t) => t,
        Err(why) => {
            let line = format!("skipped: {why}"); // not user text
            setup_events::record(
                &golden.events,
                setup_events::DOWNLOADS_OFFLINE,
                false,
                &line,
            );
            ui.line(&text::downloads_offline_failed(&why));
            return;
        }
    };
    let linked = backend.shutdown(golden, password).and_then(|()| {
        offline_link::link_disk(tool.as_mut(), &golden.disk, &golden.dir.join("mnt"))
    });
    record_link(golden, linked, ui);
}
