//! One app session on the macOS guest: the profile folder, its stamp, the
//! decision of how the next start runs (`decide`), the files it makes
//! (`prepare`), and the one function that removes files of a profile
//! (`remove`). Plain Rust: the tests run on every system. The clone of
//! the golden disk and the machine identifier are closures, so the Linux
//! tests pass a copy and some bytes.
//!
//! Every file is written as `<name>.partial` and renamed when complete, and
//! the stamp is written last. A stopped or crashed paddock leaves a folder
//! that `decide` reads as `Repair`, never as a half state that boots.

use super::golden;
use super::layout::{self, Golden};
use super::proto::Version;
use super::text;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use super::layout::{commit, partial, safe_name, PARTIAL};
pub use super::migrate::{migrate, stale_reason, Migration};
pub use super::pair::{data_formatted, golden_note, Refusal};
pub use super::reset::{reset, reset_removals, Reset};

/// The stamp schema this paddock writes and reads.
pub const SCHEMA: u32 = 1;
/// The data disk of a profile: the guest home, sparse.
pub const DATA_DISK_BYTES: u64 = 20 << 30;
/// Free space on the volume before a clone of the golden.
pub const CLONE_NEEDS: u64 = 8 << 30;
/// Free space on the volume before a save, on top of the RAM size.
pub const SAVE_EXTRA: u64 = 1 << 30;
/// The environment variable that replaces the free space of the volume
/// in the checks and in the Mac scripts.
pub const FAKE_FREE_VAR: &str = "PADDOCK_MACVM_FAKE_FREE";

/// The files of one profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub dir: PathBuf,
}

impl Profile {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn lock(&self) -> PathBuf {
        self.dir.join("profile.lock")
    }
    /// The clone of the golden disk, writable.
    pub fn system(&self) -> PathBuf {
        self.dir.join("system.img")
    }
    /// The copy of the golden auxiliary storage (NVRAM).
    pub fn aux(&self) -> PathBuf {
        self.dir.join("aux.img")
    }
    /// The machine identifier of this profile.
    pub fn machine_id(&self) -> PathBuf {
        self.dir.join("machine-id")
    }
    /// The data disk: the guest home. It survives everything.
    pub fn data(&self) -> PathBuf {
        self.dir.join("data.img")
    }
    /// The system set: the files that the data disk is bound to. They go
    /// only with the data disk.
    pub fn system_set(&self) -> [PathBuf; 3] {
        [self.system(), self.aux(), self.machine_id()]
    }
    /// The disk pair with its machine identifier and the stamp: what a
    /// backup holds and a restore puts back.
    pub fn pair(&self) -> [PathBuf; 5] {
        [
            self.data(),
            self.system(),
            self.aux(),
            self.machine_id(),
            self.stamp(),
        ]
    }
    /// The saved machine state of the last quit.
    pub fn state(&self) -> PathBuf {
        self.dir.join("state.vzs")
    }
    pub fn stamp(&self) -> PathBuf {
        self.dir.join("stamp.json")
    }
    pub fn drop_dir(&self) -> PathBuf {
        self.dir.join("drop")
    }
    pub fn downloads(&self) -> PathBuf {
        self.dir.join("downloads")
    }
    pub fn apps(&self) -> PathBuf {
        self.dir.join("apps")
    }
    pub fn backups(&self) -> PathBuf {
        self.dir.join("backups")
    }
    pub fn logs(&self) -> PathBuf {
        self.dir.join("logs")
    }
    pub fn console_log(&self) -> PathBuf {
        self.logs().join("console.log")
    }
    pub fn shim_log(&self) -> PathBuf {
        self.logs().join("shim.log")
    }
    pub fn events_log(&self) -> PathBuf {
        self.logs().join("events.jsonl")
    }
    /// The agent log of the guest, fetched at the quit.
    pub fn agent_log(&self) -> PathBuf {
        self.logs().join("agent.log")
    }
    /// The denials of the session: the frames the stub network dropped
    /// and the targets the proxy refused.
    pub fn boundary_log(&self) -> PathBuf {
        self.logs().join(crate::boundary::FILE)
    }
    /// The connection log of the firewall: one line per decision
    /// of the session. A new session rotates the last one to `.1`.
    pub fn connections_log(&self) -> PathBuf {
        self.logs().join(super::status::CONNECTIONS_FILE)
    }
    /// The folders that `Fresh` makes next to the disks.
    pub fn folders(&self) -> [PathBuf; 5] {
        [
            self.drop_dir(),
            self.downloads(),
            self.apps(),
            self.backups(),
            self.logs(),
        ]
    }
}

