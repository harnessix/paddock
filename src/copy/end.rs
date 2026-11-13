//! The end of a run on a copy: the note for the user, the automatic
//! apply, and the cleanup of a copy without changes.

use super::apply::{self, Opts};
use super::changes::{self, Status};
use super::config::Apply;
use super::run::Run;
use super::Config;
use crate::env::HostEnv;

/// What the end of a run has to know.
#[derive(Debug, Clone, Copy, Default)]
pub struct End {
    /// The run had auto approvals.
    pub auto_approvals: bool,
    /// The run came from a launcher entry: its terminal closes at the end.
    pub launcher: bool,
    pub quiet: bool,
}

/// What the end step did.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The run has no copy.
    NoCopy,
    /// The copy has no open change. Without an apply, it is gone.
    Clean,
    /// paddock applied the changes: this many.
    Applied(usize),
    /// Changes wait for the user: this many, and the conflicts among them.
    Open(usize, usize),
}

/// The first changes, one per line, for the note at the end.
pub fn preview(st: &Status, max: usize) -> String {
    let mut out = String::new();
    for c in st.changes.iter().take(max) {
        let word = match (c.conflict, c.kept) {
            (Some(_), _) => "conflict",
            (None, true) => "kept",
            (None, false) => c.kind.word(),
        };
        out.push_str(&format!("  {} {:<9} {}\n", c.kind.glyph(), word, c.path));
    }
    if st.changes.len() > max {
        out.push_str(&format!("  and {} more\n", st.changes.len() - max));
    }
    out
}

fn auto_apply(env: &HostEnv, run: &mut Run, st: &Status, e: &End) -> Option<usize> {
    if !e.auto_approvals || st.conflicts() > 0 || st.ready() == 0 {
        return None;
    }
    match apply::apply(env, run, &Opts::default()) {
        Ok(done) => Some(done.count()),
        Err(err) => {
            log::warn!("the automatic apply did not run: {err:#}");
            None
        }
    }
}

/// The end of the run `id`. `ask` puts a yes-or-no question to the user
/// of a launcher run; None when nobody can answer.
pub fn of_run(env: &HostEnv, id: &str, e: End, ask: &mut dyn FnMut(&str) -> Option<bool>) -> Ended {
    let Some(mut run) = super::run::of_id(env, id) else {
        return Ended::NoCopy;
    };
    run.rec.ended = Some(crate::state::now());
    if let Err(err) = run.save() {
        log::warn!("cannot record the end of the copy: {err:#}");
    }
    let cfg = Config::load(env).unwrap_or_default();
    let st = match changes::status(&run) {
        Ok(st) => st,
        Err(err) => {
            log::warn!("cannot list the changes of the copy: {err:#}");
            return Ended::Open(0, 0);
        }
    };
    let short = crate::ids::short(id).to_string();
    let folder = run.rec.project.display().to_string();
    if st.changes.is_empty() {
        // Nothing to apply. A copy that never had an apply goes now: there
        // is nothing to look at. After an apply it stays for a while
        // (`sweep`), so an undo of the apply shows the changes again. The
        // backups stay in both cases.
        if run.rec.synced.is_empty() {
            if let Err(err) = run.remove_tree() {
                log::warn!("cannot remove the copy: {err:#}");
            }
        }
        if !e.quiet {
            crate::note::info(format!(
                "the copy has no open change. {folder} is as the run left it."
            ));
        }
        return Ended::Clean;
    }
    let (n, conflicts) = (st.changes.len(), st.conflicts());
    if !e.quiet {
        crate::note::info(format!(
            "the copy has {}. Your folder {folder} did not change.\n{}",
            st.counts_line(),
            preview(&st, 10)
        ));
    }
    let applied = |n: usize| {
        if !e.quiet {
            crate::note::info(format!(
                "applied {n} change(s) to {folder}. A backup came first: `paddock undo` takes the apply back."
            ));
        }
        Ended::Applied(n)
    };
    if cfg.apply == Apply::Auto {
        if let Some(done) = auto_apply(env, &mut run, &st, &e) {
            return applied(done);
        }
        if !e.quiet && conflicts > 0 {
            crate::note::info("no automatic apply, because of the conflict(s).");
        }
    } else if e.launcher && conflicts == 0 && st.ready() > 0 {
        // The terminal of a launcher run closes now: ask here. A question
        // is never hidden, also with -q (note.rs).
        let q = format!("Apply {} change(s) to {folder}?", st.ready());
        if ask(&q) == Some(true) {
            match apply::apply(env, &mut run, &Opts::default()) {
                Ok(done) => return applied(done.count()),
                Err(err) => crate::note::info(format!("the apply did not run: {err:#}")),
            }
        }
    }
    if !e.quiet {
        crate::note::info(format!(
            "`paddock changes {short}` lists the changes, `paddock apply {short}` writes them to your folder, `paddock discard {short}` drops them."
        ));
    }
    Ended::Open(n, conflicts)
}

/// A copy of an ended run without an open change stays this long, so an
/// undo of its last apply still shows the changes.
pub const KEEP_CLEAN: u64 = 7 * 86_400;

/// Remove the copies of the store that nobody needs: the run ended more
/// than `KEEP_CLEAN` ago and has no open change. A copy with an open
/// change is never removed here: only `paddock discard` does that.
pub fn sweep(env: &HostEnv, store: &super::store::Store, now: u64) -> usize {
    let mut removed = 0;
    for mut run in super::run::of_store(store) {
        let old = run.rec.ended.is_some_and(|t| t + KEEP_CLEAN < now);
        if !old || !run.has_tree() || run.running(env) {
            continue;
        }
        if changes::status(&run).is_ok_and(|st| st.changes.is_empty()) && run.remove_tree().is_ok()
        {
            removed += 1;
        }
    }
    removed
}
