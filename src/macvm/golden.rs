//! The golden macOS guest: its seven states, its manifest, the guest
//! password, the sizes and the texts of `paddock macvm setup`.
//!
//! Everything here runs on every system. The framework calls that move
//! the golden from one state to the next are in `vz_mac.rs`, behind the
//! `Backend` trait of `setup.rs`.

use super::layout::{self, Layout};
use super::session::Stamp;
use super::text;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The manifest schema that this paddock writes and reads.
pub const SCHEMA: u32 = 1;
/// The disk of the golden, sparse. macOS wants a large volume.
pub const DISK_BYTES: u64 = 64 << 30;
/// What an install really writes into the sparse disk, with room.
pub const INSTALL_REAL_BYTES: u64 = 30 << 30;
/// The size of a restore image when the download has not told it yet,
/// 15 GB as the README says.
pub const IPSW_GUESS_BYTES: u64 = 15_000_000_000;
/// The minutes that the install and the guided step take, for the
/// question.
pub const INSTALL_MINUTES: u32 = 30;
/// The password of the guest account: letters of [`PASSWORD_SET`], this
/// many. 20 characters from 20 letters is 86 bits.
pub const PASSWORD_LEN: usize = 20;
/// The letters of a made password: the lowercase letters that sit on the
/// same key, unshifted, on the US, UK, German QWERTZ, French AZERTY and
/// Chinese Pinyin/ABC layouts. `a q w z y m` move on AZERTY or QWERTZ,
/// digits need shift on AZERTY, and symbols move everywhere. So the
/// `Type the password` button of the guided panel types the same text
/// whatever layout the person picked in Setup Assistant, and a person
/// who types it by hand finds each letter where it is printed.
pub const PASSWORD_SET: &[u8] = b"bcdefghijklnoprstuvx";
/// The longest password that the keyring takes: the one that the person
/// set in Setup Assistant may be any length, but the session helper reads
/// the token and the password from one pipe of `2 * TOKEN_LINE_MAX` bytes.
pub const PASSWORD_MAX: usize = 128;
/// The account that the guided step asks the user to create.
pub const GUEST_USER: &str = "paddock";

/// The seven states of a setup, plus the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The restore image: fetched from Apple or given with `--ipsw`.
    Fetch,
    /// `VZMacOSInstaller` writes `disk.img`, `aux.img` and the ids.
    Install,
    /// The seed folder: the agent, its `launch` script, the launchd
    /// plist and `install.sh`, in the golden folder for the guided boot.
    /// An older build wrote them into the disk; its manifest runs this.
    Seed,
    /// The first boot in paddock's window: the account (the provisioning
    /// options or the person in Setup Assistant), the agent (the offline
    /// seed or the typed command), until the agent's first report.
    Guided,
    /// The guest stops, its TCC database gets the agent's Accessibility
    /// permission offline, and it boots again until the agent reports the
    /// permission. A golden that the person trusted in the window passes.
    Trust,
    /// The agent configures the guest with the commands of `configure.rs`.
    Configure,
    /// The guest shuts down, the images become read-only and immutable.
    Seal,
    /// Done. Sessions clone this golden.
    Sealed,
}

impl State {
    pub const ALL: [State; 8] = [
        State::Fetch,
        State::Install,
        State::Seed,
        State::Guided,
        State::Trust,
        State::Configure,
        State::Seal,
        State::Sealed,
    ];

    /// The name in the manifest and in the lines.
    pub fn name(self) -> &'static str {
        match self {
            State::Fetch => "fetch",
            State::Install => "install",
            State::Seed => "seed",
            State::Guided => "guided",
            State::Trust => "trust",
            State::Configure => "configure",
            State::Seal => "seal",
            State::Sealed => "sealed",
        }
    }

    /// Whether the state writes files under the golden folder, so that a
    /// `.partial` left behind means that it crashed.
    pub fn writes(self) -> bool {
        matches!(self, State::Install | State::Seed | State::Seal)
    }

    /// The number of the state, 1 to [`State::STEPS`], for the progress
    /// lines.
    /// The number of states before `Sealed`.
    pub const STEPS: usize = State::ALL.len() - 1;

    pub fn number(self) -> usize {
        State::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1
    }
}

