//! The commands of the live grants: `paddock app share NAME
//! add|remove|mode|list`, `paddock app network NAME MODE` and `paddock app
//! clipboard NAME`. Each change runs under the lock of the grants file.
//! First the new grants are checked against every rule of the save, so a
//! save that would be refused changes nothing, also not the live session.
//! While the session runs the change then goes to the shim (`grant ...` on
//! the control socket), and only a change that the shim took is written
//! into the profile. A refused or failed live change leaves the file as it
//! was. When the disk then fails the save, the live change is undone; when
//! the undo fails too, the error says so, and the session keeps the change
//! until it stops. Then the change goes to `changes[]` and the event log.

use super::grants::{self, Applies, Change, Clipboard, FolderGrant, Source, What};
use super::grants_live::{Answer, LiveShare, Op};
use super::host::{self, Session, DEFAULT_HOME};
use super::layout::Layout;
use super::proto::SHARE_ROOT;
use super::session::Profile;
use super::shares::normalize;
use super::text;
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the CLI waits for the answer of the shim.
pub const ANSWER_WAIT: Duration = Duration::from_secs(20);
/// The longest answer line.
const ANSWER_MAX: usize = 1 << 20;

/// The profile of a grants command.
pub struct Target {
    pub name: String,
    pub profile: Profile,
    pub running: Option<Session>,
    /// The session record, read again under the grants lock.
    pub record: PathBuf,
}

impl Target {
    pub fn of(env: &HostEnv, name: &str) -> Result<Self> {
        if !host::valid_name(name) {
            bail!(text::bad_app_name(name));
        }
        let layout = Layout::new(env);
        let profile = layout.profile(name, DEFAULT_HOME);
        let running = host::running(&layout, name, DEFAULT_HOME);
        let record = layout.record(name, DEFAULT_HOME);
        if !profile.dir.is_dir() && running.is_none() && crate::app::app_profile(env, name).is_err()
        {
            bail!(text::grant_no_profile(name));
        }
        Ok(Target {
            name: name.to_string(),
            profile,
            running,
            record,
        })
    }

    /// The live change, when the session runs.
    fn live(&self, op: &Op) -> Result<Option<Answer>> {
        match &self.running {
            Some(s) => ask(&s.control, op).map(Some),
            None => Ok(None),
        }
    }
}

/// The session side of one change under the grants lock.
struct Step<'a> {
    t: &'a Target,
    /// The live change that undoes the one applied, when one was applied.
    inverse: Option<Op>,
}

impl Step<'_> {
    /// Ask the session without changing it.
    fn read(&self, op: &Op) -> Result<Option<Answer>> {
        self.t.live(op)
    }

    /// Apply `op` live. `g` holds the grants as the save will write them:
    /// a save that would refuse them is refused here, before the session
    /// changes. `undo` puts the session back when the save fails later.
    fn apply(&mut self, g: &grants::Grants, op: &Op, undo: Op) -> Result<Option<Answer>> {
        grants::check_save(&self.t.profile, g)?;
        let answer = self.t.live(op)?;
        if answer.is_some() {
            self.inverse = Some(undo);
        }
        Ok(answer)
    }
}

/// Persist a live change, restoring the old live state if persistence fails.
/// `f` changes the grants before it applies the change through the step,
/// so no change goes live that the save would refuse.
fn update_live<T>(
    t: &Target,
    f: impl FnOnce(&mut grants::Grants, &mut Step) -> Result<T>,
) -> Result<T> {
    let mut step = Step { t, inverse: None };
    let _lock = grants::Lock::take(&t.profile)?;
    // A start registers its session under this lock: one that began after
    // `Target::of` must get the change live, not as a next-start change.
    if running_pid(&t.record) != t.running.as_ref().map(|s| s.pid) {
        bail!(text::grant_session_changed(&t.name));
    }
    let mut grants = grants::load(&t.profile)?;
    let result = f(&mut grants, &mut step).and_then(|value| {
        grants::save(&t.profile, &grants)?;
        Ok(value)
    });
    match result {
        Ok(value) => Ok(value),
        // The renamed policy has the change: the live state matches it.
        Err(e) if grants::unconfirmed(&e) => Err(e.context(text::grant_kept_unconfirmed())),
        Err(e) => match step.inverse {
            Some(op) => match t.live(&op) {
                Ok(_) => Err(e.context(text::grant_reverted())),
                Err(rollback) => {
                    Err(e.context(text::grant_revert_failed(&format!("{rollback:#}"))))
                }
            },
            None => Err(e),
        },
    }
}

