//! The commands of the live grants: `paddock app share NAME
//! add|remove|mode|list`, `paddock app network NAME MODE` and `paddock app
//! clipboard NAME`. Each change runs under the lock of the grants file:
//! while the session runs it goes to the shim first (`grant ...` on the
//! control socket), and only a change that the shim took is written into
//! the profile. A refused or failed live change leaves the file as it was.
//! Then the change goes to `changes[]` and the event log.

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
}

impl Target {
    pub fn of(env: &HostEnv, name: &str) -> Result<Self> {
        if !host::valid_name(name) {
            bail!(text::bad_app_name(name));
        }
        let layout = Layout::new(env);
        let profile = layout.profile(name, DEFAULT_HOME);
        let running = host::running(&layout, name, DEFAULT_HOME);
        if !profile.dir.is_dir() && running.is_none() && crate::app::app_profile(env, name).is_err()
        {
            bail!(text::grant_no_profile(name));
        }
        Ok(Target {
            name: name.to_string(),
            profile,
            running,
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

/// Persist a live change, restoring the old live state if persistence fails.
fn update_live<T>(
    t: &Target,
    f: impl FnOnce(&mut grants::Grants, &mut Option<Op>) -> Result<T>,
) -> Result<T> {
    let mut inverse = None;
    let _lock = grants::Lock::take(&t.profile)?;
    let mut grants = grants::load(&t.profile)?;
    let result = f(&mut grants, &mut inverse).and_then(|value| {
        grants::save(&t.profile, &grants)?;
        Ok(value)
    });
    match result {
        Ok(value) => Ok(value),
        Err(e) => match inverse {
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

/// What a change prints with `--json`.
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub change: Change,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<Answer>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
    let (change, live) = update_live(&t, |g, inverse| {
        if let Some(f) = g.shares.iter().find(|f| f.path == real) {
            bail!(text::grant_already_shared(name, &real, f.read_only));
        }
        let live = t.live(&Op::ShareAdd {
            path: real.clone(),
            read_only,
        })?;
        if live.is_some() {
            *inverse = Some(Op::ShareRemove { path: real.clone() });
        }
        g.shares.push(FolderGrant {
            path: real.clone(),
            read_only,
            unprotected,
            added: now,
        });
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
    let o = Outcome {
        change,
        live,
        warnings,
    };
    print(env, &t, &o, json)
}

/// The live folder of a path, from the shim.
fn live_share(t: &Target, path: &Path, fresh: bool) -> Result<Option<LiveShare>> {
    Ok(t.live(if fresh { &Op::ListFresh } else { &Op::List })?
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
    let (change, live) = update_live(&t, |g, inverse| {
        let granted = g.shares.iter().position(|f| f.path == p);
        let live = match live_share(&t, &p, true)? {
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
                let result = t.live(&Op::ShareRemove { path: p.clone() })?;
                if result.is_some() {
                    *inverse = Some(Op::ShareAdd {
                        path: p.clone(),
                        read_only: ls.read_only,
                    });
                }
                result
            }
            None => None,
        };
        if granted.is_none() && live.is_none() {
            bail!(text::grant_not_shared(name, &p));
        }
        if let Some(i) = granted {
            g.shares.remove(i);
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
    let o = Outcome {
        change,
        live,
        warnings: Vec::new(),
    };
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
    let (change, live) = update_live(&t, |g, inverse| {
        let granted = g.shares.iter().position(|f| f.path == p);
        let live = match live_share(&t, &p, false)? {
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
                }
                let result = t.live(&Op::ShareMode {
                    path: p.clone(),
                    read_only,
                })?;
                if result.is_some() {
                    *inverse = Some(Op::ShareMode {
                        path: p.clone(),
                        read_only: ls.read_only,
                    });
                }
                result
            }
            None => None,
        };
        match granted {
            Some(i) => g.shares[i].read_only = read_only,
            None if live.is_some() => g.shares.push(FolderGrant {
                path: p.clone(),
                read_only,
                unprotected: false,
                added: now,
            }),
            None => bail!(text::grant_not_shared(name, &p)),
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
    let o = Outcome {
        change,
        live,
        warnings: Vec::new(),
    };
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
    let (change, live) = update_live(&t, |g, inverse| {
        let previous = t
            .live(&Op::List)?
            .map(|a| {
                a.network
                    .context(text::grant_missing_live_setting("network mode"))
            })
            .transpose()?;
        let live = t.live(&Op::Network {
            mode: mode.to_string(),
        })?;
        if live.is_some() {
            *inverse = Some(Op::Network {
                mode: previous.expect("running session reported its mode"),
            });
        }
        g.network = Some(mode.to_string());
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
    let o = Outcome {
        change,
        live,
        warnings: Vec::new(),
    };
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
        let g = grants::load(&t.profile)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&g.clipboard)?);
        } else {
            println!("{}", text::grant_clipboard_now(name, g.clipboard));
        }
        return Ok(0);
    }
    let now = crate::state::now();
    let (change, live) = update_live(&t, |g, inverse| {
        let new = Clipboard {
            to_app: to_app.unwrap_or(g.clipboard.to_app),
            from_app: from_app.unwrap_or(g.clipboard.from_app),
        };
        let previous = t
            .live(&Op::List)?
            .map(|a| {
                a.clipboard
                    .context(text::grant_missing_live_setting("clipboard settings"))
            })
            .transpose()?;
        let live = t.live(&Op::Clipboard(new))?;
        if live.is_some() {
            *inverse = Some(Op::Clipboard(
                previous.expect("running session reported its clipboard settings"),
            ));
        }
        g.clipboard = new;
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
    let o = Outcome {
        change,
        live,
        warnings: Vec::new(),
    };
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
/// clipboard directions, the devices, and the last grant changes, newest
/// first.
pub fn status_part(profile: &Profile, record: Option<&Session>) -> serde_json::Value {
    let g = grants::load(profile).unwrap_or_default();
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
        "changes": grants::recent(&g),
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

#[cfg(test)]
#[path = "grants_cli_tests.rs"]
mod tests;
