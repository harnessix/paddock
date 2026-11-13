//! The switches between the link and the sync mode, and the watcher
//! that syncs while a run goes on.

use super::*;
use crate::note::Level::Warning;

/// Make `.paddock` of the work tree a real folder in sync mode. From the
/// link: the link goes, and the first sync copies the loft's notes into
/// the new folder. The store does not change.
pub fn to_sync(env: &HostEnv, loft: &Loft, worktree: &Path, who: &str) -> Result<Report> {
    let _lock = lock(loft)?;
    let top = Root::open(worktree)?.dir(Path::new(""), false)?;
    let name = OsStr::new(NOTES);
    let created = match top.kind(name)? {
        Some(Kind::Symlink) => {
            if !crate::loft::host::linked(env, worktree) {
                bail!(
                    "{}/{NOTES} is a symlink that does not lead to the loft; remove it first",
                    worktree.display()
                );
            }
            top.remove(name)?;
            top.make_dir(name, 0o700)?;
            true
        }
        Some(Kind::Dir) => false,
        None => {
            top.make_dir(name, 0o700)?;
            true
        }
        Some(_) => bail!("{}/{NOTES} is not a folder", worktree.display()),
    };
    if created || !base_file(loft, worktree).is_file() {
        // The base marks the sync mode, also when the first sync stops. A
        // new folder starts from an empty base, whatever an old one said:
        // its first sync only pulls. (The marker check of the sync says
        // the same; this one does not depend on it.)
        save_base(
            loft,
            worktree,
            &Base {
                worktree: worktree.to_path_buf(),
                ..Base::default()
            },
        )?;
    }
    crate::loft::exclude(worktree, "the loft", &[format!("/{NOTES}")])?;
    sync_locked(loft, worktree, who)
}

/// The runs of `worktree` other than `who`.
pub(super) fn other_runs(env: &HostEnv, worktree: &Path, who: &str) -> Vec<String> {
    let me = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    crate::state::Running::list(env)
        .into_iter()
        .filter(|r| r.id != who)
        .filter(|r| {
            let w = worktree_of(&r.project);
            std::fs::canonicalize(&w).unwrap_or(w) == me
        })
        .map(|r| r.id)
        .collect()
}

