//! The copy of the home onto the data volume at `data.prepare`, as a plan
//! of small parts. One `ditto` of the whole home ran into the 90 s limit
//! of a configure command on a fresh macOS 27.2 guest: the home holds
//! folders that macOS guards with its privacy prompts (TCC), and a read of
//! them by a process without the permission is refused file by file or
//! waits on a prompt that nobody answers. So:
//!
//! 1. `du -sk` measures the parts that are not guarded; the whole copy
//!    gets [`copy_limit`] of that size, at least 60 s.
//! 2. Each part of the home is one `ditto` of its own; the `Library`
//!    folder is split into its entries. A guarded folder ([`GUARDED_HOME`],
//!    [`GUARDED_LIBRARY`]) gets [`GUARD_LIMIT`]: when it does not end in
//!    that time, it is left as an empty folder with its mode, and the copy
//!    goes on with a warning. A symbolic link is made again as a link,
//!    never followed; the Downloads link to the host share is skipped,
//!    because `data.prepare` makes it again on the volume.
//! 3. A file that `ditto` cannot read (`Operation not permitted`,
//!    `Permission denied`) is a warning with a count, not a failure. Any
//!    other error of `ditto` fails the copy.
//! 4. After each part the progress goes to the host with the time the
//!    rest may take, so the host waits as long as the copy may run.
//!
//! The plan and the executor are pure over a runner, so the tests run on
//! Linux; `data.rs` runs them with `sudo` in the guest.

use super::configure::{Plan, Ran};
use super::platform::Fail;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The least time of the whole copy.
pub const BASE_LIMIT: Duration = Duration::from_secs(60);
/// The copy may take one second more per this many bytes.
pub const BYTES_PER_SECOND: u64 = 50_000_000;
/// The longest copy, whatever the size.
pub const MAX_LIMIT: Duration = Duration::from_secs(4 * 3600);
/// The time of the copy when `du` could not measure the home.
pub const UNKNOWN_LIMIT: Duration = Duration::from_secs(30 * 60);
/// The most time that one guarded folder may take.
pub const GUARD_LIMIT: Duration = Duration::from_secs(20);
/// The most time of the `du` that measures the home.
pub const MEASURE_LIMIT: Duration = Duration::from_secs(90);
/// The host gets a progress line at most this often.
pub const PROGRESS_EVERY: Duration = Duration::from_secs(5);

/// The folders at the top of the home that the privacy prompts guard.
pub const GUARDED_HOME: &[&str] = &["Desktop", "Documents", "Downloads"];
/// The folders in `~/Library` that macOS guards: the data of other apps
/// (a prompt) and the data vaults (refused).
pub const GUARDED_LIBRARY: &[&str] = &[
    "Accounts",
    "Biome",
    "Calendars",
    "Containers",
    "Cookies",
    "Daemon Containers",
    "Group Containers",
    "HomeKit",
    "IdentityServices",
    "Mail",
    "Messages",
    "Metadata",
    "Mobile Documents",
    "PersonalizationPortrait",
    "Reminders",
    "Safari",
    "Sharing",
    "Suggestions",
];

/// What an entry of a folder is. A link is never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Link,
}

/// One entry of the home or of its `Library`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// The permission bits, for a folder that is left empty.
    pub mode: u32,
    /// Where a link points.
    pub target: Option<PathBuf>,
}

/// One step of the copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Make a folder with this mode.
    Dir { dst: PathBuf, mode: u32 },
    /// `ditto src dst`. A guarded part has [`GUARD_LIMIT`] and is left as
    /// an empty folder with `mode` when it does not end.
    Copy {
        src: PathBuf,
        dst: PathBuf,
        guarded: bool,
        mode: u32,
    },
    /// `ln -s target dst`.
    Link { dst: PathBuf, target: PathBuf },
}

/// The plan of the copy of `home` (its entries and those of its
/// `Library`) into `to`, and the sources that `du` measures. Pure.
pub fn plan(
    home: &Path,
    to: &Path,
    home_mode: u32,
    entries: &[Entry],
    library: Option<(u32, &[Entry])>,
) -> (Vec<Op>, Vec<PathBuf>) {
    let mut ops = vec![Op::Dir {
        dst: to.to_path_buf(),
        mode: home_mode,
    }];
    let mut measure = Vec::new();
    for e in entries {
        if e.name == "Library" && e.kind == Kind::Dir {
            if let Some((mode, inner)) = library {
                let (lib_src, lib_dst) = (home.join("Library"), to.join("Library"));
                ops.push(Op::Dir {
                    dst: lib_dst.clone(),
                    mode,
                });
                for i in inner {
                    let guarded = GUARDED_LIBRARY.contains(&i.name.as_str());
                    part(&lib_src, &lib_dst, i, guarded, &mut ops, &mut measure);
                }
                continue;
            }
        }
        if e.name == "Downloads" && e.kind == Kind::Link {
            // `data.prepare` links Downloads to the host share again.
            continue;
        }
        let guarded = GUARDED_HOME.contains(&e.name.as_str());
        part(home, to, e, guarded, &mut ops, &mut measure);
    }
    (ops, measure)
}