/// The display of the session in pixels and its scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub w: u32,
    pub h: u32,
    pub scale: u32,
}

impl fmt::Display for Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}@{}", self.w, self.h, self.scale)
    }
}

impl std::str::FromStr for Display {
    type Err = String;
    /// `WxH@S`, for example `2880x1800@2`.
    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || text::display_arg_invalid(text);
        let (size, scale) = text.split_once('@').ok_or_else(bad)?;
        let (w, h) = size.split_once('x').ok_or_else(bad)?;
        let (w, h, scale) = (
            w.parse::<u32>().map_err(|_| bad())?,
            h.parse::<u32>().map_err(|_| bad())?,
            scale.parse::<u32>().map_err(|_| bad())?,
        );
        if !(640..=16384).contains(&w) || !(480..=16384).contains(&h) || !(1..=4).contains(&scale) {
            return Err(text::display_arg_range(text));
        }
        Ok(Self { w, h, scale })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStamp {
    pub version: String,
    pub build: String,
    /// The day of the copy into the guest, `2026-11-02`, for `paddock app
    /// status`. Not part of the version compare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied: Option<String>,
}

impl AppStamp {
    /// The same app version and build: the copy in the guest is current.
    pub fn same_version(&self, other: &AppStamp) -> bool {
        self.version == other.version && self.build == other.build
    }
}

/// Every version that a start compares. An unknown
/// field is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub schema: u32,
    /// The golden version and build, `15.2 (24C101)`.
    pub golden: String,
    pub host_build: String,
    /// The framework version at the last write.
    pub vz: String,
    pub protocol: Version,
    pub paddock: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppStamp>,
    pub display: Display,
    /// The guest prepared the data disk (`data.prepare` answered). The
    /// host skips the request after that. The binding of the disks is
    /// read from the disk itself (`data_formatted`), never from here.
    #[serde(default)]
    pub data_prepared: bool,
    /// Why the last start did not prepare the data disk, for the status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_error: Option<String>,
    /// `<random>-<counter>`. The counter moves at every start, so a saved
    /// state resumes only against the data disk it saw.
    pub data_id: String,
    /// The `data_id` at the last save, when `state.vzs` is from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_id: Option<String>,
    /// The length of `state.vzs` at the last save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_len: Option<u64>,
    /// True after a quit that ended the session in order.
    pub clean: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Why a stamp cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampError {
    Missing,
    Unreadable(String),
    UnknownSchema(u32),
}

impl Stamp {
    pub fn read(path: &Path) -> Result<Self, StampError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(StampError::Missing),
            Err(e) => return Err(StampError::Unreadable(e.to_string())),
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| StampError::Unreadable(e.to_string()))?;
        let schema = value
            .get("schema")
            .and_then(|s| s.as_u64())
            .ok_or_else(|| StampError::Unreadable("no schema field".into()))?;
        if schema != SCHEMA as u64 {
            return Err(StampError::UnknownSchema(schema as u32));
        }
        serde_json::from_value(value).map_err(|e| StampError::Unreadable(e.to_string()))
    }

    /// Write the stamp atomically with mode 0600.
    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self)?;
        crate::privfs::write_atomic(path, json).with_context(|| text::cannot_write(path))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    /// The counter part of `data_id`, moved by one.
    pub fn next_data_id(&self) -> String {
        match self.data_id.rsplit_once('-') {
            Some((head, n)) => match n.parse::<u64>() {
                Ok(n) => format!("{head}-{}", n + 1),
                Err(_) => format!("{}-1", self.data_id),
            },
            None => format!("{}-1", self.data_id),
        }
    }
}