/// Back to the link: one last sync, then the folder moves aside, and
/// every file in it must be in the loft with the same content. When one
/// differs, the folder comes back and nothing changes. Then each file
/// goes the way a sync deletes one: renamed aside, read again, removed
/// only when it still has the content of the loft. A file that changed
/// in the meantime stays in the folder aside. Refused while another run
/// of the work tree is active: its agent writes to the folder.
pub fn to_link(env: &HostEnv, loft: &Loft, worktree: &Path, who: &str) -> Result<String> {
    let runs = other_runs(env, worktree, who);
    if !runs.is_empty() {
        bail!(
            "a run of {} is active ({}): switch {NOTES} to the link after it ends",
            worktree.display(),
            runs.join(", ")
        );
    }
    let _lock = lock(loft)?;
    let r = sync_locked(loft, worktree, who)?;
    let top = Root::open(worktree)?.dir(Path::new(""), false)?;
    let aside = format!("{NOTES}.to-link-{}", tag(who));
    crate::loft::exclude(worktree, "the loft", &[format!("/{NOTES}.to-link-*")])?;
    top.rename_noreplace(OsStr::new(NOTES), OsStr::new(&aside))
        .with_context(|| format!("cannot move {}/{NOTES} aside", worktree.display()))?;
    let staged = top.subdir(OsStr::new(&aside), false)?;
    let store_dir = open_store(loft)?;
    let (mut a, mut s) = (Side::default(), Side::default());
    scan(&staged, "", 0, &mut a);
    scan(&store_dir, "", 0, &mut s);
    let differ: Vec<String> = a
        .files
        .iter()
        .filter(|(k, h)| s.files.get(*k) != Some(*h))
        .map(|(k, _)| k.clone())
        .chain(a.blocked.iter().cloned())
        .chain(a.stale.iter().map(|(k, _)| k.clone()))
        .collect();
    if !differ.is_empty() {
        let back = top.rename_noreplace(OsStr::new(&aside), OsStr::new(NOTES));
        let where_ = if back.is_ok() {
            format!("{}/{NOTES} stays a folder", worktree.display())
        } else {
            format!("the folder is now {}/{aside}", worktree.display())
        };
        bail!(
            "{} not synced with the loft ({}): {where_}. Run `paddock loft sync`, solve the conflicts, then switch again",
            crate::loft::push::names(&differ.iter().cloned().collect()),
            r.lines().join("; ")
        );
    }
    #[cfg(test)]
    AFTER_SCAN.with(|f| {
        if let Some(f) = f.borrow_mut().take() {
            f()
        }
    });
    // Only verified copies go, each one checked again at its removal. The
    // inbox moves into the loft's inbox.
    let mut left = Vec::new();
    let t = tag(who);
    for (k, h) in &a.files {
        match del(&staged, k, h, &t) {
            Ok(Del::Done) => {}
            Ok(Del::Kept) => left.push(format!("{k} (changed during the switch)")),
            Err(e) => left.push(format!("{k} ({e:#})")),
        }
    }
    if let Ok(inbox) = staged.subdir(OsStr::new("inbox"), false) {
        let to = loft.notes().join("inbox");
        crate::privfs::dir(&to)?;
        for (n, k) in inbox.entries()? {
            let dst = to.join(&n);
            if k == Kind::File
                && !dst.exists()
                && std::fs::rename(inbox.path().join(&n), &dst).is_ok()
            {
                continue;
            }
            left.push(format!("inbox/{}", n.to_string_lossy()));
        }
    }
    let _ = staged.remove(OsStr::new(".gitignore"));
    let _ = staged.remove(OsStr::new(MARK));
    prune(&staged);
    drop(staged);
    let gone = top.remove_dir(OsStr::new(&aside)).is_ok();
    // The base goes before the link: a stop between the two leaves no
    // stale base (and a folder without a marker never uses one).
    let _ = std::fs::remove_file(base_file(loft, worktree));
    crate::loft::host::link(loft, worktree)?;
    let mut msg = format!(
        "{}/{NOTES} leads to the loft's notes again.",
        worktree.display()
    );
    if !gone {
        msg.push_str(&format!(
            " What the loft does not have ({}) stays in {}/{aside}.",
            left.join(", "),
            worktree.display()
        ));
    }
    Ok(msg)
}

/// Remove the empty folders below `d`.
pub(in crate::loft) fn prune(d: &Dir) {
    for (n, k) in d.entries().unwrap_or_default() {
        if k == Kind::Dir {
            if let Ok(sub) = d.subdir(&n, false) {
                prune(&sub);
            }
            let _ = d.remove_dir(&n);
        }
    }
}

/// Bring `.paddock` of the work tree to the configured mode. Returns a
/// line for the user when it switched.
pub fn apply_mode(env: &HostEnv, worktree: &Path, who: &str) -> Result<Option<String>> {
    let loft = Loft::of(env, worktree);
    let m = std::fs::symlink_metadata(worktree.join(NOTES));
    match mode(env, worktree) {
        Mode::Sync => {
            if synced(&loft, worktree) {
                return Ok(None);
            }
            let from_link = m.as_ref().is_ok_and(|m| m.file_type().is_symlink());
            let r = to_sync(env, &loft, worktree, who)?;
            Ok(Some(format!(
                "{}/{NOTES} is a real folder of the project now, synced with the loft{}.{}",
                worktree.display(),
                if from_link { " (it was a link)" } else { "" },
                if r.conflicts.is_empty() {
                    String::new()
                } else {
                    format!(" {}", r.lines().join("; "))
                }
            )))
        }
        Mode::Link => {
            if !synced(&loft, worktree) {
                return Ok(None);
            }
            to_link(env, &loft, worktree, who).map(Some)
        }
    }
}

