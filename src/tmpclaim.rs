//! Fixed names in `/private/tmp` for a Seatbelt run on macOS (TMP1).
//!
//! On Linux each sandbox gets a private `/tmp` (bwrap `--tmpfs /tmp`). Seatbelt
//! cannot mount or remap a path, and it has no filter for the owner of a
//! file. So a Seatbelt run gets a private `TMPDIR` (run.rs `private_tmp`), and
//! `/private/tmp` itself stays denied. That is enough for every program that
//! builds its paths from `$TMPDIR`.
//!
//! Some programs use a fixed name in `/private/tmp` instead. codex 0.166 binds
//! its app server socket in `/private/tmp/codex-daemon-<uid>/<sha256>`. A
//! harness profile names such entries (`[[macos.tmp]]`), and paddock claims
//! them on the host before the sandbox starts:
//!
//! - `dir`, `file`: paddock makes the entry, so it is the run's own. The run
//!   gets read and write on it (a folder: on all below it, Unix sockets too).
//! - `socket`: the run binds the socket itself. paddock checks that the name
//!   is free.
//! - `shared`: a folder that every run uses, with names in it that are the
//!   run's own (`entries`). paddock makes the folder (mode 0700) when it is
//!   missing. The run gets its own entries only, never the folder or the
//!   entries of the host and of other runs.
//!
//! A name that exists and that no live paddock run claims belongs to the host
//! or to another program: the run does not get it. A name that a live run
//! claims is not given twice: the second run is told which run holds it.
//! The claims are recorded in `STATE/tmp/<run>.json` before the start. The end
//! of the run stops every process of the user that runs in this run's
//! sandbox (the kernel tells it, `sandbox_check`, never a program name):
//! a server that left the process group and its helpers. Then it removes
//! the entries (never through a symlink). While a process of the run still
//! runs, the names and the record stay. A run that ended without its end step
//! (kill -9) is released by the next paddock command, or by `paddock gc`.
//! A start also stops what an earlier run of the same home left without a
//! record.
//!
//! The design and the rejected options: `.paddock/TMP1-notes.md`.

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

/// The real path of `/tmp` on macOS.
pub const ROOT: &str = "/private/tmp";
/// The folder of the claim records in paddock's state folder.
pub const RECORDS: &str = "tmp";
/// The folder of the made marks, next to the records (TMP1-F4).
pub const MADE: &str = "tmp-made";

mod want;
pub use want::{check_name, expand, resolve, Resolved, Vars, Want, WantKind};

/// What a recorded entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Dir,
    File,
    Socket,
    /// The folder of a `shared` want.
    Folder,
    /// A name in a `shared` folder.
    Entry,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Item {
    /// Relative to the root: `NAME` or `FOLDER/ENTRY`.
    pub path: String,
    pub kind: ItemKind,
    /// paddock made it (a folder of `shared`: a paddock run made it).
    pub made: bool,
}

/// The claims of one run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Record {
    pub run: String,
    /// The paddock process of the run. The claims hold while it lives.
    pub pid: u32,
    pub root: PathBuf,
    pub items: Vec<Item>,
    /// Pid files of processes to stop at the end.
    #[serde(default)]
    pub stop: Vec<PathBuf>,
    /// Paths that only this run's sandbox reads (its probe file, next to
    /// the record) and writes (its TMPDIR): the check before a process is
    /// stopped.
    #[serde(default)]
    pub probe_read: PathBuf,
    #[serde(default)]
    pub probe_write: PathBuf,
    /// The real sandbox home of the run.
    #[serde(default)]
    pub home: PathBuf,
}

impl Record {
    pub fn names(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| i.kind != ItemKind::Folder)
            .map(|i| format!("{}/{}", self.root.display(), i.path))
            .collect()
    }
}

/// The outcome of a claim.
#[derive(Debug, Default)]
pub struct Claim {
    pub record: Option<Record>,
    /// Lines for the user: what the run got, and what it did not get and why.
    pub granted: Vec<String>,
    pub refused: Vec<String>,
    /// Launch flags to drop.
    pub drop_args: Vec<String>,
}

/// Where a claim works: the root, the records and how a run is told live.
pub struct Place<'a> {
    pub root: &'a Path,
    pub records: &'a Path,
    pub uid: u32,
    pub live: &'a dyn Fn(u32) -> bool,
    /// The processes of the host: found by the sandbox they run in.
    pub procs: &'a dyn Procs,
}

