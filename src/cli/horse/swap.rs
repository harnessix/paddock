//! `paddock horse swap`: a Codex session continues at another branch under
//! the same thread id, through the custody of the run that holds it or on
//! disk.

use anyhow::{bail, Context as _, Result};
use paddock::horse::codex::swap::{self, request};
use paddock::horse::guard::{self, Op};
use paddock::horse::{self, Store};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The answer of the run may come this long after the wait for the turn.
const GRACE: Duration = Duration::from_secs(60);

pub(crate) fn run(
    st: &Store,
    session: &str,
    branch: &str,
    interrupt: bool,
    wait: u64,
    codex_home: Option<PathBuf>,
    yes: bool,
) -> Result<i32> {
    let s = st.session(session)?;
    let to = swap::resolve_branch(st, &s, branch)?;
    let preview = guard::check(st, session, &Op::Swap { to: to.clone() }, horse::now_ms())?;
    eprintln!("paddock: {}", preview.message());
    if !preview.keeps && !yes {
        eprintln!("paddock: nothing changed; add --yes to swap anyway");
        return Ok(1);
    }
    let holder = horse::live::records(st)?
        .into_iter()
        .find(|r| r.sessions.iter().any(|x| x.session == session));
    let Some(holder) = holder else {
        let home = codex_home.with_context(|| {
            format!(
                "session {session} is in no live run, so paddock swaps its rollout on disk: give --codex-home (the Codex home that holds sessions/, for example the ~/.codex of its sandbox)"
            )
        })?;
        st.save_verdict(&preview)?;
        let f = swap::swap_file(st, session, &to, &home)?;
        swap::record(st, &s, "swap", &f)?;
        println!(
            "session {session} continues at {} under thread {}",
            to.short(),
            f.thread
        );
        if let Some(k) = &f.kept {
            println!("the old head stays in session {k}");
        }
        return Ok(0);
    };
    let wait = Duration::from_secs(wait);
    let r = request::ask(st, session, &to, interrupt, wait)?;
    eprintln!(
        "paddock: run {} holds session {session}; the swap waits until no turn runs{}",
        holder.run,
        if interrupt {
            " (the running turn is interrupted)"
        } else {
            ""
        }
    );
    let started = Instant::now();
    let mut checked = Instant::now();
    let answer = loop {
        match request::answer(st, session, &r.id) {
            Ok(Some(a)) => break a,
            Ok(None) => {}
            Err(e) => return Err(e),
        }
        if started.elapsed() > wait + GRACE {
            request::remove(st, session, &r.id)?;
            bail!(
                "run {} did not answer within {} s; nothing changed",
                holder.run,
                (wait + GRACE).as_secs()
            );
        }
        if checked.elapsed() > Duration::from_secs(1) {
            checked = Instant::now();
            let held = horse::live::records(st)?
                .iter()
                .any(|x| x.run == holder.run && x.sessions.iter().any(|y| y.session == session));
            // The custody may have answered just before it ended.
            if !held && request::answer(st, session, &r.id)?.is_none() {
                request::remove(st, session, &r.id)?;
                bail!(
                    "run {} ended before it reached a safe point; nothing changed. Swap the stopped session with --codex-home",
                    holder.run
                );
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    request::remove(st, session, &r.id)?;
    match answer.state {
        request::State::Done => {
            if let Some(v) = &answer.verdict {
                if v.message() != preview.message() {
                    eprintln!("paddock: at the swap: {}", v.message());
                }
            }
            println!(
                "session {session} continues at {} under thread {} (after {} ms of waiting)",
                to.short(),
                answer.thread.as_deref().unwrap_or("?"),
                answer.waited_ms
            );
            if let Some(k) = &answer.kept {
                println!("the old head stays in session {k}");
            }
            Ok(0)
        }
        _ => {
            eprintln!(
                "paddock: the swap of session {session} failed: {}",
                answer.error.as_deref().unwrap_or("no reason given")
            );
            if answer.ended {
                eprintln!(
                    "paddock: the custody of run {} ended; the next run resumes from the rollout",
                    holder.run
                );
            }
            Ok(1)
        }
    }
}