fn running_pid(record: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(record).ok()?;
    let s: Session = serde_json::from_str(&text).ok()?;
    host::alive(s.pid).then_some(s.pid)
}

/// One `grant` line to the shim and its answer.
pub fn ask(control: &Path, op: &Op) -> Result<Answer> {
    let mut c =
        UnixStream::connect(control).with_context(|| text::grant_session_unreachable(control))?;
    c.set_read_timeout(Some(ANSWER_WAIT)).ok();
    c.write_all(format!("{}\n", op.line()).as_bytes())
        .with_context(|| text::grant_session_unreachable(control))?;
    let line = crate::vm::proto::read_raw_line(&mut c, ANSWER_MAX)
        .with_context(|| text::grant_no_answer(control))?
        .with_context(|| text::grant_no_answer(control))?;
    parse_answer(&line)
}

/// The answer line of the shim: `ok JSON` or `error TEXT`.
pub fn parse_answer(line: &str) -> Result<Answer> {
    if let Some(e) = line.strip_prefix("error ") {
        bail!(e.to_string());
    }
    let json = line
        .strip_prefix("ok ")
        .with_context(|| text::grant_bad_answer(line))?;
    serde_json::from_str(json).with_context(|| text::grant_bad_answer(line))
}

/// What a change prints with `--json`. `v` and `ok` as in the other
/// launcher answers.
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub v: u32,
    pub ok: bool,
    pub change: Change,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<Answer>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Outcome {
    pub fn new(change: Change, live: Option<Answer>, warnings: Vec<String>) -> Self {
        Outcome {
            v: 1,
            ok: true,
            change,
            live,
            warnings,
        }
    }
}

fn mode_word(read_only: bool) -> &'static str {
    if read_only {
        "ro"
    } else {
        "rw"
    }
}

fn applies_of(answer: &Option<Answer>) -> Applies {
    answer
        .as_ref()
        .map(|a| a.applies)
        .unwrap_or(Applies::NextStart)
}

/// A folder named on the command line: absolute, and the real path when
/// it exists, as the shares hold it.
pub fn named_folder(path: &Path, cwd: &Path) -> PathBuf {
    let p = normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    });
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn print(env: &HostEnv, t: &Target, o: &Outcome, json: bool) -> Result<i32> {
    if json {
        println!("{}", serde_json::to_string_pretty(o)?);
        return Ok(0);
    }
    for w in &o.warnings {
        eprintln!("{w}");
    }
    println!(
        "{}",
        text::grant_done(&t.name, &describe(env, &o.change), o)
    );
    Ok(0)
}

fn describe(env: &HostEnv, c: &Change) -> String {
    match c.what {
        What::Folder => grants::tilde(Path::new(&c.target), &env.home),
        _ => c.target.clone(),
    }
}

/// `paddock app share NAME add PATH [--rw] [--unprotected]`.
pub fn share_add(
    env: &HostEnv,
    name: &str,
    path: &Path,
    rw: bool,
    unprotected: bool,
    json: bool,
) -> Result<i32> {
    let t = Target::of(env, name)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| env.home.clone());
    let unprotected = unprotected || crate::app::unprotected_workspaces(env, name).unwrap_or(false);
    let (real, warnings) = grants::check_folder(
        name,
        path,
        &cwd,
        &env.home,
        &grants::paddock_dirs(env),
        unprotected,
    )?;
    let read_only = !rw;
    let now = crate::state::now();
    let (change, live) = update_live(&t, |g, step| {
        if let Some(f) = g.shares.iter().find(|f| f.path == real) {
            bail!(text::grant_already_shared(name, &real, f.read_only));
        }
        g.shares.push(FolderGrant {
            path: real.clone(),
            read_only,
            unprotected,
            added: now,
        });
        let live = step.apply(
            g,
            &Op::ShareAdd {
                path: real.clone(),
                read_only,
            },
            Op::ShareRemove { path: real.clone() },
        )?;
        let c = Change::new(
            now,
            What::Folder,
            "add",
            &real.display().to_string(),
            mode_word(read_only),
            applies_of(&live),
            Source::You,
        );
        g.changes.push(c.clone());
        Ok((c, live))
    })?;
    grants::event(&t.profile, &change, &env.home);
    let o = Outcome::new(change, live, warnings);
    print(env, &t, &o, json)
}