/// How a state ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The state did its work.
    Done,
    /// The state failed with this reason. The setup stays in the state.
    Failed(String),
    /// The restore image failed its check (`Install` only): the fetch
    /// runs again, so a torn download is fetched again, never installed.
    ImageRejected(String),
    /// The user stopped the setup. The state stays for a later continue.
    UserStopped,
    /// A `.partial` of this state was found at the start: the earlier
    /// setup crashed in it. The partial goes and the state runs again.
    Crashed,
}

/// The state after `outcome` in `state`. Pure: the table test lists every
/// pair.
pub fn next(state: State, outcome: &Outcome) -> State {
    match (state, outcome) {
        (State::Sealed, _) => State::Sealed,
        (State::Install, Outcome::ImageRejected(_)) => State::Fetch,
        (s, Outcome::Failed(_) | Outcome::ImageRejected(_)) => s,
        (s, Outcome::UserStopped | Outcome::Crashed) => s,
        (State::Fetch, Outcome::Done) => State::Install,
        (State::Install, Outcome::Done) => State::Seed,
        (State::Seed, Outcome::Done) => State::Guided,
        (State::Guided, Outcome::Done) => State::Trust,
        (State::Trust, Outcome::Done) => State::Configure,
        (State::Configure, Outcome::Done) => State::Seal,
        (State::Seal, Outcome::Done) => State::Sealed,
    }
}

/// What a setup finds at its start: the state to run, and whether the
/// earlier setup crashed in it (a writing state with a `.partial`).
pub fn resume_point(manifest: Option<&Manifest>, partials: &[PathBuf]) -> (State, Option<Outcome>) {
    let Some(m) = manifest else {
        return (State::Fetch, None);
    };
    if m.state.writes() && !partials.is_empty() {
        return (m.state, Some(Outcome::Crashed));
    }
    (m.state, None)
}

/// `manifest.toml` of a golden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub state: State,
    /// The macOS version of the restore image, for example `15.2`.
    pub version: String,
    /// Its build, for example `24C101`.
    pub build: String,
    /// `VZMacHardwareModel.dataRepresentation`, base64.
    #[serde(default)]
    pub hardware_model_b64: String,
    /// The macOS version of the host at the build.
    pub host_version: String,
    pub host_build: String,
    /// The paddock version whose agent the golden carries.
    pub agent_version: String,
    /// The last write, RFC 3339 UTC.
    pub time: String,
    /// The restore image that `Install` uses, and its size when the
    /// download told it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipsw: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipsw_size: Option<u64>,
    /// The time of the seal, RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<String>,
}

impl Manifest {
    /// A new manifest at `Fetch`, before anything is known.
    pub fn new(host_version: &str, host_build: &str, agent_version: &str) -> Self {
        Self {
            schema: SCHEMA,
            state: State::Fetch,
            version: String::new(),
            build: String::new(),
            hardware_model_b64: String::new(),
            host_version: host_version.into(),
            host_build: host_build.into(),
            agent_version: agent_version.into(),
            time: now_text(),
            ipsw: None,
            ipsw_size: None,
            sealed: None,
        }
    }

    /// Parse a manifest. An unknown schema or an unknown state is an
    /// error that names the file and `paddock macvm remove`: this paddock
    /// cannot continue a setup that another version left.
    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        let raw: toml::Value =
            toml::from_str(text).with_context(|| text::manifest_not_toml(path))?;
        let schema = raw.get("schema").and_then(|v| v.as_integer());
        if schema != Some(SCHEMA as i64) {
            bail!(text::manifest_schema(path, schema, SCHEMA));
        }
        let state = raw.get("state").and_then(|v| v.as_str()).unwrap_or("");
        if !State::ALL.iter().any(|s| s.name() == state) {
            bail!(text::manifest_unknown_state(path, state));
        }
        raw.try_into().with_context(|| text::manifest_invalid(path))
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).with_context(text::manifest_unserializable)
    }

    /// Move to `state` and stamp the time.
    pub fn at(&mut self, state: State) -> &mut Self {
        self.state = state;
        self.time = now_text();
        self
    }

    /// `15.2 (24C101)`, or `unknown` before the fetch.
    pub fn version_text(&self) -> String {
        if self.version.is_empty() {
            "unknown".into()
        } else if self.build.is_empty() {
            self.version.clone()
        } else {
            format!("{} ({})", self.version, self.build) // not user text
        }
    }
}