/// What the host is now: the values that a stamp is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Now {
    pub golden: String,
    pub host_build: String,
    pub vz: String,
    pub protocol: Version,
    pub paddock: String,
    pub agent: String,
    pub display: Display,
}

impl Now {
    /// A stamp of this host for a profile, at rest.
    pub fn stamp(&self, data_id: String) -> Stamp {
        Stamp {
            schema: SCHEMA,
            golden: self.golden.clone(),
            host_build: self.host_build.clone(),
            vz: self.vz.clone(),
            protocol: self.protocol,
            paddock: self.paddock.clone(),
            agent: self.agent.clone(),
            app: None,
            display: self.display,
            data_prepared: false,
            data_error: None,
            data_id,
            state_id: None,
            state_len: None,
            clean: true,
            saved: None,
            extra: Default::default(),
        }
    }
}

/// Why a profile with a stamp cannot start warm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    GoldenChanged,
    HostChanged,
    ProtocolChanged,
    DisplayChanged,
    StateDamaged,
    Unreadable,
}

/// How the next start runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    Fresh,
    Repair,
    Stale(Reason),
    Warm,
    Cold,
}

impl Start {
    /// The start makes a clone of the golden disk.
    pub fn clones(&self) -> bool {
        matches!(
            self,
            Start::Fresh | Start::Repair | Start::Stale(Reason::GoldenChanged)
        )
    }

    /// The word of the event log and of `paddock app status`.
    pub fn word(&self) -> &'static str {
        match self {
            Start::Fresh => "fresh",
            Start::Repair => "repair",
            Start::Stale(_) => "cold",
            Start::Warm => "warm",
            Start::Cold => "cold",
        }
    }
}

/// The space line of a save that cannot happen.
pub fn save_refusal(needs: u64, free: u64) -> String {
    text::save_skipped(needs, free)
}

/// Whether any `.partial` file or folder exists in the profile, at the
/// top level or one level down.
pub fn partials(profile: &Profile) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(&profile.dir) else {
        return found;
    };
    for e in entries.flatten() {
        let p = e.path();
        if is_partial(&p) {
            found.push(p);
            continue;
        }
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Ok(inner) = std::fs::read_dir(&p) {
                found.extend(inner.flatten().map(|e| e.path()).filter(|p| is_partial(p)));
            }
        }
    }
    found.sort();
    found
}

fn is_partial(p: &Path) -> bool {
    layout::is_partial(p)
}

/// The start table. `free` is the free space of the volume
/// of the profile; a start that clones needs [`CLONE_NEEDS`].
pub fn decide(profile: &Profile, now: &Now, free: u64) -> Result<Start, Refusal> {
    let start = decide_files(profile, now)?;
    if start.clones() && free < CLONE_NEEDS {
        return Err(Refusal::Space {
            needs: CLONE_NEEDS,
            free,
        });
    }
    Ok(start)
}

/// The decision from the files alone, before the space check. The data
/// disk, once formatted, binds the system set: a formatted `data.img`
/// without `system.img` is `DataLocked`.
pub fn decide_files(profile: &Profile, now: &Now) -> Result<Start, Refusal> {
    let exists = |p: PathBuf| std::fs::symlink_metadata(p).is_ok();
    if !exists(profile.dir.clone()) {
        return Ok(Start::Fresh);
    }
    let has_data = exists(profile.data());
    let has_system = exists(profile.system());
    let bound = has_data && data_formatted(&profile.data());
    if bound && !has_system {
        return Err(Refusal::DataLocked {
            profile: profile.dir.clone(),
            backup: super::backup::newest(profile).map(|b| b.dir),
        });
    }
    if !partials(profile).is_empty() {
        return Ok(Start::Repair);
    }
    let stamp = match Stamp::read(&profile.stamp()) {
        Ok(s) => s,
        Err(StampError::Missing) if !has_data && !has_system => return Ok(Start::Fresh),
        Err(StampError::Missing | StampError::Unreadable(_)) => return Ok(Start::Repair),
        Err(StampError::UnknownSchema(_)) => return Ok(Start::Stale(Reason::Unreadable)),
    };
    if !has_data || !has_system || !exists(profile.aux()) || !exists(profile.machine_id()) {
        return Ok(Start::Repair);
    }
    if let Some(reason) = stale_reason(&stamp, now, bound) {
        return Ok(Start::Stale(reason));
    }
    let Ok(state) = std::fs::symlink_metadata(profile.state()) else {
        return Ok(Start::Cold);
    };
    let matches = stamp.state_id.as_deref() == Some(stamp.data_id.as_str())
        && stamp.state_len.is_some_and(|len| state.len() >= len)
        && stamp.clean;
    Ok(if matches {
        Start::Warm
    } else {
        Start::Stale(Reason::StateDamaged)
    })
}

