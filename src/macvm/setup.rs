//! The driver of `paddock macvm setup`: the seven states in order, each
//! resumable, with the manifest as the record of where the setup is.
//!
//! The driver is plain code that runs on every system. The framework
//! calls of each state are behind [`Backend`]: `vz_mac.rs` implements it
//! on macOS, and the tests implement it with files. The rules it keeps:
//!
//! - The manifest names the state before the state runs. A later setup
//!   continues there.
//! - A writing state (`Install`, `Seed`, `Seal`) that left a `.partial`
//!   crashed: the partials go and the state runs again.
//! - No state needs root on the host. `Seed` writes the seed folder and,
//!   through a mount without owners, the first-boot daemon and the agent
//!   into the guest disk (`offline.rs`); on any error the disk is the
//!   untouched clone again and the person types the seed command in the
//!   guided window. `Trust` stops the guest and sets the agent's
//!   Accessibility permission in its TCC database (`tcc.rs`); on any
//!   error the person turns the switch on in the window.
//! - Every state start and end goes to `events.jsonl` of the golden.
//! - The manifest says `sealed` only after the images are sealed.
//! - The free space is checked before the download and before the install.
//! - The password of the guest never goes to the terminal or to a log.

use super::diskimage::DiskTool;
use super::golden::{self, Keyring, Manifest, Outcome, State};
use super::layout::{self, Golden, Layout};
use super::provision::{self, Route};
use super::text;
use super::{seed, setup_events, setup_offline, tcc};
use crate::env::HostEnv;
use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The exit code when the user stopped or a state failed: a later setup
/// continues.
pub const EXIT_AGAIN: i32 = 1;

/// The options of a setup.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// `--ipsw PATH`: a restore image file instead of the download.
    pub ipsw: Option<PathBuf>,
    /// `--yes`: no question before the download.
    pub yes: bool,
    /// The `paddock-guest` binary that the seed folder holds.
    pub agent: Option<PathBuf>,
    /// `PADDOCK_MACVM_FAKE_FREE`: the free space, for the checks.
    pub fake_free: Option<String>,
    /// `PADDOCK_MACVM_SKIP_CONFIGURE=1`: pass `Configure` without the
    /// guest agent. For the checks of the seal without the agent.
    pub skip_configure: bool,
    /// `--force`: `paddock macvm update` rebuilds a golden of the version
    /// the current one has.
    pub force: bool,
    /// The macOS version of the current golden when this build is an
    /// `update` into `golden.new`: `Fetch` refuses an image of the same
    /// version without `--force`, before any download.
    pub current_version: Option<String>,
    /// `PADDOCK_MACVM_GUIDED=1`: every step of the guest to the person in
    /// the guided window, no offline patch of the disk.
    pub guided: bool,
}

/// The host at the build.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostInfo {
    pub version: String,
    pub build: String,
}

/// The newest restore image that this host runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreInfo {
    pub url: String,
    pub version: String,
    pub build: String,
    /// The size that the server told, when it did.
    pub size: Option<u64>,
}

/// What `VZMacOSRestoreImage.load` says about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub version: String,
    pub build: String,
    /// `VZMacHardwareModel.dataRepresentation`, base64.
    pub hardware_model_b64: String,
}

/// How a state with a window ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Done,
    /// The user pressed `Stop` or closed the window.
    Stopped,
    /// `sudo -S` in the guest refused the password: the person set
    /// another one in Setup Assistant, or mistyped it twice the same way.
    /// The driver asks for it ([`Backend::ask_password`]) and retries.
    WrongPassword,
}

/// How many times the configure step runs with a password that the guest
/// refuses before the state fails. The person can `Stop` at each question.
pub const PASSWORD_TRIES: usize = 5;