fn part(
    from: &Path,
    to: &Path,
    e: &Entry,
    guarded: bool,
    ops: &mut Vec<Op>,
    measure: &mut Vec<PathBuf>,
) {
    let (src, dst) = (from.join(&e.name), to.join(&e.name));
    match (e.kind, &e.target) {
        (Kind::Link, Some(target)) => ops.push(Op::Link {
            dst,
            target: target.clone(),
        }),
        // A link that cannot be read: nothing to copy.
        (Kind::Link, None) => {}
        _ => {
            if !guarded {
                measure.push(src.clone());
            }
            ops.push(Op::Copy {
                src,
                dst,
                guarded,
                mode: e.mode,
            });
        }
    }
}

/// The KiB of each path in the output of `du -sk PATH...`: the lines
/// `N<tab>PATH`. The error lines of `du` are skipped. Pure.
pub fn du_sizes(out: &str) -> BTreeMap<PathBuf, u64> {
    out.lines()
        .filter_map(|l| {
            let (n, path) = l.split_once('\t')?;
            Some((PathBuf::from(path), n.trim().parse().ok()?))
        })
        .collect()
}

/// The time of the whole copy for `bytes` (`None`: not measured):
/// [`BASE_LIMIT`] plus one second per [`BYTES_PER_SECOND`], at most
/// [`MAX_LIMIT`]. Pure.
pub fn copy_limit(bytes: Option<u64>) -> Duration {
    match bytes {
        None => UNKNOWN_LIMIT,
        Some(b) => (BASE_LIMIT + Duration::from_secs(b / BYTES_PER_SECOND)).min(MAX_LIMIT),
    }
}

/// The errors in the output of one `ditto`: the files that it could not
/// read, and the other error lines. Pure.
pub fn ditto_errors(out: &str) -> (usize, Vec<String>) {
    let mut denied = 0;
    let mut other = Vec::new();
    for l in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if l.ends_with("Operation not permitted") || l.ends_with("Permission denied") {
            denied += 1;
        } else if l.starts_with("ditto:") {
            other.push(l.to_string());
        }
    }
    (denied, other)
}

/// The most times that one part is copied when `ditto` is interrupted.
pub const COPY_TRIES: u32 = 3;

/// Whether the output of a `ditto` names only an interrupted call, which
/// a second try can get past. Pure.
pub fn transient(out: &str) -> bool {
    let (_, other) = ditto_errors(out);
    !other.is_empty() && other.iter().all(|l| l.ends_with("Interrupted system call"))
}

/// What the copy did, for the log and the reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub parts: usize,
    /// The files that could not be read.
    pub denied: usize,
    /// The guarded folders that did not end in [`GUARD_LIMIT`] and are
    /// empty on the volume.
    pub skipped: Vec<PathBuf>,
}

/// The cancel flag and the progress line of a running `data.prepare`.
pub struct Watch {
    pub cancel: Arc<AtomicBool>,
    /// A line for the host and the time that the rest may take.
    pub progress: Box<dyn Fn(String, Duration) + Send + Sync>,
}