/// The line that the user sees for a decision.
pub fn line(start: Start, old: Option<&Stamp>, now: &Now) -> String {
    match start {
        Start::Fresh => text::start_fresh(),
        Start::Repair => text::profile_repaired(),
        Start::Stale(Reason::GoldenChanged) => {
            text::golden_changed(old.map(|s| s.golden.as_str()).unwrap_or("?"), &now.golden)
        }
        Start::Stale(Reason::HostChanged) => text::state_stale(
            old.map(|s| s.host_build.as_str()).unwrap_or("?"),
            &now.host_build,
        ),
        Start::Stale(Reason::ProtocolChanged) => text::protocol_changed(
            old.map(|s| s.protocol.major).unwrap_or(0),
            now.protocol.major,
        ),
        Start::Stale(Reason::DisplayChanged) => text::display_differs(
            &old.map(|s| s.display.to_string())
                .unwrap_or_else(|| "?".into()),
            &now.display.to_string(),
        ),
        Start::Stale(Reason::StateDamaged) => text::state_damaged(),
        Start::Stale(Reason::Unreadable) => text::stamp_foreign(),
        Start::Warm => text::warm_start(),
        Start::Cold if old.is_some_and(|s| !s.clean) => text::ended_without_save(),
        Start::Cold => text::cold_boot(),
    }
}

/// What `remove` removes. The only remover of profile files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    System,
    Aux,
    State,
    Stamp,
    Apps,
    Drop,
    Logs,
    Partials,
    /// The machine identifier: part of the system set, goes with `Data`.
    MachineId,
    Data,
    All,
}

impl What {
    pub const ALL: [What; 11] = [
        What::System,
        What::Aux,
        What::State,
        What::Stamp,
        What::Apps,
        What::Drop,
        What::Logs,
        What::Partials,
        What::MachineId,
        What::Data,
        What::All,
    ];

    /// The variants that name a file of the system set.
    pub fn system_set(self) -> bool {
        matches!(self, What::System | What::Aux | What::MachineId)
    }
}

/// The paths that `remove(profile, what)` touches.
pub fn targets(profile: &Profile, what: What) -> Vec<PathBuf> {
    match what {
        What::System => vec![profile.system()],
        What::Aux => vec![profile.aux()],
        What::State => vec![profile.state()],
        What::Stamp => vec![profile.stamp()],
        What::Apps => vec![profile.apps()],
        What::Drop => vec![profile.drop_dir()],
        What::Logs => vec![profile.logs()],
        What::Partials => partials(profile),
        What::MachineId => vec![profile.machine_id()],
        What::Data => vec![profile.data()],
        What::All => vec![profile.dir.clone()],
    }
}

/// Remove files of a profile. Returns the paths that existed and went.
pub fn remove(profile: &Profile, what: What) -> io::Result<Vec<PathBuf>> {
    let mut gone = Vec::new();
    for p in targets(profile, what) {
        let Ok(m) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if m.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
        gone.push(p);
    }
    Ok(gone)
}

/// What a start discards before it runs. `Repair` keeps the system set: a
/// formatted data disk mounts only under the system disk that made it, and
/// `prepare` clones only what is missing. `Stale(GoldenChanged)` is decided
/// only for a blank data disk.
pub fn removals(start: Start) -> Vec<What> {
    match start {
        Start::Fresh | Start::Warm | Start::Cold => vec![],
        Start::Repair => vec![What::Partials, What::Stamp, What::State],
        Start::Stale(Reason::GoldenChanged) => {
            vec![What::State, What::Stamp, What::System, What::Aux]
        }
        Start::Stale(_) => vec![What::State],
    }
}