/// The framework side of each state.
pub trait Backend {
    fn host(&self) -> HostInfo;
    /// `VZMacOSRestoreImage.fetchLatestSupported`, plus the size from the
    /// server.
    fn latest(&mut self) -> Result<RestoreInfo>;
    /// Download `url` to `dest`, a `.partial` name. The backend prints the
    /// progress lines and may continue a partial file.
    fn download(&mut self, url: &str, dest: &Path, size: Option<u64>) -> Result<()>;
    /// `VZMacOSRestoreImage.load` of a file: an error when this Mac cannot
    /// run it.
    fn check_image(&mut self, path: &Path) -> Result<ImageInfo>;
    /// Write `disk.img`, `aux.img`, `hardware-model` and `machine-id`
    /// under their `.partial` names and run the installer. The driver
    /// commits them.
    fn install(&mut self, golden: &Golden, ipsw: &Path) -> Result<()>;
    /// The framework check of the provisioning options: `Err` with the
    /// name of the class or method that this Mac lacks.
    fn provisioning(&self) -> std::result::Result<(), String>;
    /// The guided state: boot the golden in the window (with the
    /// provisioning options when `route` says so) and wait for the agent.
    /// With `route.trust_offline` it ends at the agent's first health
    /// report and stops the guest; else when the agent reports the
    /// Accessibility permission. `Stopped` when the user stops. The boot
    /// shares `golden.seed` read-only as `seed::SHARE`.
    fn guided(&mut self, golden: &Golden, password: &str, route: &Route) -> Result<End>;
    /// The tool that mounts the guest disk for the offline seed and trust;
    /// `None` when this backend has none.
    fn disk_tool(&mut self) -> Option<Box<dyn DiskTool>> {
        None
    }
    /// The compiled code requirement of the agent binary `agent`, for a
    /// TCC row that has none.
    fn agent_requirement(&mut self, _agent: Option<&Path>) -> Result<Vec<u8>> {
        Ok(tcc::identifier_requirement(tcc::IDENTIFIER))
    }
    /// Boot the golden (when it is not up), wait for the agent's hello and
    /// its first health report: `Some(accessibility)`, or `None` when the
    /// user stopped.
    fn trusted(&mut self, golden: &Golden, password: &str) -> Result<Option<bool>>;
    /// The configure request to the agent. `End::WrongPassword` when
    /// `sudo -S` refused the password.
    fn configure(&mut self, golden: &Golden, password: &str) -> Result<End>;
    /// The guest refused the password: the panel asks the person for the
    /// one they set, in a secure field. `None` when they stop instead.
    fn ask_password(&mut self, golden: &Golden) -> Result<Option<String>>;
    /// Shut the guest down and wait for the stop, when it runs. The driver
    /// sets the flags. The password is for `sudo -S` of the shutdown in
    /// the guest.
    fn shutdown(&mut self, golden: &Golden, password: &str) -> Result<()>;
    /// The setup ends, whatever the state: close the window and stop a
    /// guest that still runs.
    fn finish(&mut self) {}
}

/// The terminal side: the lines and the one question.
pub struct Ui<'a> {
    pub out: &'a mut dyn Write,
    pub ask: &'a mut dyn FnMut(&str) -> Result<bool>,
}

impl Ui<'_> {
    pub(super) fn line(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }
}

/// The checks before anything is made: the sandbox, the keyring, the
/// `--ipsw` path. `paddock macvm setup` runs them before its question,
/// the helper runs them again before the states.
pub fn preflight(env: &HostEnv, opts: &Opts) -> Result<Keyring> {
    crate::update::refuse_in_sandbox(env, "paddock macvm setup")?;
    if let Some(p) = &opts.ipsw {
        if !p.is_file() {
            bail!(text::ipsw_missing(p));
        }
    }
    Keyring::open(env)
}

/// The agent binary that the seed folder holds: the given one, the one
/// of this paddock version in the layout, or `paddock-guest` next to the
/// running binary. `None` when this build has no agent yet.
pub fn agent_binary(layout: &Layout, opts: &Opts) -> Option<PathBuf> {
    if let Some(p) = &opts.agent {
        return Some(p.clone());
    }
    let own = layout.agent_file(super::version::paddock_version());
    if own.is_file() {
        return Some(own);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("paddock-guest")))
        .filter(|p| p.is_file())
}

/// Run the setup of `golden` to the seal, or to the point where the user
/// stopped or a state failed. The exit code is 0 at the seal and
/// `EXIT_AGAIN` otherwise.
pub fn run(
    env: &HostEnv,
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    ui: &mut Ui,
) -> Result<i32> {
    let r = run_states(env, layout, golden, backend, opts, ui);
    backend.finish();
    r
}