/// The live folder of a path, from the shim.
fn live_share(step: &Step, path: &Path, fresh: bool) -> Result<Option<LiveShare>> {
    Ok(step
        .read(if fresh { &Op::ListFresh } else { &Op::List })?
        .and_then(|a| a.shares.into_iter().find(|s| s.path == path)))
}

/// `paddock app share NAME remove PATH [--force]`. A folder in which the
/// app holds files open is removed only with `--force`.
pub fn share_remove(
    env: &HostEnv,
    name: &str,
    path: &Path,
    force: bool,
    json: bool,
) -> Result<i32> {
    let t = Target::of(env, name)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| env.home.clone());
    let p = named_folder(path, &cwd);
    let now = crate::state::now();
    let (change, live) = update_live(&t, |g, step| {
        let granted = g.shares.iter().position(|f| f.path == p);
        if let Some(i) = granted {
            g.shares.remove(i);
        }
        let live = match live_share(step, &p, true)? {
            Some(ls) => {
                let open = match ls.open_handles {
                    Some(n) => n,
                    None if !force => bail!(text::grant_open_unknown(&p)),
                    None => 0,
                };
                if open > 0 && !force {
                    bail!(text::grant_remove_open(
                        name,
                        &grants::tilde(&p, &env.home),
                        open
                    ));
                }
                step.apply(
                    g,
                    &Op::ShareRemove { path: p.clone() },
                    Op::ShareAdd {
                        path: p.clone(),
                        read_only: ls.read_only,
                    },
                )?
            }
            None => None,
        };
        if granted.is_none() && live.is_none() {
            bail!(text::grant_not_shared(name, &p));
        }
        let c = Change::new(
            now,
            What::Folder,
            "remove",
            &p.display().to_string(),
            "-",
            applies_of(&live),
            Source::You,
        );
        g.changes.push(c.clone());
        Ok((c, live))
    })?;
    grants::event(&t.profile, &change, &env.home);
    let o = Outcome::new(change, live, Vec::new());
    print(env, &t, &o, json)
}

/// `paddock app share NAME mode PATH ro|rw`. A folder of `app run` that
/// the session shares becomes a grant with the new mode, so the next
/// start keeps it.
pub fn share_mode(
    env: &HostEnv,
    name: &str,
    path: &Path,
    read_only: bool,
    json: bool,
) -> Result<i32> {
    let t = Target::of(env, name)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| env.home.clone());
    let p = named_folder(path, &cwd);
    let now = crate::state::now();
    let (change, live) = update_live(&t, |g, step| {
        let granted = g.shares.iter().position(|f| f.path == p);
        if let Some(i) = granted {
            g.shares[i].read_only = read_only;
        }
        let live = match live_share(step, &p, false)? {
            Some(ls) => {
                if granted.is_none() {
                    grants::check_folder(
                        name,
                        &p,
                        &cwd,
                        &env.home,
                        &grants::paddock_dirs(env),
                        false,
                    )?;
                    // A folder of `app run` becomes a grant: only a
                    // running session lists one.
                    g.shares.push(FolderGrant {
                        path: p.clone(),
                        read_only,
                        unprotected: false,
                        added: now,
                    });
                }
                step.apply(
                    g,
                    &Op::ShareMode {
                        path: p.clone(),
                        read_only,
                    },
                    Op::ShareMode {
                        path: p.clone(),
                        read_only: ls.read_only,
                    },
                )?
            }
            None => None,
        };
        if granted.is_none() && live.is_none() {
            bail!(text::grant_not_shared(name, &p));
        }
        let c = Change::new(
            now,
            What::Folder,
            "mode",
            &p.display().to_string(),
            mode_word(read_only),
            applies_of(&live),
            Source::You,
        );
        g.changes.push(c.clone());
        Ok((c, live))
    })?;
    grants::event(&t.profile, &change, &env.home);
    let o = Outcome::new(change, live, Vec::new());
    print(env, &t, &o, json)
}

/// One row of `paddock app share NAME list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub path: PathBuf,
    /// The path with `~` for the home folder.
    pub shown: String,
    /// `ro` or `rw`.
    pub mode: String,
    /// `live`, `reopen-files` or `next-start`.
    pub status: Applies,
    /// Where the app sees it while the session runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest: Option<PathBuf>,
    /// The files that the app holds open in it, as the guest last counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_handles: Option<u32>,
    /// The last time the guest counted an open file in it, seconds since
    /// the epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_use: Option<u64>,
    /// `grant` (kept in the profile) or `run` (named on `app run` only).
    pub from: &'static str,
}