/// A run in sync mode: switch the mode if needed, then sync once. Errors
/// are warnings; the run goes on with what the folder has.
pub fn start_of_run(env: &HostEnv, worktree: &Path, who: &str) {
    match apply_mode(env, worktree, who) {
        Ok(Some(msg)) => crate::note::info(msg),
        Ok(None) => {}
        Err(e) => log::warn!("loft mode of {}: {e:#}", worktree.display()),
    }
    let loft = Loft::of(env, worktree);
    if synced(&loft, worktree) {
        report(sync(&loft, worktree, who), worktree);
    }
}

/// The end of a run, `paddock loft commit`: one sync when the work tree
/// is in sync mode.
pub fn sync_if_synced(env: &HostEnv, worktree: &Path, who: &str) {
    let loft = Loft::of(env, worktree);
    if synced(&loft, worktree) {
        report(sync(&loft, worktree, who), worktree);
    }
}

/// Tell the user what a sync did. `loud`: conflicts, kept deletes and
/// skipped paths go to stderr. The watcher is quiet: stderr belongs to
/// the harness while it runs.
/// Returns the notable lines (conflicts, kept deletes, skipped paths).
pub(super) fn report_with(r: Result<Report>, worktree: &Path, loud: bool) -> Vec<String> {
    let mut seen = Vec::new();
    match r {
        Ok(r) => {
            for l in r.lines() {
                let notable =
                    l.starts_with("conflict") || l.starts_with("kept") || l.starts_with("skipped");
                if notable && loud {
                    crate::note::say_line(Warning, &format!("paddock: loft sync: {l}"));
                } else if notable {
                    log::warn!("loft sync of {}: {l}", worktree.display());
                } else {
                    log::info!("loft sync of {}: {l}", worktree.display());
                }
                if notable {
                    seen.push(l);
                }
            }
        }
        Err(e) if loud => crate::note::say_line(
            Warning,
            &format!(
                "paddock: cannot sync {}/{NOTES} with the loft: {e:#}",
                worktree.display()
            ),
        ),
        Err(e) => log::warn!(
            "cannot sync {}/{NOTES} with the loft: {e:#}",
            worktree.display()
        ),
    }
    seen
}

pub(super) fn report(r: Result<Report>, worktree: &Path) {
    report_with(r, worktree, true);
}

/// A cheap fingerprint of a tree: names, kinds, sizes and change times.
pub(super) fn fingerprint(
    d: &Dir,
    h: &mut std::collections::hash_map::DefaultHasher,
    depth: usize,
) {
    let Ok(mut e) = d.stats() else {
        0u8.hash(h);
        return;
    };
    e.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, st) in e {
        n.hash(h);
        st.size.hash(h);
        st.mtime.hash(h);
        (st.kind as u8).hash(h);
        if st.kind == Kind::Dir && depth < MAX_DEPTH {
            if let Ok(sub) = d.subdir(&n, false) {
                fingerprint(&sub, h, depth + 1);
            }
        }
    }
}

pub(super) fn state_of(loft: &Loft, worktree: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Ok(d) = open_local(worktree) {
        fingerprint(&d, &mut h, 0);
    }
    if let Ok(d) = Root::open(&loft.tree()).and_then(|r| r.dir(Path::new(NOTES), false)) {
        fingerprint(&d, &mut h, 0);
    }
    h.finish()
}

/// The watcher of a run or of `paddock loft sync --watch`: a sync each
/// time either side changes. It syncs once more when it stops.
pub struct Watch {
    pub(super) stop: Arc<AtomicBool>,
    pub(super) thread: Option<std::thread::JoinHandle<Vec<String>>>,
}

impl Watch {
    /// Start watching `worktree`; None when it is not in sync mode.
    pub fn start(env: &HostEnv, worktree: &Path, who: &str) -> Option<Self> {
        let loft = Loft::of(env, worktree);
        if !synced(&loft, worktree) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (s, w, who) = (stop.clone(), worktree.to_path_buf(), who.to_string());
        let thread = std::thread::Builder::new()
            .name("loft-sync".into())
            .spawn(move || watch_loop(&loft, &w, &who, &s, WATCH_EVERY, false))
            .ok()?;
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Stop the watcher after one last sync. Returns the conflicts, kept
    /// deletes and skipped paths of all its syncs, each line once.
    pub fn stop(mut self) -> Vec<String> {
        self.finish()
    }

    pub(super) fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        let mut lines = self
            .thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default();
        let mut seen = BTreeSet::new();
        lines.retain(|l| seen.insert(l.clone()));
        lines
    }
}

