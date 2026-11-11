//! The hidden Grok Build commands of `paddock horse`, and the swap of a
//! Grok session on disk.

use anyhow::{Context as _, Result};
use paddock::horse::grok::{self, swap};
use paddock::horse::guard::{self, Op};
use paddock::horse::{self, Store};
use std::path::{Path, PathBuf};

pub(crate) fn ingest(st: &Store, path: &Path, version: &str, horse: Option<&str>) -> Result<()> {
    let r = grok::ingest(st, path, version, horse)?;
    println!(
        "{}: {} new, {} kept{}{}{}",
        r.session,
        r.added,
        r.kept,
        if r.rewritten {
            ", chat history was rewritten (its old items stay in the store)"
        } else {
            ""
        },
        if r.missing > 0 {
            format!(
                ", {} rows are not in the files any more and stay in the store",
                r.missing
            )
        } else {
            String::new()
        },
        if r.torn { ", torn tail left out" } else { "" }
    );
    Ok(())
}

pub(crate) fn project(st: &Store, session: &str, home: &Path, swap: bool) -> Result<()> {
    let p = grok::project(st, session, home, swap)?;
    println!("{} {}", p.native_id, p.path.display());
    if p.forked && !swap {
        println!("This fork starts with a cold cache.");
    }
    Ok(())
}

/// `paddock horse swap` of a Grok session that no run holds.
pub(crate) fn swap(
    st: &Store,
    session: &str,
    branch: &str,
    grok_home: Option<PathBuf>,
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
    let home = grok_home.with_context(|| {
        format!(
            "paddock swaps the files of Grok session {session} on disk: give --grok-home (the Grok home that holds sessions/, for example the ~/.grok of its sandbox)"
        )
    })?;
    st.save_verdict(&preview)?;
    let f = swap::swap_file(st, session, &to, &home)?;
    swap::record(st, &s, &f)?;
    println!(
        "session {session} continues at {} under Grok session {}",
        to.short(),
        f.native
    );
    if let Some(k) = &f.kept {
        println!("the old head stays in session {k}");
    }
    Ok(0)
}
