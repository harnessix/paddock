//! Observe local run records and request reviews without polling a vendor.
use crate::{env::HostEnv, horse::Store};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct Monitor {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let (lock, wake) = &*self.stop;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Return no monitor for a trainer itself, a disabled configuration, or a
/// run whose private store cannot be opened. The normal run stays usable.
pub fn start(env: &HostEnv, run: &str) -> Option<Monitor> {
    let cfg = match super::triggers::Config::load(env) {
        Ok(cfg) if cfg.enabled => cfg,
        Ok(_) => return None,
        Err(error) => {
            log::warn!("trainer monitor disabled: {error:#}");
            return None;
        }
    };
    let store = Store::open_host(env).ok()?;
    match crate::horse::device::peek::grant::has_trainer(&store, run) {
        Ok(false) => {}
        _ => return None,
    }
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let shared = stop.clone();
    let run = run.to_owned();
    let env = env.clone();
    let thread = std::thread::spawn(move || {
        let mut last = Instant::now();
        let running = Arc::new(AtomicBool::new(false));
        let mut last_head = None;
        let mut last_mail = 0;
        let mut was_active = false;
        let mut source = super::brief::Source::default();
        loop {
            let (lock, wake) = &*shared;
            let stopped = lock.lock().unwrap();
            let (stopped, _) = wake
                .wait_timeout_while(stopped, Duration::from_secs(10), |stopped| !*stopped)
                .unwrap();
            if *stopped {
                break;
            }
            drop(stopped);
            let elapsed = last.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            last = Instant::now();
            if crate::mail::end(&env, &run).is_some() {
                let _ = super::learning::stats(&env, &store, &run);
                break;
            }
            let Ok(Some(horse)) = crate::horse::run::by_sandbox::find(&store, &run) else {
                continue;
            };
            let Ok(session) = store.session(&horse.session) else {
                continue;
            };
            let mail = crate::mail::read(&env, &run, crate::mail::Dir::Up);
            let mail_seq = mail.last().map_or(0, |message| message.seq);
            if session.head != last_head || mail_seq != last_mail {
                let base = super::anchor::load(&store, &run)
                    .ok()
                    .flatten()
                    .and_then(|anchor| anchor.base);
                match super::records::read(&env, &store, &run, base.as_deref()) {
                    Ok(next) => {
                        source = next;
                        last_head = session.head;
                        last_mail = mail_seq;
                        if let Err(error) = super::learning::stats(&env, &store, &run) {
                            log::warn!("trainer learning update failed: {error:#}");
                        }
                    }
                    Err(error) => {
                        log::warn!("trainer monitor cannot read run records: {error:#}");
                        continue;
                    }
                }
            }
            let active = store
                .read_ref(&format!("live/{run}.json"))
                .ok()
                .flatten()
                .and_then(|bytes| {
                    serde_json::from_slice::<crate::horse::live::harness::Record>(&bytes).ok()
                })
                .and_then(|record| record.sessions.first().map(|session| !session.idle))
                .unwrap_or(false);
            // Avoid charging the interval when either observation is idle.
            // A short idle period entirely between polls cannot be measured.
            let active_elapsed = if active && was_active { elapsed } else { 0 };
            was_active = active;
            if running.load(Ordering::Acquire) {
                // Keep collecting signals without reserving a second concurrent launch.
                let _ = super::triggers::transact(&store, &run, |state| {
                    super::triggers::observe_records(state, &source);
                    if active_elapsed > 0 {
                        state.active(active_elapsed, &cfg);
                    }
                });
                continue;
            }
            let reduced = super::learning::unhelpful(&store, &run).unwrap_or_default();
            let signal = super::triggers::tick_adaptive(
                &store,
                &run,
                &source,
                active_elapsed,
                crate::horse::now_ms(),
                &cfg,
                &reduced,
            );
            if let Ok(Some(signal)) = signal {
                running.store(true, Ordering::Release);
                let run = run.clone();
                let env = env.clone();
                let running = running.clone();
                std::thread::spawn(move || {
                    if let Err(error) =
                        super::sandbox::run_reserved(&env, &run, None, false, signal.reason())
                    {
                        log::warn!("automatic trainer failed: {error:#}");
                    }
                    running.store(false, Ordering::Release);
                });
            }
        }
    });
    Some(Monitor {
        stop,
        thread: Some(thread),
    })
}