/// The time now as RFC 3339 UTC.
pub fn now_text() -> String {
    crate::waiting::format_rfc3339(crate::state::now())
}

/// The bytes that a state needs free on the volume before it starts.
/// `ipsw_size` is the size of the restore image when the download told
/// it, else `IPSW_GUESS_BYTES`.
pub fn space_needed(state: State, ipsw_size: Option<u64>) -> u64 {
    match state {
        // The download and the install after it, on the same volume.
        State::Fetch => ipsw_size.unwrap_or(IPSW_GUESS_BYTES) + INSTALL_REAL_BYTES,
        State::Install => INSTALL_REAL_BYTES,
        _ => 0,
    }
}

/// The size of a file as `du -sh` counts it: the blocks it uses, not its
/// length. A sparse 64 GiB disk with 20 GiB written says 20 GiB.
pub fn used_bytes(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|m| m.blocks() * 512)
        .unwrap_or(0)
}

/// The used bytes of every file in a folder tree.
pub fn used_bytes_in(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => total += used_bytes(&p),
                _ => {}
            }
        }
    }
    total
}

/// The free bytes of the volume that holds `path`, or of its nearest
/// existing parent. `PADDOCK_MACVM_FAKE_FREE` (`1G`, `500M`, bytes)
/// replaces it, for the checks.
pub fn free_bytes(path: &Path, fake: Option<&str>) -> Result<u64> {
    if let Some(text) = fake {
        return parse_size(text).ok_or_else(|| anyhow!(text::fake_free_invalid(text)));
    }
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!(text::no_folder_exists(path)))?;
    }
    statvfs_free(&p)
}

fn statvfs_free(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a zeroed struct of the right type.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| text::free_space_unreadable(path));
    }
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

/// `1G`, `500M`, `2T`, `1024K`, `1.5 GiB`, `8GB` or plain bytes. Binary
/// units, whatever the suffix says.
pub fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim();
    let digits: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let unit = t[digits.len()..].trim().to_ascii_lowercase();
    let n: f64 = digits.parse().ok()?;
    let mult: u64 = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return None,
    };
    Some((n * mult as f64) as u64)
}

/// The password of the guest account: [`PASSWORD_LEN`] letters of
/// [`PASSWORD_SET`] out of `/dev/urandom`. Rejection sampling keeps the
/// choice even.
pub fn make_password() -> Result<String> {
    const SET: &[u8] = PASSWORD_SET;
    // The largest multiple of the set size that fits a byte: bytes at or
    // above it would favor the first letters of the set.
    const LIMIT: usize = 256 / SET.len() * SET.len();
    let mut f =
        std::fs::File::open("/dev/urandom").with_context(|| text::urandom_failed("open"))?;
    let mut out = String::with_capacity(PASSWORD_LEN);
    let mut buf = [0u8; 64];
    while out.len() < PASSWORD_LEN {
        f.read_exact(&mut buf)
            .with_context(|| text::urandom_failed("read"))?;
        for b in buf {
            if (b as usize) < LIMIT && out.len() < PASSWORD_LEN {
                out.push(SET[b as usize % SET.len()] as char);
            }
        }
    }
    Ok(out)
}

/// Whether `s` is a password that `make_password` made.
pub fn is_password(s: &str) -> bool {
    s.len() == PASSWORD_LEN && s.bytes().all(|b| PASSWORD_SET.contains(&b))
}