/// The downloads folder: a share with a fixed name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Downloads {
    pub path: PathBuf,
    pub shown: String,
    pub guest: PathBuf,
}

/// `paddock app share NAME list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub app: String,
    pub running: bool,
    pub folders: Vec<Row>,
    pub downloads: Downloads,
    pub clipboard: Clipboard,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Why the live counts are missing, when the session did not answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// The rows of the folders: the live folders first, then the grants
/// that the session does not share (they apply at the next start).
pub fn rows(home: &Path, g: &grants::Grants, live: Option<&[LiveShare]>) -> Vec<Row> {
    let mut out = Vec::new();
    for s in live.unwrap_or_default() {
        out.push(Row {
            path: s.path.clone(),
            shown: grants::tilde(&s.path, home),
            mode: mode_word(s.read_only).into(),
            status: s.applies.unwrap_or(Applies::Live),
            guest: Some(s.guest.clone()),
            open_handles: s.open_handles,
            last_use: s.last_use,
            from: if g.shares.iter().any(|f| f.path == s.path) {
                "grant"
            } else {
                "run"
            },
        });
    }
    for f in &g.shares {
        if out.iter().any(|r| r.path == f.path) {
            continue;
        }
        out.push(Row {
            path: f.path.clone(),
            shown: grants::tilde(&f.path, home),
            mode: mode_word(f.read_only).into(),
            status: Applies::NextStart,
            guest: None,
            open_handles: None,
            last_use: None,
            from: "grant",
        });
    }
    out
}

/// The live folders from the session record, when the shim does not
/// answer.
pub fn record_rows(s: &Session) -> Vec<LiveShare> {
    let now = crate::state::now();
    s.shares
        .iter()
        .map(|r| LiveShare {
            name: r.name.clone(),
            path: r.path.clone(),
            read_only: r.read_only,
            guest: Path::new(SHARE_ROOT).join(&r.name),
            open_handles: r.open_handles,
            last_use: r.last_use,
            applies: Some(if r.reopen_until.is_some_and(|u| u > now) {
                Applies::ReopenFiles
            } else {
                Applies::Live
            }),
        })
        .collect()
}

/// `paddock app share NAME list [--json]`.
pub fn share_list(env: &HostEnv, name: &str, json: bool) -> Result<i32> {
    let t = Target::of(env, name)?;
    let g = grants::load(&t.profile)?;
    let mut warning = None;
    let live = match &t.running {
        None => None,
        Some(s) => match ask(&s.control, &Op::List) {
            Ok(a) => Some(a.shares),
            Err(e) => {
                warning = Some(text::grant_list_from_record(&format!("{e:#}")));
                Some(record_rows(s))
            }
        },
    };
    let downloads = t.profile.downloads();
    let l = Listing {
        app: name.to_string(),
        running: t.running.is_some(),
        folders: rows(&env.home, &g, live.as_deref()),
        downloads: Downloads {
            shown: grants::tilde(&downloads, &env.home),
            path: downloads,
            guest: Path::new(SHARE_ROOT).join("downloads"),
        },
        clipboard: g.clipboard,
        network: t
            .running
            .as_ref()
            .map(|s| s.network_mode.clone())
            .or(g.network.clone()),
        warning,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&l)?);
        return Ok(0);
    }
    if let Some(w) = &l.warning {
        eprintln!("{w}");
    }
    print!("{}", text::grant_list(&l));
    Ok(0)
}

/// `paddock app network NAME ask|allowlist|none`.
pub fn network(env: &HostEnv, name: &str, mode: &str, json: bool) -> Result<i32> {
    if !grants::NETWORK_MODES.contains(&mode) {
        bail!(text::grant_bad_network(mode));
    }
    let t = Target::of(env, name)?;
    let now = crate::state::now();
    let (change, live) = update_live(&t, |g, step| {
        let previous = step
            .read(&Op::List)?
            .map(|a| {
                a.network
                    .context(text::grant_missing_live_setting("network mode"))
            })
            .transpose()?;
        g.network = Some(mode.to_string());
        let live = match previous {
            Some(previous) => step.apply(
                g,
                &Op::Network {
                    mode: mode.to_string(),
                },
                Op::Network { mode: previous },
            )?,
            None => None,
        };
        let c = Change::new(
            now,
            What::Network,
            "set",
            mode,
            mode,
            applies_of(&live),
            Source::You,
        );
        g.changes.push(c.clone());
        Ok((c, live))
    })?;
    grants::event(&t.profile, &change, &env.home);
    let o = Outcome::new(change, live, Vec::new());
    print(env, &t, &o, json)
}