/// The processes of the host as a release sees them. [`HostProcs`] asks the
/// kernel; the tests use a fake.
pub trait Procs {
    /// The pids of the processes of this user.
    fn list(&self) -> Vec<u32>;
    /// Does `pid` run in the sandbox of `rec`? The probe paths of the record
    /// decide it, never the name of the program.
    fn of_run(&self, pid: u32, rec: &Record) -> bool;
    /// Does `pid` run in a paddock sandbox of the home `home` that may read
    /// none of `keep` (the run folders of the live runs)?
    fn of_home(&self, pid: u32, home: &Path, keep: &[PathBuf]) -> bool;
    /// The process runs (a zombie does not count).
    fn alive(&self, pid: u32) -> bool;
    /// SIGTERM, or SIGKILL with `kill`.
    fn signal(&self, pid: u32, kill: bool);
    /// How long a process gets after SIGTERM.
    fn grace(&self) -> std::time::Duration {
        std::time::Duration::from_secs(3)
    }
}

/// The real processes. Only macOS has Seatbelt runs that leave processes:
/// on Linux the pid namespace of bwrap ends them all, so the list is empty.
pub struct HostProcs;

impl Procs for HostProcs {
    fn list(&self) -> Vec<u32> {
        macos::list()
    }
    fn of_run(&self, pid: u32, rec: &Record) -> bool {
        macos::of_run(pid, rec).unwrap_or(false)
    }
    fn of_home(&self, pid: u32, home: &Path, keep: &[PathBuf]) -> bool {
        macos::of_home(pid, home, keep).unwrap_or(false)
    }
    fn alive(&self, pid: u32) -> bool {
        macos::alive(pid)
    }
    fn signal(&self, pid: u32, kill: bool) {
        let sig = if kill { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: kill(2) with a plain signal.
        unsafe { libc::kill(pid as libc::pid_t, sig) };
    }
}

fn record_file(records: &Path, run: &str) -> PathBuf {
    records.join(format!("{run}.json"))
}

/// The end of the name of a read probe. Not `.json`: `load_all` reads only
/// records.
pub const PROBE: &str = ".probe";

/// The read probe of `run`: a file next to its record (TMP1-F5). Only the
/// profile of this run allows to read it. It lives exactly until the
/// release of the record. No other cleanup removes it: the run folder goes
/// as soon as any paddock process lists the runs after the paddock process
/// of the run is gone, and then the end could not find its processes.
pub fn probe_file(records: &Path, run: &str) -> PathBuf {
    records.join(format!("{run}{PROBE}"))
}

/// Make the read probe of `run`. Returns its real path, as the kernel
/// checks it.
pub fn make_probe(records: &Path, run: &str) -> Result<PathBuf> {
    let p = probe_file(records, run);
    crate::privfs::write_atomic(&p, run.as_bytes())
        .with_context(|| format!("cannot write the probe of run {run}"))?;
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

/// A lock on the records, held while it lives.
struct Lock(#[allow(dead_code)] std::fs::File);

fn lock(records: &Path) -> Result<Lock> {
    use std::os::fd::AsRawFd as _;
    crate::privfs::dir(records).with_context(|| format!("cannot make {}", records.display()))?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(records.join(".lock"))?;
    // SAFETY: flock on an open file descriptor; the lock goes with the file.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Lock(f))
}

/// Every record, with a valid run name only.
pub fn load_all(records: &Path) -> Vec<Record> {
    let Ok(rd) = std::fs::read_dir(records) else {
        return vec![];
    };
    let mut out: Vec<Record> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Record>(&b).ok())
        .filter(|r| crate::view::valid_id(&r.run))
        .collect();
    out.sort_by(|a, b| a.run.cmp(&b.run));
    out
}

fn lstat(p: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(p).ok()
}

fn holder<'r>(live: &'r [Record], path: &str) -> Option<&'r Record> {
    live.iter().find(|r| {
        r.items
            .iter()
            .any(|i| i.path == path && i.kind != ItemKind::Folder)
    })
}