/// Whether the keyring takes `s` as the guest password: a made one, one
/// of an older paddock (16 letters and digits), or the one that the
/// person set in Setup Assistant and typed into the panel. It must not be
/// empty, must fit [`PASSWORD_MAX`] bytes and must have no control
/// character: the password goes to `sudo -S` as one line, and to the
/// session helper as one line of its pipe.
pub fn is_keyring_password(s: &str) -> bool {
    !s.is_empty() && s.len() <= PASSWORD_MAX && !s.chars().any(char::is_control)
}

/// The paddock keyring of this user, a folder of 0600 files under the
/// paddock state folder, the one of `keyring.rs` and of the keychain
/// passwords. The guest password lives at `macvm/guest-password`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyring {
    pub dir: PathBuf,
    pub guest_password: PathBuf,
}

impl Keyring {
    /// Open the keyring: make its folder with 0700. A folder that cannot
    /// be made or written is a keyring that is not available, and the
    /// error names it, so that `setup` stops before `Fetch`.
    pub fn open(env: &crate::env::HostEnv) -> Result<Self> {
        let dir = env.paddock_state().join("keyring").join("macvm");
        crate::privfs::dir(&dir)
            .map_err(|e| anyhow!(text::keyring_unavailable(&dir, &e.to_string())))?;
        let probe = dir.join(".probe");
        crate::privfs::write(&probe, "")
            .and_then(|()| std::fs::remove_file(&probe))
            .map_err(|e| anyhow!(text::keyring_unavailable(&dir, &e.to_string())))?;
        Ok(Self {
            guest_password: dir.join("guest-password"),
            dir,
        })
    }

    /// The guest password: read, or made the first time. Never logged.
    pub fn password(&self) -> Result<String> {
        for attempt in 0..3 {
            if let Ok(s) = std::fs::read_to_string(&self.guest_password) {
                // Only a line end goes: a password the person set may
                // start or end with a space.
                let s = s.strip_suffix('\n').unwrap_or(&s);
                let s = s.strip_suffix('\r').unwrap_or(s).to_string();
                if is_keyring_password(&s) {
                    return Ok(s);
                }
                // A damaged file: a new password replaces it.
                let _ = std::fs::remove_file(&self.guest_password);
            }
            let pw = make_password()?;
            match crate::privfs::write_new(&self.guest_password, &pw) {
                Ok(()) => return Ok(pw),
                // Another setup was first: the next round reads its password.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 2 => {}
                Err(e) => {
                    return Err(anyhow!(text::keyring_unavailable(
                        &self.dir,
                        &e.to_string()
                    )))
                }
            }
        }
        bail!(text::keyring_unavailable(
            &self.dir,
            "the password file keeps changing"
        ))
    }

    /// Put the password that the person set in Setup Assistant in place of
    /// the one the keyring had: the configure step found that the guest
    /// refuses the old one. The write is atomic, so a crash leaves the old
    /// or the new password, never half of one.
    pub fn replace(&self, password: &str) -> Result<()> {
        if !is_keyring_password(password) {
            bail!(text::keyring_password_invalid(PASSWORD_MAX));
        }
        crate::privfs::write_atomic(&self.guest_password, password)
            .map_err(|e| anyhow!(text::keyring_unavailable(&self.dir, &e.to_string())))
    }

    /// Forget the guest password. `paddock macvm remove` does it, so the
    /// next golden gets a new one.
    pub fn forget(&self) -> std::io::Result<()> {
        super::layout::remove_any(&self.guest_password)
    }
}

/// `15.2` is older than `15.3` and than `16.0`. Unknown texts compare
/// equal, so nothing warns on them.
pub fn version_older(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(a), parse(b));
    if a.is_empty() || b.is_empty() || a == [0] || b == [0] {
        return false;
    }
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x < y;
        }
    }
    false
}

// ---- The golden swap of `macvm update` and `rollback` ------------------

/// The marker of a folder swap in progress, in the `macvm` folder. It
/// holds one word, `update` or `rollback`, from before the first rename
/// to after the second. A start that finds it finishes the swap.
pub const SWAP_MARKER: &str = "golden.swap";

