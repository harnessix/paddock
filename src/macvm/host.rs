//! The parent side of an app session: `paddock app run NAME --backend
//! macvm` finds the golden, sweeps the dead session records, makes the
//! token, starts the `paddock-vm app` shim with the token on a pipe, writes
//! the `Running` record and waits. The shim lives on when this process
//! ends: the session continues after the terminal closes.

use super::golden::State;
pub use super::host_record::{RecordApp, RecordHealth, RecordShare, RecordStart, Session};
use super::layout::{Golden, Layout};
use super::session::{Profile, Stamp};
use super::text::{self, GoldenSize, SessionEnd, Share};
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// The home id of a profile until `[apps.NAME] home` exists.
pub const DEFAULT_HOME: &str = "default";
/// The display of the guest when paddock cannot read the host screens.
pub const DEFAULT_DISPLAY: &str = "2560x1600@2";
/// The exit code of a refused start: the lock, the slots, the golden.
pub const EXIT_REFUSED: i32 = 2;
/// The `--backend` name of the macOS guest.
pub const BACKEND: &str = "macvm";
/// `=1`: `paddock app run` prints the session token and the guest
/// password to its own stderr, for the Mac check scripts that grep the
/// logs and the report for them. The shim never prints them.
pub const PRINT_SECRETS_VAR: &str = "PADDOCK_MACVM_PRINT_SECRETS";

/// The size of the golden build, for the refusal without one.
pub const GOLDEN_SIZE: GoldenSize = GoldenSize {
    download_gb: 15,
    disk_gb: 25,
};
/// The minutes of the golden build, for the refusal without one.
pub const SETUP_MINUTES: u32 = 30;

/// One workspace folder in the record.
/// A valid profile name: the folder under `profiles/`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// The golden version when it is sealed, else the refusal that names
/// `paddock macvm setup`. `app` is the app of the session, for the line.
pub fn sealed(golden: &Golden, app: &str) -> Result<String> {
    match golden.read_manifest()? {
        None => bail!(text::no_golden(app, GOLDEN_SIZE, SETUP_MINUTES)),
        Some(m) if m.state != State::Sealed => bail!(text::golden_unsealed()),
        Some(m) => Ok(m.version_text()),
    }
}

/// Every session record, with its path.
pub fn sessions(dir: &Path) -> Vec<(PathBuf, Session)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") {
            if let Ok(s) = std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str::<Session>(&t).map_err(|e| e.to_string()))
            {
                out.push((p, s));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { libc::kill(pid as i32, 0) } == 0
}

/// Remove the records of sessions whose shim is gone, ask for their
/// input source back, and log the end in the profile's event log. Returns
/// the dead sessions.
pub fn sweep(
    dir: &Path,
    alive: &dyn Fn(u32) -> bool,
    restore: &mut dyn FnMut(&str),
) -> Vec<Session> {
    let mut dead = Vec::new();
    for (path, s) in sessions(dir) {
        if alive(s.pid) {
            continue;
        }
        let _ = std::fs::remove_file(&path);
        if !s.input_source.is_empty() {
            restore(&s.input_source);
        }
        let profile = Profile::new(&s.profile);
        if profile.dir.is_dir() {
            let clean = Stamp::read(&profile.stamp())
                .map(|s| s.clean)
                .unwrap_or(false);
            let ev = super::events::Events::open(&profile);
            if clean {
                ev.record(
                    "stop",
                    "the session ended without its paddock: the stamp says clean",
                    &[("pid", s.pid as f64)],
                );
            } else {
                ev.record(
                    "crashed",
                    "session ended without a save",
                    &[("pid", s.pid as f64)],
                );
            }
        }
        dead.push(s);
    }
    dead
}

/// The running session of a profile, or None.
pub fn running(layout: &Layout, app: &str, home: &str) -> Option<Session> {
    let text = std::fs::read_to_string(layout.record(app, home)).ok()?;
    let s: Session = serde_json::from_str(&text).ok()?;
    alive(s.pid).then_some(s)
}

/// How a stopped profile ended, from its stamp.
pub fn last_end(profile: &Profile) -> SessionEnd {
    end_of(Stamp::read(&profile.stamp()).ok().as_ref())
}