/// Claim the entries of `wants` for run `run`. Records of ended runs are
/// released first. Writes the record of this run before it returns.
pub fn claim(place: &Place, run: &str, pid: u32, wants: &[Resolved]) -> Result<Claim> {
    let _lock = lock(place.records)?;
    let mut live = Vec::new();
    for r in load_all(place.records) {
        if r.run == run {
            continue;
        }
        if (place.live)(r.pid) {
            live.push(r);
        } else {
            let _ = release_locked(place, &r, &[]);
        }
    }
    let mut out = Claim::default();
    let mut items = Vec::new();
    let mut stop = Vec::new();
    let mut lost_args: Vec<String> = Vec::new();
    for w in wants {
        match claim_one(place, &live, w) {
            Ok(mut got) => {
                out.granted.push(granted_line(place.root, w));
                items.append(&mut got);
                stop.extend(w.stop.iter().cloned());
                out.drop_args.extend(w.replaces_args.iter().cloned());
            }
            Err(e) => {
                out.refused.push(format!(
                    "{}/{}: not given to this run: {e:#}",
                    place.root.display(),
                    w.name
                ));
                lost_args.extend(w.replaces_args.iter().cloned());
            }
        }
    }
    out.drop_args.retain(|a| !lost_args.contains(a));
    out.drop_args.dedup();
    if !items.is_empty() {
        let rec = Record {
            run: run.to_string(),
            pid,
            root: place.root.to_path_buf(),
            items,
            stop,
            probe_read: PathBuf::new(),
            probe_write: PathBuf::new(),
            home: PathBuf::new(),
        };
        save(place.records, &rec)?;
        out.record = Some(rec);
    }
    Ok(out)
}

/// Write the record of a run.
pub fn save(records: &Path, rec: &Record) -> Result<()> {
    crate::privfs::write_atomic(
        &record_file(records, &rec.run),
        serde_json::to_vec_pretty(rec)?,
    )
    .with_context(|| format!("cannot write the /tmp record of {}", rec.run))
}

fn granted_line(root: &Path, w: &Resolved) -> String {
    let p = format!("{}/{}", root.display(), w.name);
    let what = match w.kind {
        WantKind::Dir => format!("{p}: a folder of this run only"),
        WantKind::File => format!("{p}: a file of this run only"),
        WantKind::Socket => format!("{p}: a socket of this run only"),
        WantKind::Shared => format!(
            "{p}: a folder of all runs; this run gets its own {} in it, no other",
            w.entries.join(", ")
        ),
    };
    if w.why.is_empty() {
        what
    } else {
        format!("{what} ({})", w.why)
    }
}

fn claim_one(place: &Place, live: &[Record], w: &Resolved) -> Result<Vec<Item>> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
    let path = place.root.join(&w.name);
    if w.kind != WantKind::Shared {
        if let Some(r) = holder(live, &w.name) {
            bail!(
                "run {} holds it. Two runs never share one name in /private/tmp: the second run goes without it",
                r.run
            );
        }
        if lstat(&path).is_some() {
            bail!("it exists already, and no paddock run claims it: the host or another program made it");
        }
        match w.kind {
            WantKind::Dir => std::fs::DirBuilder::new().mode(0o700).create(&path),
            WantKind::File => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map(drop),
            _ => Ok(()),
        }
        .with_context(|| format!("cannot make {}", path.display()))?;
        return Ok(vec![Item {
            path: w.name.clone(),
            kind: match w.kind {
                WantKind::Dir => ItemKind::Dir,
                WantKind::File => ItemKind::File,
                _ => ItemKind::Socket,
            },
            made: w.kind != WantKind::Socket,
        }]);
    }
    // A shared folder: first the entries, so that nothing is made for a
    // refused claim.
    let mut items = Vec::new();
    let mut leftovers: Vec<String> = Vec::new();
    for e in &w.entries {
        let rel = format!("{}/{e}", w.name);
        if let Some(r) = holder(live, &rel) {
            bail!(
                "run {} holds {e} in it (the same sandbox home). Runs never share it: the server of one run would run the commands of the other",
                r.run
            );
        }
        match lstat(&place.root.join(&rel)) {
            None => {}
            // A name built from this sandbox home that no live run holds: an
            // earlier run of this home left it (its server was stopped
            // before this claim). It goes after the folder checks below.
            Some(m) if w.home_bound && !m.file_type().is_dir() => leftovers.push(rel.clone()),
            Some(_) => bail!("{e} exists in it, and no paddock run claims it: remove it if no program of the host uses it"),
        }
        items.push(Item {
            path: rel,
            kind: ItemKind::Entry,
            made: false,
        });
    }
    let made_before = live.iter().any(|r| {
        r.items
            .iter()
            .any(|i| i.kind == ItemKind::Folder && i.path == w.name && i.made)
    });
    let mut made = made_before;
    let mut new = false;
    if lstat(&path).is_none() {
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {
                made = true;
                new = true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("cannot make {}", path.display())),
        }
    }
    let m = lstat(&path).context("it is gone")?;
    if !m.file_type().is_dir() {
        bail!("it is not a folder (a symlink or a file)");
    }
    if m.uid() != place.uid {
        bail!("its owner is uid {}, not {}", m.uid(), place.uid);
    }
    if m.mode() & 0o777 != 0o700 {
        bail!(
            "its mode is {:o}, not 700: the program refuses it, and paddock does not change a folder of the host",
            m.mode() & 0o777
        );
    }
    // The mark keeps the fact "paddock made it" when the record of the run
    // that made it goes before the folder does (TMP1-F4).
    let id = (m.dev(), m.ino());
    if new || (made && made_mark(place.records, &w.name) != Some(id)) {
        // A new folder, or one that a live run made before the marks.
        mark_made(place.records, &w.name, id);
    } else if made_mark(place.records, &w.name) == Some(id) {
        made = true;
    } else if !made {
        // A mark of an earlier folder of this name: the host made this one.
        unmark_made(place.records, &w.name);
    }
    if !leftovers.is_empty() {
        let root = crate::safefs::Root::open(place.root)?;
        for rel in &leftovers {
            root.remove_tree(Path::new(rel))
                .with_context(|| format!("cannot remove what an earlier run left: {rel}"))?;
            log::info!("/private/tmp/{rel}: removed what an earlier run of this home left");
        }
    }
    items.insert(
        0,
        Item {
            path: w.name.clone(),
            kind: ItemKind::Folder,
            made,
        },
    );
    Ok(items)
}