/// The calls of `prepare` that differ per system or per test.
pub struct Hooks<'a> {
    /// Clone the golden disk `src` as the new file `dst`: `clonefile` on
    /// macOS, a copy on Linux.
    pub clone: &'a dyn Fn(&Path, &Path) -> io::Result<()>,
    /// A new machine identifier, as its data representation.
    pub machine_id: &'a dyn Fn() -> Result<Vec<u8>>,
    /// A backup of the data disk, with its reason (`pre-repair`,
    /// `pre-golden-<version>`). `run.rs` fills it with
    /// `run_vm::backup_data`.
    pub backup: &'a mut dyn FnMut(&Profile, &str) -> Result<()>,
    /// Format a new data disk (`data_disk.rs`): `format_on_mac` on a Mac.
    /// An error leaves the disk blank for the guest agent.
    pub format_data: &'a dyn Fn(&Path) -> Result<()>,
}

/// A plain copy of a file as `dst`, which must not exist.
pub fn copy_clone(src: &Path, dst: &Path) -> io::Result<()> {
    let mut from = std::fs::File::open(src)?;
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dst)?;
    io::copy(&mut from, &mut to)?;
    Ok(())
}

/// Make the files of the profile for `start`, as `.partial` names, then
/// commit each, the stamp last. `Warm` and `Cold` change nothing.
pub fn prepare(
    profile: &Profile,
    golden: &Golden,
    now: &Now,
    start: Start,
    hooks: &mut Hooks,
) -> Result<()> {
    let old = Stamp::read(&profile.stamp()).ok();
    let exists = |p: PathBuf| std::fs::symlink_metadata(p).is_ok();
    if start == Start::Repair && exists(profile.system()) && exists(profile.data()) {
        // The pair is complete: a backup of it before anything moves.
        (hooks.backup)(profile, "pre-repair")?;
    }
    for what in removals(start) {
        remove(profile, what)
            .with_context(|| text::cannot_remove_part(&format!("{what:?}"), &profile.dir))?;
    }
    if !start.clones() {
        return Ok(());
    }
    // The system disk that survived the removals stays: a formatted data
    // disk mounts only under it.
    let kept_system = exists(profile.system());
    std::fs::create_dir_all(&profile.dir).with_context(|| text::cannot_make(&profile.dir))?;
    std::fs::set_permissions(&profile.dir, std::fs::Permissions::from_mode(0o700))?;
    for d in profile.folders() {
        if std::fs::symlink_metadata(&d).is_err() {
            std::fs::create_dir(&d).with_context(|| text::cannot_make(&d))?;
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))?;
        }
    }

    if !kept_system {
        let system = partial(&profile.system());
        let _ = std::fs::remove_file(&system);
        (hooks.clone)(&golden.disk, &system)
            .with_context(|| text::cannot_clone(&golden.disk, &system))?;
        std::fs::set_permissions(&system, std::fs::Permissions::from_mode(0o600))?;
        commit(&system)?;
    }

    if !exists(profile.aux()) {
        let aux = partial(&profile.aux());
        let _ = std::fs::remove_file(&aux);
        copy_clone(&golden.aux, &aux).with_context(|| text::cannot_copy(&golden.aux, &aux))?;
        commit(&aux)?;
    }

    if std::fs::symlink_metadata(profile.machine_id()).is_err() {
        let id = (hooks.machine_id)()?;
        if id.is_empty() || id.len() > 4096 {
            bail!(text::machine_id_size(id.len()));
        }
        let p = partial(&profile.machine_id());
        let _ = std::fs::remove_file(&p);
        write_new(&p, &id)?;
        commit(&p)?;
    }

    if std::fs::symlink_metadata(profile.data()).is_err() {
        // A GPT and an APFS volume before the first boot, so the guest
        // never shows its dialog for an unreadable disk.
        super::data_disk::make(&profile.data(), DATA_DISK_BYTES, hooks.format_data)?;
    }

    let data_id = old
        .as_ref()
        .map(|s| s.data_id.clone())
        .unwrap_or_else(|| format!("{}-0", random_hex(8)));
    let mut stamp = now.stamp(data_id);
    if let (true, Some(old)) = (kept_system, old) {
        // The system disk stays, so the golden it came from, the app copy
        // on the data disk and the prepare flag stay with it.
        stamp.golden = old.golden;
        stamp.app = old.app;
        stamp.data_prepared = old.data_prepared;
        stamp.data_error = old.data_error;
    }
    stamp.write(&profile.stamp())?;
    Ok(())
}