/// How the last session ended, from the stamp it left. No stamp: the
/// profile never ran.
pub fn end_of(stamp: Option<&Stamp>) -> SessionEnd {
    match stamp {
        Some(s) if s.clean && s.state_id.is_some() => SessionEnd::QuitWithSave,
        Some(s) if s.clean => SessionEnd::QuitWithoutSave,
        Some(_) => SessionEnd::Crashed,
        None => SessionEnd::NeverStarted,
    }
}

/// What the shim gets.
#[derive(Debug, Clone)]
pub struct StartArgs {
    pub app: String,
    pub home: String,
    pub shares: Vec<(String, PathBuf, bool)>,
    pub display: String,
    pub cpus: u32,
    pub memory_mib: u64,
    pub save: bool,
    pub token: String,
    /// The guest password for `data.prepare`, when the keyring has it.
    pub password: Option<String>,
    pub input_source: String,
    /// The share folder of the agent of this paddock version,
    /// `agent/<version>/`, when the install has the agent.
    pub agent: Option<PathBuf>,
    /// The clone of the host app bundle in the profile.
    pub app_bundle: Option<PathBuf>,
    /// The guest may drop the quarantine attribute from the copy.
    pub app_unquarantine: bool,
    /// The guest paths that the app gets as arguments.
    pub app_args: Vec<String>,
    /// The network mode word of the session.
    pub network: String,
    /// The host list of mode api or allowlist: the endpoints of the
    /// profile and `[network] allow` of the user configuration.
    pub allow: Vec<String>,
    /// The session honors the bench ops.
    pub bench: bool,
}

/// The pid of the shim, for the signal handler.
static SHIM_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_signal(sig: libc::c_int) {
    let pid = SHIM_PID.load(Ordering::Relaxed);
    if pid > 0 {
        // Only kill(2) here: it is safe in a signal handler.
        unsafe { libc::kill(pid, sig) };
    }
}

/// The command line of the shim, without the token.
pub fn command_args(
    layout: &Layout,
    args: &StartArgs,
    control: &Path,
    record: &Path,
) -> Vec<String> {
    let profile = layout.profile(&args.app, &args.home);
    let mut v = vec![
        "app".to_string(),
        "--profile".into(),
        profile.dir.display().to_string(),
        "--golden".into(),
        layout.golden.dir.display().to_string(),
        "--app".into(),
        args.app.clone(),
        "--token-fd".into(),
        "3".into(),
        "--cpus".into(),
        args.cpus.to_string(),
        "--memory".into(),
        args.memory_mib.to_string(),
        "--display".into(),
        args.display.clone(),
        "--control".into(),
        control.display().to_string(),
        "--record".into(),
        record.display().to_string(),
    ];
    for (name, path, ro) in &args.shares {
        v.push("--share".into());
        v.push(format!(
            "{name}={}{}",
            path.display(),
            if *ro { ":ro" } else { "" }
        ));
    }
    if !args.save {
        v.push("--no-save".into());
    }
    if args.bench {
        v.push("--bench".into());
    }
    if !args.input_source.is_empty() {
        v.push("--input-source".into());
        v.push(args.input_source.clone());
    }
    if let Some(agent) = &args.agent {
        v.push("--agent".into());
        v.push(agent.display().to_string());
    }
    if let Some(bundle) = &args.app_bundle {
        v.push("--app-bundle".into());
        v.push(bundle.display().to_string());
        if args.app_unquarantine {
            v.push("--app-unquarantine".into());
        }
    }
    for a in &args.app_args {
        v.push("--app-arg".into());
        v.push(a.clone());
    }
    v.push("--network".into());
    v.push(args.network.clone());
    for h in &args.allow {
        v.push("--allow".into());
        v.push(h.clone());
    }
    v
}