/// The folder of the made marks: next to the records.
fn made_dir(records: &Path) -> PathBuf {
    records.with_file_name(MADE)
}

fn made_file(records: &Path, folder: &str) -> PathBuf {
    let hex = crate::manifest::hex(&Sha256::digest(folder.as_bytes()));
    made_dir(records).join(format!("{hex}.json"))
}

/// The mark of a shared folder that a paddock run made: the device and the
/// inode of the folder. It is separate from the records, because a release
/// removes the record of the run that made the folder while another run
/// still uses it, and a record can be lost.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct MadeMark {
    folder: String,
    dev: u64,
    ino: u64,
}

/// Without the mark, the claim still holds: the folder only stays after
/// the end, as a folder of the host does.
fn mark_made(records: &Path, folder: &str, (dev, ino): (u64, u64)) {
    let m = MadeMark {
        folder: folder.to_string(),
        dev,
        ino,
    };
    let r = serde_json::to_vec_pretty(&m)
        .map_err(std::io::Error::from)
        .and_then(|b| crate::privfs::write_atomic(&made_file(records, folder), b));
    if let Err(e) = r {
        log::warn!("/private/tmp/{folder}: cannot write the made mark: {e}");
    }
}

/// The device and the inode of the folder that paddock made with this name.
fn made_mark(records: &Path, folder: &str) -> Option<(u64, u64)> {
    let b = std::fs::read(made_file(records, folder)).ok()?;
    let m: MadeMark = serde_json::from_slice(&b).ok()?;
    (m.folder == folder).then_some((m.dev, m.ino))
}

fn unmark_made(records: &Path, folder: &str) {
    let _ = std::fs::remove_file(made_file(records, folder));
}

/// Release the record of `run`: stop its processes, remove its entries and
/// the record. Returns the problems, for a warning.
pub fn release(place: &Place, run: &str) -> Vec<String> {
    let lock = match lock(place.records) {
        Ok(l) => l,
        Err(e) => return vec![format!("cannot lock the /tmp records: {e:#}")],
    };
    let all = load_all(place.records);
    let Some(rec) = all.iter().find(|r| r.run == run).cloned() else {
        return vec![];
    };
    let live: Vec<Record> = all
        .into_iter()
        .filter(|r| r.run != run && (place.live)(r.pid))
        .collect();
    let out = release_locked(place, &rec, &live);
    drop(lock);
    out
}

/// Release the records of runs whose paddock process is gone (a kill -9 of
/// paddock, a crash, a power loss): stop what they left, remove their
/// names. Every paddock command calls it, so the next command cleans up.
/// Returns the problems.
pub fn release_dead(place: &Place) -> Vec<String> {
    let all = load_all(place.records);
    if all.iter().all(|r| (place.live)(r.pid)) {
        return vec![];
    }
    let _lock = match lock(place.records) {
        Ok(l) => l,
        Err(e) => return vec![format!("cannot lock the /tmp records: {e:#}")],
    };
    let (live, dead): (Vec<Record>, Vec<Record>) = load_all(place.records)
        .into_iter()
        .partition(|r| (place.live)(r.pid));
    let mut out = Vec::new();
    for r in &dead {
        out.extend(release_locked(place, r, &live));
    }
    out
}