/// The summary of a run's watcher for stderr: the watcher logs only,
/// because stderr belongs to the harness while it runs.
pub(super) fn summary(lines: &[String]) -> Vec<String> {
    const MAX: usize = 20;
    let mut out: Vec<String> = lines
        .iter()
        .take(MAX)
        .map(|l| format!("paddock: loft sync during the run: {l}"))
        .collect();
    if lines.len() > MAX {
        out.push(format!(
            "paddock: loft sync during the run: {} more (`paddock loft status` lists the open conflicts)",
            lines.len() - MAX
        ));
    }
    out
}

/// Sync whenever the fingerprint changes, until `stop`, and once more
/// DELETE_WAIT after a sync with deletes that wait. `loud`: see
/// `report_with`.
pub fn watch_loop(
    loft: &Loft,
    worktree: &Path,
    who: &str,
    stop: &AtomicBool,
    every: Duration,
    loud: bool,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut last = None;
    let mut again: Option<std::time::Instant> = None;
    loop {
        let now = state_of(loft, worktree);
        if last != Some(now) || again.is_some_and(|t| std::time::Instant::now() >= t) {
            let r = sync(loft, worktree, who);
            // Deletes that wait get their second scan after DELETE_WAIT,
            // deferred writes at the next look.
            again = r.as_ref().ok().and_then(|r| {
                if !r.waiting.is_empty() {
                    Some(std::time::Instant::now() + DELETE_WAIT)
                } else if !r.deferred.is_empty() {
                    Some(std::time::Instant::now())
                } else {
                    None
                }
            });
            seen.extend(report_with(r, worktree, loud));
            // The sync itself changes the trees: read them again.
            last = Some(state_of(loft, worktree));
        }
        let mut waited = Duration::ZERO;
        while waited < every {
            if stop.load(Ordering::Relaxed) {
                seen.extend(last_sync(loft, worktree, who, loud));
                return seen;
            }
            let d = Duration::from_millis(100).min(every - waited);
            std::thread::sleep(d);
            waited += d;
        }
    }
}

/// The last sync of a watcher. A delete that waits gets its second scan
/// here: the watcher waits DELETE_WAIT and syncs again, so the delete is
/// not left behind in the base when the work tree goes after the run.
/// A delete that still waits after that is a notable line.
pub(super) fn last_sync(loft: &Loft, worktree: &Path, who: &str, loud: bool) -> Vec<String> {
    let mut seen = Vec::new();
    let mut r = sync(loft, worktree, who);
    for _ in 0..2 {
        let (wait, more) = match &r {
            Ok(r) => (!r.waiting.is_empty(), !r.deferred.is_empty()),
            Err(_) => (false, false),
        };
        if !wait && !more {
            break;
        }
        seen.extend(report_with(r, worktree, loud));
        if wait {
            std::thread::sleep(DELETE_WAIT + Duration::from_millis(100));
        }
        r = sync(loft, worktree, who);
    }
    let still: Vec<String> = r.as_ref().map(|r| r.waiting.clone()).unwrap_or_default();
    seen.extend(report_with(r, worktree, loud));
    if !still.is_empty() {
        let l = format!(
            "waits: {} deleted in the work tree, not yet in the loft trash. The next sync of {} does it",
            names_of(&still),
            worktree.display()
        );
        match loud {
            true => crate::note::say_line(Warning, &format!("paddock: loft sync: {l}")),
            false => log::warn!("loft sync of {}: {l}", worktree.display()),
        }
        seen.push(l);
    }
    seen
}

impl Drop for Watch {
    /// The end of the run: the harness is gone, so the summary goes to
    /// stderr.
    fn drop(&mut self) {
        if self.thread.is_none() {
            return;
        }
        for l in summary(&self.finish()) {
            crate::note::say_line(crate::note::Level::Info, &l);
        }
    }
}