fn run_states(
    env: &HostEnv,
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    ui: &mut Ui,
) -> Result<i32> {
    let keyring = preflight(env, opts)?;
    let mut password = keyring.password()?;
    let _lock = lock(layout)?;
    let mut m = match golden.read_manifest()? {
        Some(m) => m,
        None => {
            let h = backend.host();
            Manifest::new(&h.version, &h.build, super::version::paddock_version())
        }
    };
    if m.state == State::Sealed {
        ui.line(&text::golden_already(&m.version_text()));
        return Ok(0);
    }
    let (state, crashed) = golden::resume_point(Some(&m), &golden.partials());
    if state != State::Fetch || crashed.is_some() {
        ui.line(&text::setup_continues(state));
    }
    if crashed.is_some() {
        let gone = golden.remove_partials()?;
        ui.line(&text::setup_crashed(state, &gone));
    }
    let mut fetch_retries = 0;
    loop {
        let state = m.state;
        golden.write_manifest(&m)?;
        ui.line(&text::state_starts(state));
        setup_events::record(&golden.events, setup_events::STATE, false, state.name());
        let outcome = match state {
            State::Fetch => fetch(layout, backend, opts, &mut m, ui)?,
            State::Install => install(layout, golden, backend, opts, &mut m, ui)?,
            State::Seed => setup_offline::seed_state(layout, golden, backend, opts, ui),
            State::Guided => guided(layout, golden, backend, opts, &password, &m, ui),
            State::Trust => setup_offline::trust(layout, golden, backend, opts, &password, ui),
            State::Configure if opts.skip_configure => {
                ui.line(&text::agent_missing("configure skipped"));
                Outcome::Done
            }
            State::Configure => {
                setup_offline::downloads_before_configure(golden, backend, opts, &password, ui);
                configure(golden, backend, &keyring, &mut password, ui)
            }
            State::Seal => seal(golden, backend, &password),
            State::Sealed => Outcome::Done,
        };
        let after = golden::next(state, &outcome);
        record_outcome(golden, state, &outcome);
        match &outcome {
            Outcome::Done => {}
            Outcome::UserStopped => {
                m.at(after);
                golden.write_manifest(&m)?;
                ui.line(&text::setup_stopped(after));
                return Ok(EXIT_AGAIN);
            }
            Outcome::Failed(reason) => {
                m.at(after);
                golden.write_manifest(&m)?;
                ui.line(&text::state_failed(state, reason));
                return Ok(EXIT_AGAIN);
            }
            Outcome::ImageRejected(reason) => {
                fetch_retries += 1;
                ui.line(&text::state_failed(state, reason));
                if fetch_retries > 1 {
                    m.at(after);
                    golden.write_manifest(&m)?;
                    return Ok(EXIT_AGAIN);
                }
                if let Some(p) = m.ipsw.take() {
                    if opts.ipsw.is_none() {
                        let _ = std::fs::remove_file(&p);
                    }
                }
                m.ipsw_size = None;
            }
            Outcome::Crashed => unreachable!("the driver never returns Crashed from a state"),
        }
        if after == State::Sealed {
            m.sealed = Some(golden::now_text());
        }
        m.at(after);
        golden.write_manifest(&m)?;
        if after == State::Sealed {
            // The image holds the agent now: the seed folder is a copy.
            if let Err(e) = seed::remove(&golden.seed) {
                log::warn!("macvm: {e:#}");
            }
            let size = golden::used_bytes(&golden.disk) + golden::used_bytes(&golden.aux);
            ui.line(&text::golden_ready(&m.version_text(), size));
            return Ok(0);
        }
    }
}

/// The end of a state in the event log.
fn record_outcome(golden: &Golden, state: State, outcome: &Outcome) {
    let how = match outcome {
        Outcome::Done => "done".to_string(),
        Outcome::UserStopped => "stopped".to_string(),
        Outcome::Failed(r) | Outcome::ImageRejected(r) => format!("failed: {r}"), // not user text
        Outcome::Crashed => "crashed".to_string(),
    };
    let line = format!("{} {how}", state.name()); // not user text
    setup_events::record(&golden.events, setup_events::STATE, false, &line);
}

