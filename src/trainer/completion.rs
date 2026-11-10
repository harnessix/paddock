//! Hold a final worker report until its independent review finishes.
use crate::{env::HostEnv, horse::Store, mail};
use anyhow::Result;

/// True when the report may proceed. False means a review is in flight.
pub fn ready(env: &HostEnv, run: &str, summary: &str) -> Result<bool> {
    let config = super::triggers::Config::load(env)?;
    if !config.enabled {
        return Ok(true);
    }
    if !crate::horse::root(env).exists() {
        return Ok(true);
    }
    let store = Store::open_host(env)?;
    if crate::horse::run::by_sandbox::find(&store, run)?.is_none() {
        return Ok(true);
    }
    if !super::anchor::load(&store, run)?.is_some_and(|anchor| anchor.ceiling.is_some()) {
        return Ok(true);
    }
    let decision = super::triggers::transact(&store, run, |state| {
        state.final_decision(crate::horse::now_ms(), &config)
    })?;
    match decision {
        super::triggers::FinalDecision::Proceed => Ok(true),
        super::triggers::FinalDecision::Capped => {
            let _ = mail::append(
                env,
                run,
                mail::Dir::Up,
                mail::Message::event(
                    "trainer-capped",
                    "Final review skipped: trainer frequency cap or budget reached.",
                ),
            );
            Ok(true)
        }
        super::triggers::FinalDecision::Wait => Ok(false),
        super::triggers::FinalDecision::Failed => anyhow::bail!("the last final review failed; retry after the frequency cap or adjust the review budget"),
        super::triggers::FinalDecision::Start(token) => {
            let _ = mail::append(
                env,
                run,
                mail::Dir::Up,
                mail::Message::event(
                    "trainer-before-done",
                    format!("Worker requested final completion: {summary}"),
                ),
            );
            let env = env.clone();
            let run = run.to_owned();
            std::thread::spawn(move || {
                let result = super::sandbox::run_reserved(
                    &env,
                    &run,
                    None,
                    false,
                    "review before final completion",
                )
                .and_then(|status| {
                    anyhow::ensure!(
                        status.state == crate::spawn::State::Exited && status.exit_code == Some(0),
                        "trainer did not finish successfully"
                    );
                    Ok(())
                });
                let success = result.is_ok();
                if let Err(error) = result {
                    log::warn!("final trainer review failed: {error:#}");
                    let message = mail::Message::new("trainer", mail::Kind::Say, "The final trainer review could not finish. Check the run and retry your final report.");
                    let _ = mail::send_down(&env, &run, message);
                }
                let _ =
                    super::triggers::transact(&store, &run, |state| state.final_finished(token, success));
            });
            Ok(false)
        }
    }
}