/// Start the shim in its own session, with the token (and the password)
/// on fd 3, its output in the profile's `logs/shim.log`.
pub fn start(
    layout: &Layout,
    shim: &Path,
    args: &StartArgs,
    control: &Path,
    record: &Path,
) -> Result<std::process::Child> {
    let profile = layout.profile(&args.app, &args.home);
    crate::privfs::dir(&profile.logs()).or_else(|_| -> Result<()> {
        std::fs::create_dir_all(profile.logs())?;
        Ok(())
    })?;
    // The shim log rotates here, before the session, and in the shim
    // every 10 s (5 files of 1 MiB).
    let log = super::logs::open_shim_log(&profile)
        .with_context(|| text::cannot_open(&profile.shim_log()))?;
    let (read, mut write) = token_pipe()?;
    let mut cmd = std::process::Command::new(shim);
    cmd.args(command_args(layout, args, control, record))
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // The shim brings the window marks back while a firewall question
    // waits (`paddock mark`); it is not the paddock binary itself.
    if let Ok(me) = std::env::current_exe() {
        cmd.env(crate::mark::EXE_VAR, me);
    }
    let read_fd = read.as_raw_fd();
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid and dup2 are async-signal-safe. The shim outlives
    // this process by design.
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            if libc::dup2(read_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd
        .spawn()
        .with_context(|| text::cannot_run(&shim.display().to_string()))?;
    drop(read);
    let mut text = format!("{}\n", args.token); // not user text
    if let Some(p) = &args.password {
        text.push_str(p);
        text.push('\n');
    }
    write
        .write_all(text.as_bytes())
        .with_context(|| text::token_pipe_failed("write"))?;
    drop(write);
    Ok(child)
}

fn token_pipe() -> Result<(OwnedFd, std::fs::File)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: a two-element array for pipe(2).
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| text::token_pipe_failed("make"));
    }
    for fd in fds {
        // SAFETY: the fd is open; close on exec, so the shim sees only fd 3.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: both fds are open and owned here.
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    })
}

/// Whether the parent of a session waits for it. A terminal waits and
/// forwards Ctrl-C; a Dock launcher returns as soon as the session is up,
/// so the Dock shows one icon, the session's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    Terminal,
    Detach,
}

/// `paddock app run NAME --backend macvm`.
pub fn run_app(env: &HostEnv, o: &crate::app::AppOptions) -> Result<i32> {
    run_app_mode(env, o, Wait::Terminal)
}