/// `Guided`: the seed folder must be there for the share. A golden of an
/// older build that passed `Seed` by a write into the image, or a seed
/// folder of another paddock version, gets a new one first.
fn guided(
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    password: &str,
    m: &Manifest,
    ui: &mut Ui,
) -> Outcome {
    if !seed::ready(&golden.seed) {
        let agent = agent_binary(layout, opts);
        if let Err(e) = seed::prepare(&golden.seed, agent.as_deref()) {
            return Outcome::Failed(format!("{e:#}"));
        }
        ui.line(&text::seed_prepared(&golden.seed, agent.is_some()));
    }
    let route = route(golden, backend, opts, m);
    if let Some(why) = route.why() {
        ui.line(&text::route_guided(why));
    }
    if !route.seeded {
        ui.line(&text::seed_install_line(&seed::command()));
    }
    let why = route.why().map(text::guided_why).unwrap_or_default();
    let line = format!(
        // not user text
        "{} seeded {} trust offline {} {why}",
        route.word(),
        route.seeded,
        route.trust_offline
    );
    setup_events::record(&golden.events, provision::ROUTE, false, line.trim_end());
    end_to_outcome(backend.guided(golden, password, &route))
}

/// The route of the guided state (`provision.rs`).
fn route(golden: &Golden, backend: &mut dyn Backend, opts: &Opts, m: &Manifest) -> Route {
    let events = setup_events::read(&golden.events);
    provision::decide(&provision::Inputs {
        forced_guided: opts.guided,
        host: &backend.host().version,
        guest: &m.version,
        support: backend.provisioning(),
        first_boot: provision::first_boot(&events),
        seeded: provision::seeded(&events),
        disk_tool: !opts.guided && backend.disk_tool().is_some(),
    })
}

/// `Configure`: the request to the agent. When the guest refuses the
/// password, the person types the one they set, the keyring keeps it in
/// place of the made one, and the request runs again. Neither password
/// goes to the terminal or to the event log.
fn configure(
    golden: &Golden,
    backend: &mut dyn Backend,
    keyring: &Keyring,
    password: &mut String,
    ui: &mut Ui,
) -> Outcome {
    let mut refused = 0;
    loop {
        match backend.configure(golden, password) {
            Ok(End::WrongPassword) => {}
            r => return end_to_outcome(r),
        }
        refused += 1;
        setup_events::record(
            &golden.events,
            setup_events::PASSWORD_REFUSED,
            false,
            &refused.to_string(),
        );
        if refused >= PASSWORD_TRIES {
            return Outcome::Failed(text::password_refused(refused));
        }
        ui.line(&text::configure_wrong_password());
        let typed = match backend.ask_password(golden) {
            Ok(Some(p)) => p,
            Ok(None) => return Outcome::UserStopped,
            Err(e) => return Outcome::Failed(format!("{e:#}")),
        };
        if let Err(e) = keyring.replace(&typed) {
            return Outcome::Failed(format!("{e:#}"));
        }
        *password = typed;
        setup_events::record(
            &golden.events,
            setup_events::PASSWORD_REPLACED,
            false,
            "keyring",
        );
        ui.line(&text::password_replaced());
    }
}