/// The two swaps of the golden folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swap {
    /// `golden` to `golden.prev`, then `golden.new` to `golden`.
    Update,
    /// `golden` to `golden.new`, then `golden.prev` to `golden`.
    Rollback,
}

impl Swap {
    pub fn word(self) -> &'static str {
        match self {
            Swap::Update => "update",
            Swap::Rollback => "rollback",
        }
    }

    fn parse(text: &str) -> Option<Swap> {
        match text.trim() {
            "update" => Some(Swap::Update),
            "rollback" => Some(Swap::Rollback),
            _ => None,
        }
    }
}

fn marker_path(layout: &Layout) -> PathBuf {
    layout.root.join(SWAP_MARKER)
}

fn write_marker(layout: &Layout, swap: Swap) -> Result<()> {
    let p = marker_path(layout);
    crate::privfs::write_atomic(&p, swap.word()).with_context(|| text::cannot_write(&p))
}

fn read_marker(layout: &Layout) -> Option<Swap> {
    std::fs::read_to_string(marker_path(layout))
        .ok()
        .and_then(|t| Swap::parse(&t))
}

fn remove_marker(layout: &Layout) -> Result<()> {
    layout::remove_any(&marker_path(layout))
        .with_context(|| text::cannot_remove(&marker_path(layout)))
}

fn rename_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).with_context(|| text::cannot_move(from, to))
}

/// `paddock macvm update` finished its build in `golden.new`: the golden
/// becomes `golden.prev` and the new build becomes the golden. An older
/// `golden.prev` goes first. The build must be sealed. A swap that a
/// crash interrupted is finished first ([`recover`]).
pub fn swap(layout: &Layout) -> Result<()> {
    swap_with(layout, || {})
}

/// [`swap`] with a hook between the two renames. The crash test panics
/// in it.
pub fn swap_with(layout: &Layout, between: impl FnOnce()) -> Result<()> {
    recover(layout)?;
    let new = &layout.golden_new;
    if !new.dir.is_dir() {
        bail!(text::no_new_golden(&new.dir));
    }
    match new.read_manifest()? {
        Some(m) if m.state == State::Sealed => {}
        _ => bail!(text::new_golden_unsealed(&new.dir)),
    }
    if layout.golden_prev.dir.exists() {
        layout::remove_golden_dir(&layout.golden_prev)?;
    }
    write_marker(layout, Swap::Update)?;
    if layout.golden.dir.exists() {
        rename_dir(&layout.golden.dir, &layout.golden_prev.dir)?;
    }
    between();
    rename_dir(&new.dir, &layout.golden.dir)?;
    remove_marker(layout)
}

/// `paddock macvm rollback`: the previous golden takes the place of the
/// golden, which becomes `golden.new`, so that an `update` can rebuild
/// and a `remove` can drop it. A `golden.new` from before goes first.
pub fn rollback(layout: &Layout) -> Result<()> {
    rollback_with(layout, || {})
}

/// [`rollback`] with a hook between the two renames, for the crash test.
pub fn rollback_with(layout: &Layout, between: impl FnOnce()) -> Result<()> {
    recover(layout)?;
    if !layout.golden_prev.dir.is_dir() {
        bail!(text::no_prev_golden(&layout.golden_prev.dir));
    }
    if layout.golden_new.dir.exists() {
        layout::remove_golden_dir(&layout.golden_new)?;
    }
    write_marker(layout, Swap::Rollback)?;
    if layout.golden.dir.exists() {
        rename_dir(&layout.golden.dir, &layout.golden_new.dir)
            .with_context(|| text::cannot_move_aside(&layout.golden.dir))?;
    }
    between();
    rename_dir(&layout.golden_prev.dir, &layout.golden.dir)?;
    remove_marker(layout)
}

/// What [`recover`] finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovered {
    /// The second rename of an `update`: the new golden is in place.
    Update,
    /// The second rename of a `rollback`: the previous golden is in place.
    Rollback,
}