/// Before a start in `home`: stop the processes that earlier runs of this
/// home left (a server that lost its record) and that no live run owns.
/// `keep` are the run folders of live runs that have no record. Returns the
/// pids that were stopped and the problems.
pub fn stop_stale(place: &Place, home: &Path, keep: &[PathBuf]) -> (Vec<u32>, Vec<String>) {
    let _lock = match lock(place.records) {
        Ok(l) => l,
        Err(e) => return (vec![], vec![format!("cannot lock the /tmp records: {e:#}")]),
    };
    let mut keep: Vec<PathBuf> = keep.to_vec();
    keep.extend(
        load_all(place.records)
            .into_iter()
            .filter(|r| (place.live)(r.pid))
            .map(|r| r.probe_read),
    );
    keep.retain(|k| !k.as_os_str().is_empty());
    let p = place.procs;
    let (stopped, left) = stop_where(p, &[], &|pid| p.of_home(pid, home, &keep));
    let problems = left
        .iter()
        .map(|pid| {
            format!(
                "process {pid} of an earlier run of {} still runs after SIGKILL",
                home.display()
            )
        })
        .collect();
    (stopped, problems)
}

/// Stop every process that `pick` names, in two rounds (a server can start a
/// helper while it stops): SIGTERM to all at once, the grace time, SIGKILL to
/// the rest. Returns the stopped pids and the pids that still run.
fn stop_where(p: &dyn Procs, extra: &[u32], pick: &dyn Fn(u32) -> bool) -> (Vec<u32>, Vec<u32>) {
    use std::time::{Duration, Instant};
    let me = std::process::id();
    let wait = |pids: &[u32], d: Duration| {
        let t = Instant::now();
        while pids.iter().any(|&x| p.alive(x)) && t.elapsed() < d {
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    let mut stopped = Vec::new();
    let mut left = Vec::new();
    for _ in 0..2 {
        let mut cand = p.list();
        cand.extend_from_slice(extra);
        cand.sort_unstable();
        cand.dedup();
        let targets: Vec<u32> = cand
            .into_iter()
            .filter(|&x| x > 1 && x != me && p.alive(x) && pick(x))
            .collect();
        if targets.is_empty() {
            break;
        }
        for &t in &targets {
            p.signal(t, false);
        }
        wait(&targets, p.grace());
        for &t in targets.iter().filter(|&&x| p.alive(x)) {
            p.signal(t, true);
        }
        wait(&targets, Duration::from_secs(2));
        left = targets.iter().copied().filter(|&x| p.alive(x)).collect();
        stopped.extend(targets.into_iter().filter(|x| !left.contains(x)));
    }
    stopped.sort_unstable();
    stopped.dedup();
    (stopped, left)
}

/// Stop the processes of a run: every process of the user that runs in its
/// sandbox (the server, its updater, the MCP helpers under node), and the
/// pids of its pid files when they run there too. Returns the pids that
/// still run.
fn stop_run(p: &dyn Procs, rec: &Record) -> (Vec<u32>, Vec<u32>) {
    if rec.probe_read.as_os_str().is_empty() || rec.probe_write.as_os_str().is_empty() {
        // A record from before the start finished: no process of it ran.
        return (vec![], vec![]);
    }
    let mut extra = Vec::new();
    for f in &rec.stop {
        let Ok(Some(bytes)) = crate::safefs::read_path(f, false, 64 * 1024) else {
            continue;
        };
        extra.extend(parse_pid(&String::from_utf8_lossy(&bytes)));
    }
    stop_where(p, &extra, &|pid| p.of_run(pid, rec))
}

/// The file of the last release, for `paddock tmp debug`. Not `.json`:
/// `load_all` reads only records.
pub const LAST_RELEASE: &str = ".last-release";

/// What the last release did, for `paddock tmp debug` (TMP1-F3).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LastRelease {
    pub time: u64,
    /// The paddock process that released.
    pub by: u32,
    pub record: Record,
    pub probe_read_exists: bool,
    pub probe_write_exists: bool,
    pub stopped: Vec<u32>,
    pub left: Vec<u32>,
}

/// Release one record under the lock. When a process of the run still runs,
/// its names and the record stay: removing the socket of a live server makes
/// the next start fail. The next paddock command tries again.
fn release_locked(place: &Place, rec: &Record, live: &[Record]) -> Vec<String> {
    let records = place.records;
    let mut problems = Vec::new();
    let (stopped, left) = stop_run(place.procs, rec);
    let last = LastRelease {
        time: crate::state::now(),
        by: std::process::id(),
        record: rec.clone(),
        probe_read_exists: rec.probe_read.exists(),
        probe_write_exists: rec.probe_write.exists(),
        stopped: stopped.clone(),
        left: left.clone(),
    };
    log::debug!("/private/tmp: release {last:?}");
    if let Ok(b) = serde_json::to_vec_pretty(&last) {
        let _ = crate::privfs::write_atomic(&records.join(LAST_RELEASE), b);
    }
    if !left.is_empty() {
        problems.push(format!(
            "processes {left:?} of run {} still run after SIGKILL: its names in {} stay, and the next paddock command tries again",
            rec.run,
            rec.root.display()
        ));
        return problems;
    }
    match crate::safefs::Root::open(&rec.root) {
        Ok(root) => {
            // Entries before their folder.
            for i in rec.items.iter().rev() {
                let rel = Path::new(&i.path);
                let r = match i.kind {
                    ItemKind::Folder => {
                        let used = live.iter().any(|o| {
                            o.items
                                .iter()
                                .any(|x| x.path.starts_with(&format!("{}/", i.path)))
                        });
                        if !i.made || used {
                            continue;
                        }
                        // Only when it is empty: the host keeps its own entries there.
                        let r = match root.dir(Path::new(""), false) {
                            Ok(d) => match d.remove_dir(rel.as_os_str()) {
                                Err(e) if e.raw_os_error() == Some(libc::ENOTEMPTY) => Ok(()),
                                r => r.map_err(anyhow::Error::from),
                            },
                            Err(e) => Err(e),
                        };
                        if lstat(&rec.root.join(rel)).is_none() {
                            unmark_made(records, &i.path);
                        }
                        r
                    }
                    _ => root.remove_tree(rel),
                };
                if let Err(e) = r {
                    problems.push(format!(
                        "cannot remove {}/{}: {e:#}",
                        rec.root.display(),
                        i.path
                    ));
                }
            }
        }
        Err(e) => problems.push(format!("cannot open {}: {e:#}", rec.root.display())),
    }
    match std::fs::remove_file(record_file(records, &rec.run)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            problems.push(format!("cannot remove the /tmp record of {}: {e}", rec.run));
        }
        // The probe goes with the record, and never before it.
        _ => match std::fs::remove_file(probe_file(records, &rec.run)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                problems.push(format!("cannot remove the probe of {}: {e}", rec.run));
            }
            _ => {}
        },
    }
    problems
}