/// `paddock app run NAME --backend macvm`, waiting or not.
pub fn run_app_mode(env: &HostEnv, o: &crate::app::AppOptions, wait: Wait) -> Result<i32> {
    if !cfg!(target_os = "macos") {
        bail!(text::macvm_macos_only());
    }
    if !valid_name(&o.name) {
        bail!(text::bad_app_name(&o.name));
    }
    let layout = Layout::new(env);
    if let Some(r) = super::golden::recover(&layout)? {
        eprintln!("{}", r.line());
    }
    let golden = sealed(&layout.golden, &o.name)?;
    for d in [&layout.sessions, &layout.profiles] {
        crate::privfs::dir(d)?;
    }
    let helper = crate::vm::host::helper_path(env)?;
    let agent = agent_share(&layout, &helper);
    for line in super::host_mac::sweep_and_restore(&layout) {
        eprintln!("{line}");
    }
    let user = crate::manifest::Manifest::load(env, &env.home.join(".paddock-no-project"))
        .map(|m| m.merged)
        .ok();
    let auto_suspend =
        super::host_suspend::auto_suspend(user.as_ref().and_then(|u| u.macvm.as_ref()));
    let n = super::instance::choose(
        o.guest.instance,
        o.guest.new_instance,
        &super::instance::existing(&layout, &o.name),
    )
    .map_err(anyhow::Error::msg)?;
    let home = &super::instance::folder(n);
    let key = super::instance::key(&o.name, n);
    let label = super::instance::label(&o.name, n);
    let facts = super::limits::host_facts();
    if let Some(line) = super::limits::refusal(facts.as_ref(), &layout.sessions, &key, auto_suspend)
    {
        eprintln!("{line}");
        return Ok(EXIT_REFUSED);
    }
    match super::host_suspend::before_start(&layout, &o.name, home, o.guest.resumed_by.as_deref()) {
        Ok(None) => {}
        Ok(Some(s)) => {
            eprintln!("{}", text::profile_running(&s.arg(), s.pid));
            return Ok(EXIT_REFUSED);
        }
        Err(line) => {
            eprintln!("{line}");
            return Ok(EXIT_REFUSED);
        }
    }
    // The space floor: below the room of a clone, the automatic cleanup
    // removes what comes back by itself, before this start stages its app.
    let free = super::session::free_bytes(&layout.root);
    if free < super::session::CLONE_NEEDS {
        eprintln!("{}", text::low_space_cleanup(&crate::storage::human(free)));
        super::manage::auto_cleanup(env)
            .iter()
            .for_each(|l| eprintln!("{l}"));
    }
    // The shares: the roots as ws-<n> after the refusals and the secrets
    // check, then drop, downloads and apps of the profile. The shim adds
    // the agent share from --agent.
    let profile = layout.profile(&o.name, home);
    let roots: Vec<super::shares::Root> = o
        .roots
        .iter()
        .chain(o.ro.iter().filter(|r| !o.roots.contains(r)))
        .map(|r| super::shares::Root {
            path: r.clone(),
            read_only: o.ro.contains(r),
            unprotected: false,
        })
        .collect();
    let (roots, saved) = super::grants::for_start(&profile, &roots);
    let built = super::shares::build(
        &o.name,
        &roots,
        &profile,
        None,
        &env.home,
        &super::grants::paddock_dirs(env),
        crate::app::unprotected_workspaces(env, &o.name)?,
    )?;
    for w in &built.warnings {
        eprintln!("{w}");
    }
    for d in [profile.drop_dir(), profile.downloads(), profile.apps()] {
        crate::privfs::dir(&d)?;
    }
    let shares: Vec<(String, PathBuf, bool)> = built
        .shares
        .iter()
        .map(|s| (s.name.clone(), s.host.clone(), s.read_only))
        .collect();
    let cfg = user.as_ref().and_then(|u| u.apps.get(&o.name));
    let save = user
        .as_ref()
        .and_then(|u| u.macvm.as_ref())
        .and_then(|m| m.save)
        .unwrap_or(true);
    // The app: the host bundle, the clone into the profile that the `apps`
    // share carries into the guest, and the Dock face of the session. A
    // name without an app profile and without `[apps.NAME] bundle` is a
    // session that boots the guest and starts no app of its own, which the
    // Mac check scripts use.
    let app_profile = crate::app::app_profile(env, &o.name).ok();
    let bundle = super::host_app::optional_bundle(
        Path::new(super::host_app::SYSTEM_ROOT),
        env,
        &o.name,
        app_profile.as_ref(),
        cfg,
    )?;
    // A start of an instance that runs or starts already stops here.
    let Some(will_copy) = super::host_app::stage_locked(&profile, &o.name, bundle.as_ref())? else {
        return Ok(EXIT_REFUSED);
    };
    let shim = super::host_app::shim_exe(
        &layout,
        &o.name,
        n,
        bundle.as_ref(),
        &helper,
        super::version::paddock_version(),
    )?;
    // An explicit network wins; the saved one is read under the lock below.
    let network = match o.network {
        None => None,
        Some(crate::spec::Network::Host) => bail!(text::network_host_refused()),
        Some(crate::spec::Network::None) => Some("none".to_string()),
        Some(crate::spec::Network::Egress) => Some("egress".to_string()),
        Some(crate::spec::Network::Allowlist) => Some("allowlist".to_string()),
        Some(crate::spec::Network::Api) => Some("api".to_string()),
    };
    let mut args = StartArgs {
        app: o.name.clone(),
        home: home.into(),
        shares,
        display: super::host_mac::display_arg(env),
        cpus: 4,
        memory_mib: super::host_app::memory_mib(&o.name, cfg, super::host_app::host_memory_gib())?,
        save,
        token: crate::vm::proto::new_token()?,
        password: super::data_start::start_password(env),
        input_source: String::new(),
        agent,
        app_bundle: bundle.as_ref().map(|b| b.clone_path(&profile)),
        app_unquarantine: bundle
            .as_ref()
            .is_some_and(|b| !super::appcopy::quarantined_unapproved(&b.path)),
        app_args: super::host_app::app_args(app_profile.as_ref(), &built.shares),
        network: String::new(),
        allow: {
            let mut v: Vec<String> = app_profile
                .as_ref()
                .map(|p| p.network.endpoints.clone())
                .unwrap_or_default();
            v.extend(
                user.as_ref()
                    .map(|u| u.network.allow.clone())
                    .unwrap_or_default(),
            );
            v
        },
        bench: o.guest.bench,
    };
    let control = crate::sockpath::host_socket(
        &env.paddock_run(),
        env.uid,
        &format!("macvm-{key}"), // not user text
        "control.sock",
    )?;
    let record = layout.record(&o.name, home);
    let display: super::session::Display = args
        .display
        .parse()
        .unwrap_or_else(|_| DEFAULT_DISPLAY.parse().expect("the default display parses"));
    // A locked data disk is refused here, before any shim.
    let predicted = match super::start::predict(&profile, &golden, display) {
        Ok(s) => s,
        Err(refusal) => {
            eprintln!("{refusal}");
            return Ok(1);
        }
    };
    if let Ok(s) = Stamp::read(&profile.stamp()).map(|s| s.golden) {
        if s != golden && !s.is_empty() {
            eprintln!("{}", text::golden_newer(&o.name, &s, &golden));
        }
    }
    if auto_suspend {
        let me = env.user.clone();
        let full = || {
            super::lock::holders(Path::new(super::lock::SLOT_DIR), &me)
                .iter()
                .all(Option::is_some)
        };
        match super::host_suspend::make_room(
            &layout.sessions,
            &key,
            &label,
            &full,
            &super::host_suspend::ask,
            super::suspend::WAIT,
        ) {
            Ok(lines) => lines.iter().for_each(|l| eprintln!("{l}")),
            // The shim's own slot refusal names the holders.
            Err(None) => {}
            Err(Some(line)) => {
                eprintln!("{line}");
                return Ok(EXIT_REFUSED);
            }
        }
    }
    // The grants lock until the record exists: no change falls between.
    let Some((policy, word)) = super::grants::start_network(&profile, &saved, network) else {
        return Ok(EXIT_REFUSED);
    };
    args.network = word;
    let mut child = start(&layout, &shim, &args, &control, &record)?;
    // The window marks and the firewall panel of mode ask.
    crate::mark::spawn(env);
    let session = Session {
        pid: child.id(),
        app: o.name.clone(),
        home: home.into(),
        profile: layout.profile(&o.name, home).dir,
        slot: None,
        input_source: args.input_source.clone(),
        started: now(),
        control: control.clone(),
        bench: args.bench,
        badge: None,
        health: None,
        start: None,
        app_info: None,
        network: None,
        network_mode: args.network.clone(),
        shares: args
            .shares
            .iter()
            .filter(|(name, _, _)| name.starts_with(super::shares::WS_PREFIX))
            .map(|(_, path, ro)| RecordShare {
                path: path.clone(),
                read_only: *ro,
                ..Default::default()
            })
            .collect(),
        display_name: super::host_running::display_name(&o.name, n, bundle.as_ref()),
        instance: n,
        last_key: now(),
        ..Default::default()
    };
    crate::privfs::write_atomic(&record, serde_json::to_vec_pretty(&session)?)?;
    super::host_running::register(env, &session, &o.roots, &args.network);
    drop(policy);
    if env.var(PRINT_SECRETS_VAR).is_some_and(|v| v == "1") {
        // The Mac check scripts read the secrets from this stderr to
        // prove that no log holds them. Nothing else prints them.
        let password = super::golden::Keyring::open(env)
            .and_then(|k| k.password())
            .ok();
        eprintln!(
            "{}",
            text::secrets_printed(&args.token, password.as_deref())
        );
    }
    let shares: Vec<Share<'_>> = args
        .shares
        .iter()
        .filter(|(name, _, _)| name.starts_with(super::shares::WS_PREFIX))
        .map(|(_, path, ro)| Share {
            path,
            writable: !ro,
        })
        .collect();
    // The start lines. The decision here is the estimate the user is told;
    // the shim decides again under the locks.
    let downloads = layout.profile(&o.name, home).downloads();
    let plan = super::start::Plan {
        app: &o.name,
        golden: &golden,
        memory_mib: args.memory_mib,
        display,
        mixed_scale: super::host_mac::host_screens().is_some_and(|s| s.mixed_scale()),
        shares,
        downloads: &downloads,
        network: &args.network,
        start: predicted,
        copy: bundle
            .as_ref()
            .and_then(|b| super::start::copy_part(b, will_copy)),
    };
    for line in super::start::lines(&plan) {
        eprintln!("{line}");
    }
    if wait == Wait::Detach {
        // The session has its own Dock face and lives on by itself.
        return Ok(0);
    }
    SHIM_PID.store(child.id() as i32, Ordering::Relaxed);
    let handler = forward_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a handler that only calls kill(2).
        unsafe { libc::signal(sig, handler) };
    }
    let status = child.wait()?;
    SHIM_PID.store(0, Ordering::Relaxed);
    let code = crate::vm::term::exit_code(status);
    if code != 0 {
        let profile = Profile::new(&session.profile);
        for line in tail(&profile.shim_log(), 5) {
            eprintln!("{line}");
        }
    }
    Ok(code)
}