impl Watch {
    /// A watch that nobody cancels and that reports nowhere.
    pub fn quiet() -> Self {
        Watch {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Box::new(|_, _| {}),
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// `Err` with the cancel text once the flag is set.
    pub fn check(&self) -> Result<(), Fail> {
        if self.cancelled() {
            return Err((ErrorCode::Io, text::data_prepare_cancelled()));
        }
        Ok(())
    }
}

/// Run the ops in order with `run` (a plan and its limit), all within
/// `limit`. `sizes` are the bytes of the parts that `du` measured, for the
/// progress. A cancel stops before the next part and kills the running
/// one (the runner watches the flag).
pub fn execute(
    ops: &[Op],
    sizes: &BTreeMap<PathBuf, u64>,
    limit: Duration,
    watch: &Watch,
    mut run: impl FnMut(&Plan, Duration) -> Result<Ran, Fail>,
) -> Result<Report, Fail> {
    let start = Instant::now();
    let total: u64 = sizes.values().sum();
    let mut done_bytes = 0u64;
    let mut report = Report::default();
    let mut last_progress: Option<Instant> = None;
    let copies = ops.iter().filter(|o| matches!(o, Op::Copy { .. })).count();
    for op in ops {
        watch.check()?;
        let left = limit.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Err((ErrorCode::Timeout, text::data_copy_timed_out(limit)));
        }
        match op {
            Op::Dir { dst, mode } => mkdir(dst, *mode, left, &mut run)?,
            Op::Link { dst, target } => {
                let argv = vec!["/bin/ln".into(), "-s".into(), path(target), path(dst)];
                let r = run(&root(argv), left)?;
                if !r.ok() {
                    log::warn!("copy part {}: the link failed: {}", dst.display(), r.output);
                }
            }
            Op::Copy {
                src,
                dst,
                guarded,
                mode,
            } => {
                let wait = if *guarded {
                    left.min(GUARD_LIMIT)
                } else {
                    left
                };
                let t = Instant::now();
                let argv = vec!["/usr/bin/ditto".into(), path(src), path(dst)];
                let mut r = run(&root(argv.clone()), wait)?;
                watch.check()?;
                // A signal can cut a read short (EINTR); the part is copied
                // again, up to COPY_TRIES times, onto what is already there.
                let mut tries = 1;
                while !r.ok() && !r.timed_out && transient(&r.output) && tries < COPY_TRIES {
                    tries += 1;
                    log::info!("copy part {}: interrupted, try {tries}", src.display());
                    r = run(&root(argv.clone()), wait)?;
                    watch.check()?;
                }
                let (denied, other) = ditto_errors(&r.output);
                report.denied += denied;
                report.parts += 1;
                log::info!(
                    "copy part {}: exit {:?}, {} ms, timed out {}, {} not readable",
                    src.display(),
                    r.code,
                    t.elapsed().as_millis(),
                    r.timed_out,
                    denied
                );
                if r.timed_out && *guarded {
                    log::warn!(
                        "copy part {}: no end in {} s, left empty",
                        src.display(),
                        wait.as_secs()
                    );
                    report.skipped.push(src.clone());
                    // What `ditto` left half done goes; the folder stays.
                    let _ = run(&root(vec!["/bin/rm".into(), "-rf".into(), path(dst)]), wait);
                    mkdir(dst, *mode, limit.saturating_sub(start.elapsed()), &mut run)?;
                } else if r.timed_out {
                    return Err((ErrorCode::Timeout, text::data_copy_timed_out(limit)));
                } else if !r.ok() && (!other.is_empty() || denied == 0) {
                    let why = other.first().cloned().unwrap_or(r.output);
                    return Err((ErrorCode::Io, text::data_copy_failed(&path(src), &why)));
                }
                done_bytes += sizes.get(src).copied().unwrap_or(0);
                let due = last_progress.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY);
                if due || report.parts == copies {
                    last_progress = Some(Instant::now());
                    (watch.progress)(
                        text::data_copy_progress(report.parts, copies, done_bytes, total),
                        limit.saturating_sub(start.elapsed()),
                    );
                }
            }
        }
    }
    Ok(report)
}

fn mkdir(
    dst: &Path,
    mode: u32,
    left: Duration,
    run: &mut impl FnMut(&Plan, Duration) -> Result<Ran, Fail>,
) -> Result<(), Fail> {
    let make = run(
        &root(vec!["/bin/mkdir".into(), "-p".into(), path(dst)]),
        left,
    )?;
    if !make.ok() {
        return Err((
            ErrorCode::Io,
            text::data_copy_failed(&path(dst), &make.output),
        ));
    }
    let chmod = vec![
        "/bin/chmod".into(),
        format!("{:o}", mode & 0o7777),
        path(dst),
    ];
    let _ = run(&root(chmod), left)?;
    Ok(())
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A command as root: `sudo -S -p '' -- ARGV`, the password on stdin.
fn root(argv: Vec<String>) -> Plan {
    let mut v = vec![
        super::configure::SUDO.to_string(),
        "-S".into(),
        "-p".into(),
        String::new(),
        "--".into(),
    ];
    v.extend(argv);
    Plan {
        argv: v,
        password_on_stdin: true,
    }
}

/// The entries of a folder, links not followed. An entry that cannot be
/// read is skipped with a log line.
pub fn read_entries(dir: &Path) -> std::io::Result<Vec<Entry>> {
    use std::os::unix::fs::PermissionsExt;
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            log::warn!("copy: cannot read {}", e.path().display());
            continue;
        };
        let ft = meta.file_type();
        let kind = if ft.is_symlink() {
            Kind::Link
        } else if ft.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            kind,
            mode: meta.permissions().mode(),
            target: if kind == Kind::Link {
                std::fs::read_link(e.path()).ok()
            } else {
                None
            },
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
#[path = "data_copy_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "data_copy_macos_tests.rs"]
mod macos_tests;