/// The pid in a pid file: a number, or JSON with a number `pid`.
pub fn parse_pid(text: &str) -> Option<u32> {
    let t = text.trim();
    let pid = t.parse::<u32>().ok().or_else(|| {
        serde_json::from_str::<serde_json::Value>(t)
            .ok()?
            .get("pid")?
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
    })?;
    (pid > 1).then_some(pid)
}

#[cfg(target_os = "macos")]
#[path = "tmpclaim/macos.rs"]
mod macos;

#[cfg(not(target_os = "macos"))]
mod macos {
    use std::path::{Path, PathBuf};
    pub fn list() -> Vec<u32> {
        vec![]
    }
    pub fn of_run(_pid: u32, _rec: &super::Record) -> anyhow::Result<bool> {
        Ok(false)
    }
    pub fn of_home(_pid: u32, _home: &Path, _keep: &[PathBuf]) -> anyhow::Result<bool> {
        Ok(false)
    }
    pub fn alive(pid: u32) -> bool {
        crate::state::alive(pid)
    }
    pub fn describe(pid: u32) -> String {
        format!("pid {pid}: no Seatbelt here")
    }
    pub fn probes(_pid: u32, _rec: &super::Record) -> String {
        String::new()
    }
}

pub mod host;

#[cfg(test)]
#[path = "tmpclaim_tests.rs"]
mod tests;

/// Seatbelt and the real `/private/tmp`. They run only on macOS.
#[cfg(all(test, target_os = "macos"))]
#[path = "tmpclaim_macos_tests.rs"]
mod macos_tests;
