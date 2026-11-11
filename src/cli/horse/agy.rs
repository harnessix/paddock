//! The hidden agy commands of `paddock horse`.

use anyhow::Result;
use paddock::horse::{agy, Store};
use std::path::Path;

pub(crate) fn ingest(st: &Store, path: &Path, version: &str, horse: Option<&str>) -> Result<()> {
    let r = agy::ingest(st, path, version, horse)?;
    println!(
        "{}: {} new, {} kept{}{}",
        r.session,
        r.added,
        r.kept,
        if r.updated > 0 {
            format!(
                ", {} rows changed (the old versions stay in the store)",
                r.updated
            )
        } else {
            String::new()
        },
        if r.dropped > 0 {
            format!(
                ", {} rows are not in the database any more and stay in the store",
                r.dropped
            )
        } else {
            String::new()
        },
    );
    Ok(())
}

pub(crate) fn project(st: &Store, session: &str, home: &Path) -> Result<()> {
    let p = agy::project(st, session, home)?;
    println!("{} {}", p.native_id, p.path.display());
    if p.forked {
        println!("This fork starts with a cold cache: the agy cache scope is unknown, so paddock treats a new conversation id as cold.");
    }
    Ok(())
}