impl Recovered {
    /// The line for the user.
    pub fn line(self) -> String {
        match self {
            Recovered::Update => text::recovered_update(),
            Recovered::Rollback => text::recovered_rollback(),
        }
    }
}

/// Finish a swap that a crash interrupted between its two renames. Every
/// entry point of the golden calls it first (`paddock app run`, `macvm
/// status`, `update`, `rollback`, the doctor). Nothing to do when the
/// golden folder exists. Without it, the marker says which second rename
/// to repeat. Without a marker, `golden.prev` with `golden.new` is an
/// interrupted update (the newer build wins), `golden.prev` alone an
/// interrupted rollback.
pub fn recover(layout: &Layout) -> Result<Option<Recovered>> {
    let marker = read_marker(layout);
    let has = |g: &layout::Golden| g.dir.is_dir();
    if has(&layout.golden) {
        if marker.is_some() {
            remove_marker(layout)?;
        }
        return Ok(None);
    }
    let (prev, new) = (has(&layout.golden_prev), has(&layout.golden_new));
    let which = match marker {
        Some(s) => s,
        None if prev && new => Swap::Update,
        None if prev => Swap::Rollback,
        None => return Ok(None),
    };
    let done = match which {
        Swap::Update if new => {
            rename_dir(&layout.golden_new.dir, &layout.golden.dir)?;
            Some(Recovered::Update)
        }
        Swap::Rollback if prev => {
            rename_dir(&layout.golden_prev.dir, &layout.golden.dir)?;
            Some(Recovered::Rollback)
        }
        // The folder the marker names is gone, so the other one, when
        // there is one, becomes the golden: a golden beats none.
        _ if prev => {
            rename_dir(&layout.golden_prev.dir, &layout.golden.dir)?;
            Some(Recovered::Rollback)
        }
        _ if new => {
            rename_dir(&layout.golden_new.dir, &layout.golden.dir)?;
            Some(Recovered::Update)
        }
        _ => None,
    };
    remove_marker(layout)?;
    Ok(done)
}

/// The state of `golden.prev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prev {
    /// No previous golden.
    None,
    /// Kept for `paddock macvm rollback`: no profile has completed a
    /// session on the new golden yet.
    Kept { version: String },
    /// A profile completed a session on the new golden (a stamp with the
    /// new version and `clean = true`), so `paddock gc` removes it.
    Removable { version: String },
}

impl Prev {
    pub fn version(&self) -> Option<&str> {
        match self {
            Prev::None => None,
            Prev::Kept { version } | Prev::Removable { version } => Some(version),
        }
    }
}

/// The version text of a golden folder, `unknown` without a manifest.
pub fn version_text_of(g: &layout::Golden) -> String {
    g.read_manifest()
        .ok()
        .flatten()
        .map(|m| m.version_text())
        .unwrap_or_else(|| "unknown".into())
}

/// Whether `golden.prev` exists and whether it can go: the rule is one
/// stamp with the version of the current golden and `clean = true`.
pub fn prev_state(layout: &Layout) -> Prev {
    if !layout.golden_prev.dir.is_dir() {
        return Prev::None;
    }
    let version = version_text_of(&layout.golden_prev);
    let current = match layout.golden.read_manifest().ok().flatten() {
        Some(m) if m.state == State::Sealed => m.version_text(),
        _ => return Prev::Kept { version },
    };
    let clean_on_new = layout.profile_dirs().iter().any(|(_, _, dir)| {
        Stamp::read(&dir.join("stamp.json")).is_ok_and(|s| s.golden == current && s.clean)
    });
    if clean_on_new {
        Prev::Removable { version }
    } else {
        Prev::Kept { version }
    }
}

/// Remove `golden.prev`. Sessions never use it, so none has to stop.
pub fn remove_prev(layout: &Layout) -> Result<()> {
    layout::remove_golden_dir(&layout.golden_prev)
}

#[cfg(test)]
#[path = "golden_tests.rs"]
mod tests;