/// The value word of a clipboard change: `to-app on, from-app off`.
pub fn clipboard_value(c: Clipboard) -> String {
    let w = |b: bool| if b { "on" } else { "off" };
    format!("to-app {}, from-app {}", w(c.to_app), w(c.from_app)) // not user text
}

/// `paddock app clipboard NAME [--to-app on|off] [--from-app on|off]`.
/// Without a flag it prints the two directions.
pub fn clipboard(
    env: &HostEnv,
    name: &str,
    to_app: Option<bool>,
    from_app: Option<bool>,
    json: bool,
) -> Result<i32> {
    let t = Target::of(env, name)?;
    if to_app.is_none() && from_app.is_none() {
        if json {
            // The saved directions, also as `to_app` and `from_app` at the
            // top for older readers, and the session's while it runs.
            println!(
                "{}",
                serde_json::to_string_pretty(&clipboard_state(env, name)?)?
            );
        } else {
            let g = grants::load(&t.profile)?;
            println!("{}", text::grant_clipboard_now(name, g.clipboard));
        }
        return Ok(0);
    }
    let now = crate::state::now();
    let (change, live) = update_live(&t, |g, step| {
        let new = Clipboard {
            to_app: to_app.unwrap_or(g.clipboard.to_app),
            from_app: from_app.unwrap_or(g.clipboard.from_app),
        };
        let previous = step
            .read(&Op::List)?
            .map(|a| {
                a.clipboard
                    .context(text::grant_missing_live_setting("clipboard settings"))
            })
            .transpose()?;
        g.clipboard = new;
        let live = match previous {
            Some(previous) => step.apply(g, &Op::Clipboard(new), Op::Clipboard(previous))?,
            None => None,
        };
        let c = Change::new(
            now,
            What::Clipboard,
            "set",
            "clipboard",
            &clipboard_value(new),
            applies_of(&live),
            Source::You,
        );
        g.changes.push(c.clone());
        Ok((c, live))
    })?;
    grants::event(&t.profile, &change, &env.home);
    let o = Outcome::new(change, live, Vec::new());
    print(env, &t, &o, json)
}

/// A device row of `paddock app status --json`. paddock's macOS guest
/// attaches no camera, microphone or USB device, and the VM can attach
/// one only at its start: every row says `next-start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    pub name: &'static str,
    pub granted: bool,
    pub applies: Applies,
}

/// The devices of an app session.
pub const DEVICES: [&str; 3] = ["camera", "microphone", "usb"];

/// The grants part of `paddock app status --json`: the open file handles
/// per shared folder (path to count, while the session runs), the
/// clipboard directions, the devices, and the last grant changes. Effects
/// with unknown current state have their own list; neither list proves a
/// complete history across old writers or lost sidecars.
pub fn status_part(profile: &Profile, record: Option<&Session>) -> serde_json::Value {
    let g = grants::load(profile).unwrap_or_default();
    let (changes, historical_changes) = grants::recent_compatible(&g);
    let handles: serde_json::Map<String, serde_json::Value> = record
        .map(|r| {
            r.shares
                .iter()
                .filter_map(|s| {
                    s.open_handles
                        .map(|n| (s.path.display().to_string(), serde_json::json!(n)))
                })
                .collect()
        })
        .unwrap_or_default();
    let devices: Vec<Device> = DEVICES
        .iter()
        .map(|name| Device {
            name,
            granted: false,
            applies: Applies::NextStart,
        })
        .collect();
    serde_json::json!({
        "open_handles_per_share": handles,
        "clipboard": {
            "to_app": g.clipboard.to_app,
            "from_app": g.clipboard.from_app,
            "applies": if record.is_some() { Applies::Live } else { Applies::NextStart },
        },
        "devices": devices,
        "changes": changes,
        "historical_changes": historical_changes,
        "changes_complete": false,
        "changes_history": g.history_state,
    })
}

/// Put the grants part into the JSON of `paddock app status`.
pub fn extend_status(v: &mut serde_json::Value, profile: &Profile, record: Option<&Session>) {
    if let (Some(obj), serde_json::Value::Object(part)) =
        (v.as_object_mut(), status_part(profile, record))
    {
        obj.extend(part);
    }
}

#[path = "grants_session.rs"]
mod session;
pub use session::{
    clipboard_session, clipboard_show, clipboard_state, share_keep, ClipboardState, SessionOutcome,
};

#[cfg(test)]
#[path = "grants_cli_tests.rs"]
mod tests;