/// The agent share of this paddock version, made from `paddock-guest` next
/// to the shim. A share that cannot be made is one line, not a refusal: the
/// session runs with the agent of the golden.
pub fn agent_share(layout: &Layout, shim: &Path) -> Option<PathBuf> {
    let install = shim.parent()?;
    match layout.agent_share(install, super::version::paddock_version()) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("{}", text::agent_share_failed(&format!("{e:#}")));
            None
        }
    }
}

/// The last `n` lines of a file.
pub fn tail(path: &Path, n: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// `paddock app stop NAME [--instance N]`: the quit path of the session,
/// then wait for the shim to end. A suspended session has its state
/// saved already: its shim ends at once. `Ok(false)` when nothing runs.
pub fn stop(env: &HostEnv, app: &str, n: u32, wait: Duration) -> Result<bool> {
    let layout = Layout::new(env);
    let Some(s) = running(&layout, app, &super::instance::folder(n)) else {
        return Ok(false);
    };
    let name = &s.arg();
    if s.suspended {
        super::host_suspend::term(s.pid);
    }
    // A frozen session is thawed first; the shim resumes by itself too.
    let id = super::host_running::running_id(&s.key());
    match crate::freeze::macvm::thaw_before_quit(env, &id) {
        _ if s.suspended => {}
        Ok(true) => eprintln!("{}", text::thaw_before_quit(name)),
        Ok(false) => {}
        Err(e) => log::warn!("macvm: thaw before the quit: {e:#}"),
    }
    if !s.suspended && crate::vm::host::control(&s.control, "stop").is_err() {
        // SAFETY: a stop signal to the shim that the record names.
        unsafe { libc::kill(s.pid as i32, libc::SIGTERM) };
    }
    let end = Instant::now() + wait;
    while alive(s.pid) {
        if Instant::now() > end {
            bail!(text::stop_timeout(name, s.pid, wait));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(true)
}

/// `paddock app files NAME`: the downloads folder of the profile, opened
/// in the Finder on macOS.
pub fn files(env: &HostEnv, app: &str, n: u32) -> Result<i32> {
    if !valid_name(app) {
        bail!(text::bad_app_name(app));
    }
    let layout = Layout::new(env);
    let downloads = layout.profile(app, &super::instance::folder(n)).downloads();
    crate::privfs::dir(&downloads)?;
    let name = super::instance::arg(app, n);
    println!("{}", text::downloads_folder(&name, &downloads));
    if cfg!(target_os = "macos") {
        let status = std::process::Command::new("/usr/bin/open")
            .arg(&downloads)
            .status()
            .with_context(|| text::cannot_run("open"))?;
        if !status.success() {
            return Ok(1);
        }
    }
    Ok(0)
}

/// `paddock app list`: one line per profile of the macOS guest.
pub fn list_lines(env: &HostEnv) -> Vec<String> {
    let layout = Layout::new(env);
    let mut out = super::host_mac::sweep_and_restore(&layout);
    for (app, home, _) in layout.profile_dirs() {
        let profile = layout.profile(&app, &home);
        let state = match running(&layout, &app, &home) {
            Some(s) if s.suspended => text::list_suspended(s.pid),
            Some(s) => text::list_running(
                s.pid,
                &crate::vm::host::duration(now().saturating_sub(s.started)),
            ),
            None => text::list_stopped(text::session_end(last_end(&profile))),
        };
        let name = match super::instance::number(&home) {
            Some(n) => super::instance::label(&app, n),
            None => format!("{app}/{home}"), // not user text
        };
        out.push(text::list_line(&name, &state));
    }
    out
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