pub(super) fn end_to_outcome(r: Result<End>) -> Outcome {
    match r {
        Ok(End::Done) => Outcome::Done,
        Ok(End::Stopped) => Outcome::UserStopped,
        Ok(End::WrongPassword) => Outcome::Failed(text::password_refused(1)),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

/// `Fetch`: the file of `--ipsw`, or the newest image from Apple after
/// the space check and the question.
fn fetch(
    layout: &Layout,
    backend: &mut dyn Backend,
    opts: &Opts,
    m: &mut Manifest,
    ui: &mut Ui,
) -> Result<Outcome> {
    if let Some(path) = &opts.ipsw {
        let path = std::path::absolute(path)?;
        let needed = golden::space_needed(State::Install, None);
        if let Some(refusal) = space_refusal(&layout.root, needed, "the golden", opts)? {
            return Ok(Outcome::Failed(refusal));
        }
        return Ok(match backend.check_image(&path) {
            Ok(info) if same_as_current(opts, &info.version) => {
                Outcome::Failed(text::update_same_version(&info.version))
            }
            Ok(info) => {
                take_image(
                    m,
                    &path,
                    info,
                    std::fs::metadata(&path).ok().map(|x| x.len()),
                );
                Outcome::Done
            }
            Err(e) => Outcome::Failed(text::image_check_failed(&path, &format!("{e:#}"))),
        });
    }
    let latest = match backend.latest() {
        Ok(l) => l,
        Err(e) => return Ok(Outcome::Failed(format!("{e:#}"))),
    };
    if same_as_current(opts, &latest.version) {
        return Ok(Outcome::Failed(text::update_same_version(&latest.version)));
    }
    let dest = layout.ipsw_file(&latest.version);
    // A complete download of this version from an earlier setup.
    let have = std::fs::metadata(&dest).map(|x| x.len()).ok();
    if have.is_some() && latest.size.is_none_or(|s| have == Some(s)) {
        if let Ok(info) = backend.check_image(&dest) {
            ui.line(&text::image_already_there(&dest));
            take_image(m, &dest, info, latest.size);
            return Ok(Outcome::Done);
        }
        let _ = std::fs::remove_file(&dest);
    }
    let needed = golden::space_needed(State::Fetch, latest.size);
    if let Some(refusal) = space_refusal(
        &layout.ipsw,
        needed,
        "the download and the golden after it",
        opts,
    )? {
        return Ok(Outcome::Failed(refusal));
    }
    let question = text::fetch_question(
        &latest.url,
        &latest.version,
        text::GoldenSize {
            download_gb: gb_of(latest.size.unwrap_or(golden::IPSW_GUESS_BYTES)),
            disk_gb: gb_of(golden::INSTALL_REAL_BYTES),
        },
        golden::INSTALL_MINUTES,
    );
    if !opts.yes && !(ui.ask)(&question)? {
        return Ok(Outcome::UserStopped);
    }
    crate::privfs::dir(&layout.ipsw)?;
    let partial = layout::partial(&dest);
    for attempt in 0..2 {
        backend.download(&latest.url, &partial, latest.size)?;
        let got = std::fs::metadata(&partial).map(|x| x.len()).unwrap_or(0);
        if let Some(want) = latest.size {
            if got != want {
                let _ = std::fs::remove_file(&partial);
                if attempt == 0 {
                    ui.line(&text::download_short(got, want));
                    continue;
                }
                return Ok(Outcome::Failed(text::download_short_twice(got, want)));
            }
        }
        layout::commit(&partial).with_context(|| text::cannot_commit(&dest))?;
        return Ok(match backend.check_image(&dest) {
            Ok(info) => {
                take_image(m, &dest, info, latest.size);
                Outcome::Done
            }
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                Outcome::Failed(text::image_check_failed(&dest, &format!("{e:#}")))
            }
        });
    }
    unreachable!()
}

/// An `update` without `--force` refuses the version of the current
/// golden: a rebuild of the same macOS is 30 minutes for nothing.
fn same_as_current(opts: &Opts, version: &str) -> bool {
    !opts.force
        && opts
            .current_version
            .as_deref()
            .is_some_and(|c| c == version)
}

fn take_image(m: &mut Manifest, path: &Path, info: ImageInfo, size: Option<u64>) {
    m.version = info.version;
    m.build = info.build;
    m.hardware_model_b64 = info.hardware_model_b64;
    m.ipsw = Some(path.to_path_buf());
    m.ipsw_size = size;
}

/// `Install`: the space check, the image check, the installer, then the
/// commit of the four files, the disk last.
fn install(
    layout: &Layout,
    golden: &Golden,
    backend: &mut dyn Backend,
    opts: &Opts,
    m: &mut Manifest,
    ui: &mut Ui,
) -> Result<Outcome> {
    let Some(ipsw) = m.ipsw.clone().or_else(|| opts.ipsw.clone()) else {
        return Ok(Outcome::ImageRejected(text::image_not_named()));
    };
    let len = std::fs::metadata(&ipsw).map(|x| x.len()).ok();
    if len.is_none() {
        return Ok(Outcome::ImageRejected(text::image_gone(&ipsw)));
    }
    if let (Some(want), Some(got)) = (m.ipsw_size, len) {
        if want != got {
            return Ok(Outcome::ImageRejected(text::image_size_differs(
                &ipsw, got, want,
            )));
        }
    }
    if let Err(e) = backend.check_image(&ipsw) {
        return Ok(Outcome::ImageRejected(text::image_check_failed(
            &ipsw,
            &format!("{e:#}"),
        )));
    }
    let needed = golden::space_needed(State::Install, m.ipsw_size);
    if let Some(refusal) = space_refusal(&golden.dir, needed, "the install", opts)? {
        return Ok(Outcome::Failed(refusal));
    }
    let _ = layout;
    if let Err(e) = backend.install(golden, &ipsw) {
        return Ok(Outcome::Failed(format!("{e:#}")));
    }
    for f in [
        &golden.hardware_model,
        &golden.machine_id,
        &golden.aux,
        &golden.disk,
    ] {
        let p = layout::partial(f);
        if !p.exists() {
            return Ok(Outcome::Failed(text::installer_left_no(&p)));
        }
    }
    for f in [
        &golden.hardware_model,
        &golden.machine_id,
        &golden.aux,
        &golden.disk,
    ] {
        layout::commit(&layout::partial(f)).with_context(|| text::cannot_commit(f))?;
    }
    ui.line(&text::installed(
        &m.version_text(),
        &golden.disk,
        golden::used_bytes(&golden.disk),
    ));
    Ok(Outcome::Done)
}

/// `Seal`: the guest stops, the images become read-only and immutable.
fn seal(golden: &Golden, backend: &mut dyn Backend, password: &str) -> Outcome {
    if let Err(e) = backend.shutdown(golden, password) {
        return Outcome::Failed(format!("{e:#}"));
    }
    for f in golden.sealed_files() {
        if let Err(e) = layout::seal_file(f) {
            return Outcome::Failed(text::seal_failed(f, &e.to_string()));
        }
    }
    Outcome::Done
}

/// The refusal line when the volume of `path` has less than `needed`.
pub fn space_refusal(path: &Path, needed: u64, what: &str, opts: &Opts) -> Result<Option<String>> {
    let free = golden::free_bytes(path, opts.fake_free.as_deref())?;
    if free >= needed {
        return Ok(None);
    }
    Ok(Some(text::setup_no_space(
        needed,
        free,
        what,
        volume_of(path),
    )))
}

/// Whole decimal gigabytes of a size, for the question.
fn gb_of(bytes: u64) -> u32 {
    (bytes as f64 / 1e9).round() as u32
}

/// The nearest existing parent of a path, for the space line.
fn volume_of(path: &Path) -> &Path {
    let mut p = path;
    while !p.exists() {
        match p.parent() {
            Some(parent) => p = parent,
            None => return path,
        }
    }
    p
}

/// The question on the terminal, for the helper. Without a terminal, the
/// line names `--yes` and the answer is no.
pub fn terminal_ask(question: &str) -> Result<bool> {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        println!("{}", text::ask_without_terminal(question));
        return Ok(false);
    }
    eprint!("{}", text::ask_prompt(question));
    std::io::stderr().flush().ok();
    let mut a = String::new();
    std::io::stdin().read_line(&mut a)?;
    Ok(matches!(a.trim(), "y" | "Y" | "yes"))
}

/// One setup at a time per host. The lock is tried for two seconds
/// before the refusal: a process that forked while the last setup held
/// the lock keeps it until its exec, for a moment.
fn lock(layout: &Layout) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    crate::privfs::dir(&layout.root).with_context(|| text::cannot_make(&layout.root))?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&layout.setup_lock)
        .with_context(|| text::cannot_open(&layout.setup_lock))?;
    for attempt in 0..20 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // SAFETY: a plain flock on a file that we own.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(f);
        }
    }
    Err(anyhow!(text::setup_lock_held(&layout.setup_lock)))
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