/// Mark the session as running: the data counter moves, so an old saved
/// state never matches again, and `clean` is false until the quit path
/// ends. Returns the stamp that the session holds.
pub fn begin(profile: &Profile, now: &Now, start: Start) -> Result<Stamp> {
    let mut stamp = match Stamp::read(&profile.stamp()) {
        Ok(s) => s,
        Err(StampError::UnknownSchema(_)) => {
            // Another paddock wrote it. Keep its data id when it has one,
            // so the backups and the guest agree on the disk.
            let id =
                raw_data_id(&profile.stamp()).unwrap_or_else(|| format!("{}-0", random_hex(8)));
            now.stamp(id)
        }
        Err(e) => bail!(text::stamp_unreadable(&profile.stamp(), &format!("{e:?}"))),
    };
    // `golden` stays: it names the golden this system disk was cloned from.
    stamp.host_build = now.host_build.clone();
    stamp.vz = now.vz.clone();
    stamp.protocol = now.protocol;
    stamp.paddock = now.paddock.clone();
    stamp.agent = now.agent.clone();
    stamp.display = now.display;
    stamp.data_id = stamp.next_data_id();
    stamp.clean = false;
    if start != Start::Warm {
        stamp.state_id = None;
        stamp.state_len = None;
    }
    stamp.write(&profile.stamp())?;
    Ok(stamp)
}

/// Write the stamp at the end of a session. `saved` is the length of the
/// saved state when the quit path saved one.
pub fn finish(
    profile: &Profile,
    stamp: &mut Stamp,
    clean: bool,
    saved: Option<u64>,
    now_secs: u64,
) -> Result<()> {
    stamp.clean = clean;
    stamp.saved = Some(crate::waiting::format_rfc3339(now_secs));
    match saved {
        Some(len) => {
            stamp.state_id = Some(stamp.data_id.clone());
            stamp.state_len = Some(len);
        }
        None => {
            stamp.state_id = None;
            stamp.state_len = None;
        }
    }
    stamp.write(&profile.stamp())
}

/// The `data_id` of a stamp of any schema, when it has one.
fn raw_data_id(path: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let id = v.get("data_id")?.as_str()?;
    (!id.is_empty() && id.len() <= 64).then(|| id.to_string())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| text::cannot_make(path))?;
    f.write_all(bytes).and_then(|_| f.sync_all())?;
    Ok(())
}

/// `n` random bytes as hex.
pub fn random_hex(n: usize) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_err()
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        buf = t.to_le_bytes()[..n.min(16)].to_vec();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The free space of the volume of `path`, or of its nearest ancestor
/// that exists. [`FAKE_FREE_VAR`] replaces it for the checks. Zero when
/// nothing of the path exists.
pub fn free_bytes(path: &Path) -> u64 {
    let fake = std::env::var(FAKE_FREE_VAR).ok();
    golden::free_bytes(path, fake.as_deref()).unwrap_or(0)
}

/// `2G`, `500M`, `8GiB`, `1024` (bytes).
pub fn parse_size(text: &str) -> Option<u64> {
    golden::parse_size(text)
}

/// `1.2 GiB`, `512 MiB`, `8 GiB`.
pub fn size_text(bytes: u64) -> String {
    text::gib(bytes)
}

#[cfg(test)]
#[path = "session_reset_tests.rs"]
mod reset_tests;
#[cfg(test)]
#[path = "session_tests.rs"]
pub(crate) mod tests;
