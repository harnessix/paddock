//! Device scenarios that are sensitive to load, run
//! twice each at once in parallel threads, on any Linux machine that has
//! bubblewrap: an agent sandbox that drives its own instance, two agent
//! sandboxes on one shared instance, the shared instance under the
//! systemd user manager (when the host has one), and the 64 MiB stream
//! whose memory a process of its own measures (when the host has FUSE).
//!
//! Two scenarios of one kind share nothing but the host: each makes its
//! own home, state and runtime folder, its own unit name, its own mount.
//! A scenario that the host cannot run says so and counts as skipped.
//!
//! Both tests are load and memory measurements, so they are ignored. Run
//! them on purpose with
//! `cargo test --test devices_stress -- --ignored --nocapture`.

mod common;

use common::device_world::{can_run, fuse_works};
use common::scenarios::{
    agent_drives_instance, agent_uses_shared_instance, big_stream_in_a_child, measure_big_stream,
    MEASURE_CHILD,
};
use common::unit_scenario::unit_owns_instance;

/// The name of the measurement test, as the harness of this binary knows
/// it, for the process that runs it alone.
const STREAM_TEST: &str = "a_64_mib_stream_holds_little_memory_in_a_process_of_its_own";

/// The 64 MiB stream through a real mount holds little memory and the
/// bytes match. The resident set is of the whole process, so the test
/// starts itself again in a process of its own, and that process
/// measures.
#[test]
#[ignore = "measurement: resident memory, duration and MiB/s of a 64 MiB stream; run it on purpose with --ignored"]
fn a_64_mib_stream_holds_little_memory_in_a_process_of_its_own() {
    if std::env::var_os(MEASURE_CHILD).is_some() {
        measure_big_stream();
        return;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return;
    }
    big_stream_in_a_child(STREAM_TEST);
}

/// Each scenario twice at once. Every failure is reported, with the
/// scenario that failed, after all of them ended.
#[test]
#[ignore = "load test: every device scenario twice at once, with the 64 MiB memory measurement; run it on purpose with --ignored"]
fn the_device_scenarios_run_twice_at_once() {
    if !can_run() {
        return;
    }
    let fuse = fuse_works();
    let systemd = common::systemd_user();
    if !fuse {
        eprintln!("no FUSE on this host: the sandbox and stream scenarios skip");
    }
    if !systemd {
        eprintln!("no systemd user instance: the unit scenario skips");
    }
    type Job = (&'static str, Box<dyn FnOnce() -> bool + Send>);
    let mut jobs: Vec<Job> = Vec::new();
    for _ in 0..2 {
        if fuse {
            jobs.push((
                "an agent sandbox drives its instance",
                Box::new(agent_drives_instance),
            ));
            jobs.push((
                "two agent sandboxes use the shared instance",
                Box::new(agent_uses_shared_instance),
            ));
            jobs.push((
                "the 64 MiB stream in a process of its own",
                Box::new(|| big_stream_in_a_child(STREAM_TEST)),
            ));
        }
        if systemd {
            jobs.push((
                "the unit owns the shared instance",
                Box::new(unit_owns_instance),
            ));
        }
    }
    if jobs.is_empty() {
        eprintln!("skipped: nothing of this runs here");
        return;
    }
    let started = std::time::Instant::now();
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|(name, f)| (name, std::thread::spawn(f)))
        .collect();
    let mut ran = 0;
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    for (name, h) in handles {
        match h.join() {
            Ok(true) => ran += 1,
            Ok(false) => skipped.push(name),
            Err(p) => failed.push(format!("{name}: {}", panic_text(&p))),
        }
    }
    eprintln!(
        "{ran} scenarios ran at once in {:?}; skipped: {skipped:?}",
        started.elapsed()
    );
    assert!(
        failed.is_empty(),
        "{} of the scenarios failed:\n\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}

/// The message of a panic.
fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = p.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    "a panic with no message".to_string()
}
