//! App mode: desktop agent apps (Electron apps such as ChatGPT/Codex Desktop,
//! and flatpak apps) in a sandbox.
//!
//! The app's windows are normal windows on the host desktop, through a
//! restricted Wayland socket (wlsec.rs). Its session bus is a private
//! `dbus-daemon` inside the sandbox, with a private keyring and paddock's
//! link and notification services (keyring.rs, portal.rs). With
//! `[apps.NAME] bus = "host-filtered"` it is the host bus through a filter
//! that passes only the desktop portals (open a URL, choose a file), the
//! notification service and the tray. Either way, the host keyring, the
//! systemd user manager and every other host service stay out of reach.

use crate::env::HostEnv;
use crate::spec::{sandbox_runtime_dir, SandboxSpec};
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Bus names an app may talk to.
pub const BUS_TALK: &[&str] = &[
    "org.freedesktop.portal.Desktop",
    "org.freedesktop.portal.Documents",
    "org.freedesktop.Notifications",
    "org.kde.StatusNotifierWatcher",
];

/// Bus names an app may own. None: xdg-dbus-proxy accepts only whole names or
/// a ".*" suffix, and "org.kde.*" would let the app impersonate host
/// services. Tray icons that need their own bus name may not appear.
pub const BUS_OWN: &[&str] = &[];

/// Lives while the app runs. Dropping it stops the bus proxy and closes the
/// restricted Wayland socket.
pub struct AppSession {
    _wayland: crate::wlsec::RestrictedSocket,
    proxy: Option<Child>,
    _forwarder: Option<ports::Forwarder>,
}

impl Drop for AppSession {
    fn drop(&mut self) {
        if let Some(p) = &mut self.proxy {
            let _ = p.kill();
            let _ = p.wait();
        }
    }
}

/// The session bus of an app, from the user configuration only: a project
/// file cannot give an app the host bus.
pub fn bus_mode(env: &HostEnv, name: &str) -> Result<AppBus> {
    let user = Manifest::load(env, &env.home.join(".paddock-no-project"))?.merged;
    Ok(user.apps.get(name).and_then(|a| a.bus).unwrap_or_default())
}

/// Where links for the app arrive inside the sandbox.
pub const INBOUND_DIR: &str = "/run/paddock/inbound";

pub fn start_session(
    env: &HostEnv,
    spec: &mut SandboxSpec,
    profile: &Profile,
) -> Result<AppSession> {
    let run = env.paddock_run().join(&spec.id);
    std::fs::create_dir_all(&run)?;

    // Link handoff: a helper in the sandbox listens on a socket in a folder
    // that the host can also reach. `paddock app open` writes a link there,
    // and the helper starts the app with it, inside the same sandbox, where
    // the app's single-instance lock passes it to the running window.
    let schemes = profile
        .app
        .as_ref()
        .map(|a| a.schemes.clone())
        .unwrap_or_default();
    let user = Manifest::load(env, &env.home.join(".paddock-no-project"))?.merged;
    let login_ports = user.apps.get(&profile.name).is_some_and(|a| a.login_ports);
    // On the private bus, links go through the host passthrough channel, so
    // paddock knows the login callback port of each login page it opens,
    // the same way as for terminal harnesses (login.rs).
    let callbacks = spec.private_bus && !login_ports;
    let inbound = run.join("inbound");
    if !schemes.is_empty() || login_ports || callbacks {
        std::fs::create_dir_all(&inbound)?;
        spec.add_mount(Mount::bind(
            &inbound,
            INBOUND_DIR,
            true,
            "app mode: link handoff",
        ));
        // Links start the app itself, on the bus that already runs.
        let app_cmd = match spec.command.split_first() {
            Some((w, rest)) if spec.private_bus && w == crate::keyring::TOOL => rest,
            _ => &spec.command[..],
        };
        spec.set_env(
            "PADDOCK_INBOUND_CMD",
            serde_json::to_string(app_cmd)?,
            "app mode: link handoff",
        );
        spec.set_env(
            "PADDOCK_INBOUND_SCHEMES",
            schemes.join(","),
            "app mode: link handoff",
        );
        if callbacks {
            spec.set_env("PADDOCK_INBOUND_PORTS", "1", "app mode: login callbacks");
        }
        if login_ports {
            spec.set_env("PADDOCK_INBOUND_PORTS", "1", "app mode: login_ports");
            spec.warnings.push(format!(
                "login_ports is on for {}: ports that it listens on are open on 127.0.0.1 of this host while it listens",
                profile.name
            ));
        }
        spec.setup.push("paddock inbound >/dev/null 2>&1 &".into());
    }
    let forwarder = if login_ports {
        Some(ports::Forwarder::start(inbound.clone()))
    } else if callbacks {
        Some(ports::Forwarder::start_filtered(
            inbound.clone(),
            crate::login::allowed,
        ))
    } else {
        None
    };

    let wl_path = run.join("wayland");
    let wayland = crate::wlsec::create(&wl_path, &format!("paddock.{}", spec.harness), &spec.id)?;
    spec.gui.host_socket = Some(wl_path);

    if spec.private_bus {
        // The bus itself starts inside the sandbox (keyring.rs). Apps that
        // open links with xdg-open instead of the portal reach the host the
        // same way.
        crate::login::open_shim(env, spec, "app mode: links to the host browser")?;
        spec.set_env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!(
                "unix:path={}",
                sandbox_runtime_dir(spec.uid).join("bus").display()
            ),
            "app mode: private session bus",
        );
        return Ok(AppSession {
            _wayland: wayland,
            proxy: None,
            _forwarder: forwarder,
        });
    }
    let bus_addr = env
        .var("DBUS_SESSION_BUS_ADDRESS")
        .context("app mode needs the host session bus (DBUS_SESSION_BUS_ADDRESS is not set)")?
        .to_string();
    let proxy_exe = env
        .which("xdg-dbus-proxy")
        .context("app mode needs xdg-dbus-proxy (package xdg-dbus-proxy)")?;
    let bus_path = run.join("bus");
    let _ = std::fs::remove_file(&bus_path);
    let mut cmd = Command::new(proxy_exe);
    cmd.arg(&bus_addr).arg(&bus_path).arg("--filter");
    for n in BUS_TALK {
        cmd.arg(format!("--talk={n}"));
    }
    for n in BUS_OWN {
        cmd.arg(format!("--own={n}"));
    }
    let mut proxy = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    while !crate::gui::is_socket(&bus_path) {
        if let Some(st) = proxy.try_wait()? {
            bail!("xdg-dbus-proxy exited with {st}");
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = proxy.kill();
            bail!("xdg-dbus-proxy did not create its socket");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    spec.bus_socket = Some(bus_path);
    let rt: PathBuf = sandbox_runtime_dir(spec.uid);
    spec.set_env(
        "DBUS_SESSION_BUS_ADDRESS",
        format!("unix:path={}", rt.join("bus").display()),
        "app mode: filtered host bus",
    );
    Ok(AppSession {
        _wayland: wayland,
        proxy: Some(proxy),
        _forwarder: forwarder,
    })
}

use crate::build;
#[cfg(not(target_os = "macos"))]
use crate::build::Options;
use crate::detect::{self, Detector};
use crate::manifest::{AppBus, Manifest};
use crate::profile::{HarnessFactory, Profile};
#[cfg(not(target_os = "macos"))]
use crate::spec::GuiMode;
use crate::spec::{Mount, MountKind, Network};

/// Which route an app takes on macOS: the guest VM or Seatbelt.
#[path = "app/macroute.rs"]
mod macroute;
pub use macroute::{app_label, mac_route, MacRoute};

#[derive(Default)]
pub struct AppOptions {
    pub name: String,
    pub roots: Vec<PathBuf>,
    pub ro: Vec<PathBuf>,
    pub gpu: bool,
    pub network: Option<Network>,
    pub inherit_env: Option<bool>,
    pub auth: Option<String>,
    /// Extra arguments for the app, for example a link to open.
    pub args: Vec<String>,
    /// bwrap (default), docker, podman or srt. Flatpak apps need bwrap.
    pub backend: Option<String>,
    /// Open the first workspace folder in a new git worktree (`[apps.NAME] worktree` too).
    pub worktree: bool,
    /// The options of the macOS guest only: the bench, the instance.
    pub guest: crate::macvm::host_record::GuestOptions,
}

pub fn app_profile(env: &HostEnv, name: &str) -> Result<Profile> {
    let f = HarnessFactory::new(env)?;
    let p = f
        .profile(name)
        .with_context(|| format!("unknown app '{name}'. See paddock app list"))?;
    if p.app.is_none() {
        bail!("{name} is a terminal harness: run it with `paddock {name}`");
    }
    Ok(p.clone())
}

/// The workspace folders: from the command line, else `[apps.NAME] roots` in
/// the user configuration.
pub fn roots(env: &HostEnv, name: &str, cli: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let raw: Vec<PathBuf> = if !cli.is_empty() {
        cli.to_vec()
    } else {
        let user = Manifest::load(env, &env.home.join(".paddock-no-project"))?.merged;
        user.apps
            .get(name)
            .map(|a| a.roots.iter().map(|r| env.expand(r, &env.home)).collect())
            .unwrap_or_default()
    };
    if raw.is_empty() {
        bail!(
            "no workspace folders for {name}. Name them: paddock app run {name} ~/src/project, \
             or save them: paddock app install {name} ~/src/project"
        );
    }
    // The roots are resolved again at each start. A symlink on the way inside
    // a folder a sandbox can write (another root, a sandbox home) is refused:
    // a sandbox could have planted it to point the root somewhere else.
    let data = [
        env.paddock_data().join("homes"),
        env.paddock_data().join("worktrees"),
    ];
    let base: Vec<PathBuf> = data
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let raw: Vec<PathBuf> = raw
        .into_iter()
        .map(|r| std::path::absolute(&r).unwrap_or(r))
        .collect();
    let first: Vec<PathBuf> = raw
        .iter()
        .map(|r| crate::safefs::resolve_approved(r, &base))
        .collect::<Result<_>>()?;
    let mut out = Vec::new();
    for (i, r) in raw.iter().enumerate() {
        let mut writable = base.clone();
        writable.extend(
            first
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, p)| p.clone()),
        );
        let c = crate::safefs::resolve_approved(r, &writable)?;
        if crate::paths::same_file(&c, &env.home) || crate::paths::within(&env.home, &c) {
            bail!(
                "{} is the home directory or contains it; name project folders instead",
                c.display()
            );
        }
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Mount the other workspace folders, with the same git and secrets
/// protection as the first one.
fn add_roots(env: &HostEnv, spec: &mut SandboxSpec, m: &Manifest, extra: &[PathBuf]) -> Result<()> {
    for root in extra {
        spec.add_mount(Mount::same(root, true, "app workspace folder"));
        let cx = detect::Context {
            env,
            project: root,
            manifest: m,
        };
        detect::Git.apply(&cx, spec)?;
        detect::Secrets.apply(&cx, spec)?;
    }
    Ok(())
}

/// The extra arguments for the app: the workspace folders for editors that
/// open them (`open_roots`), then links or other arguments.
pub fn app_args(profile: &Profile, roots: &[PathBuf], extra: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    if profile.app.as_ref().is_some_and(|a| a.open_roots) {
        v.extend(roots.iter().map(|r| r.display().to_string()));
    }
    v.extend(extra.iter().cloned());
    v
}

pub fn run(env: &HostEnv, o: &AppOptions) -> Result<i32> {
    crate::storage::guard::before_run(env, o.roots.first().map(PathBuf::as_path), false)?;
    if o.backend.as_deref() == Some(crate::macvm::host::BACKEND) {
        return crate::macvm::host::run_app(env, o);
    }
    // macOS: the guest VM by default, else Seatbelt instead of bwrap,
    // Wayland and D-Bus.
    #[cfg(target_os = "macos")]
    return macroute::run_macos(env, o);
    #[cfg(not(target_os = "macos"))]
    run_linux(env, o)
}

#[cfg(not(target_os = "macos"))]
fn run_linux(env: &HostEnv, o: &AppOptions) -> Result<i32> {
    let profile = app_profile(env, &o.name)?;
    let roots = roots(env, &o.name, &o.roots)?;
    let user = Manifest::load(env, &env.home.join(".paddock-no-project"))?.merged;
    let cfg = user.apps.get(&o.name);
    let gpu = o.gpu || cfg.and_then(|a| a.gpu).unwrap_or(false);
    let worktree = o.worktree || cfg.and_then(|a| a.worktree).unwrap_or(false);
    if let Some(id) = profile.app.as_ref().and_then(|a| a.flatpak.clone()) {
        if o.backend.as_deref().is_some_and(|b| b != "bwrap") {
            bail!("{} is a flatpak app: flatpak runs its own bubblewrap sandbox, so it needs the bwrap backend", o.name);
        }
        crate::loft::push::notice(env, &roots[0]);
        return run_flatpak(env, &profile, &id, &roots, gpu, worktree, o);
    }
    let opts = Options {
        harness: o.name.clone(),
        project: Some(roots[0].clone()),
        backend: Some(o.backend.clone().unwrap_or_else(|| "bwrap".into())),
        network: o.network,
        gui: Some(GuiMode::App),
        gpu,
        no_yolo: true,
        inherit_env: o.inherit_env,
        auth: o.auth.clone(),
        args: app_args(&profile, &roots, &o.args),
        worktree,
        worktree_app: Some(o.name.clone()),
        ..Default::default()
    };
    let mut built = build::build(env, &opts)?;
    built.view = false;
    add_roots(env, &mut built.spec, &built.manifest, &roots[1..])?;
    crate::policy::enforce(env, &built.spec)?;
    let backend = crate::run::select_backend(env, &built)?;
    let wt = built.worktree.clone();
    let code = crate::run::run(env, built, backend.as_ref(), false, false)?.code;
    if let Some(w) = wt {
        report_worktree(env, &o.name, &w);
    }
    Ok(code)
}

#[cfg(not(target_os = "macos"))]
/// After the app exits: print what it left in its worktree, and send a
/// notification when no terminal shows the text (a launcher start).
fn report_worktree(env: &HostEnv, name: &str, w: &build::Worktree) {
    let (report, removed) = crate::worktree::finish(env, w);
    eprintln!("\n{report}");
    if removed || std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        return;
    }
    let (commits, changed) = build::worktree_counts(env, w);
    let note = worktree_note(name, w, commits, changed);
    // The Merge and Remove buttons. Without actions, only the text shows.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let shown = crate::broker::notify::ask(env, &note, move |key| {
        let _ = tx.send(key.to_string());
    });
    match shown {
        Ok(_handle) => {
            if let Ok(key) = rx.recv_timeout(std::time::Duration::from_secs(WORKTREE_ANSWER_SECS)) {
                desktop_notice(env, &worktree_action(env, &w.path, &key));
            }
        }
        Err(e) => log::debug!("no desktop notification: {e:#}"),
    }
}

#[cfg(not(target_os = "macos"))]
/// How long paddock waits for a button of the worktree notification.
const WORKTREE_ANSWER_SECS: u64 = 30 * 60;

/// Run the button that the user picked on the worktree notification, the
/// same as `paddock worktree merge` or `paddock worktree remove` (without
/// --force), and give back a notification with the result.
pub fn worktree_action(env: &HostEnv, path: &Path, key: &str) -> crate::broker::notify::Note {
    use crate::worktree;
    let all = worktree::list(env);
    let r = worktree::find(&all, &path.display().to_string()).and_then(|w| match key {
        "merge" => worktree::merge(env, &w),
        "remove" => worktree::remove(env, &w, false),
        _ => anyhow::bail!("unknown action {key}"),
    });
    match r {
        Ok(t) => crate::broker::notify::Note {
            summary: "paddock worktree".into(),
            body: t,
            ..Default::default()
        },
        Err(e) => crate::broker::notify::Note {
            summary: format!("paddock could not {key} the worktree"),
            body: format!("{e:#}"),
            ..Default::default()
        },
    }
}

/// The notification after an app worked in a worktree. The branch name and
/// the paths come from a repository that the sandbox wrote, so they go only
/// into the D-Bus message, never into the arguments of a host process.
pub fn worktree_note(
    name: &str,
    w: &build::Worktree,
    commits: usize,
    changed: Option<usize>,
) -> crate::broker::notify::Note {
    let changed = match changed {
        Some(n) => format!("{n}"),
        None => "an unknown number of".to_string(),
    };
    crate::broker::notify::Note {
        summary: format!("{name} worked on branch {}", w.branch),
        body: format!(
            "{commits} commits and {changed} uncommitted changes in {}. Review: git -C {} log {}",
            w.path.display(),
            w.repo.display(),
            w.branch
        ),
        actions: vec![
            ("merge".into(), "Merge".into()),
            ("remove".into(), "Remove".into()),
        ],
        ..Default::default()
    }
}

/// The notification when an app from a launcher entry cannot start.
pub fn start_error_note(name: &str, e: &anyhow::Error) -> crate::broker::notify::Note {
    crate::broker::notify::Note {
        summary: format!("paddock could not start {name}"),
        body: format!("{e:#}"),
        ..Default::default()
    }
}

/// Show a notification on the desktop through the session bus. The text can
/// come from a sandbox: no process gets it as arguments, where a leading "-"
/// would read as an option and every local user could read it in /proc.
/// Without a session bus, the terminal text is all there is.
pub fn desktop_notice(env: &HostEnv, n: &crate::broker::notify::Note) {
    if let Err(e) = crate::broker::notify::show(env, n) {
        log::debug!("no desktop notification: {e:#}");
    }
}

/// Flatpak apps run in flatpak's own sandbox, with every permission the app
/// asks for dropped (`--sandbox`, which also drops `flatpak override`
/// permissions) and only the workspace folders, the display, the GPU and the
/// portals added back. Never `--socket=session-bus`: that is the unfiltered
/// bus, through which `flatpak-spawn --host` runs commands on the host.
/// pasta gives it the same network path as bwrap. `fs` comes from
/// [`flatpak_workspace`].
pub fn flatpak_args(id: &str, command: Option<&str>, fs: &[String], gpu: bool) -> Vec<String> {
    let mut a: Vec<String> = [
        "run",
        "--sandbox",
        "--session-bus",
        "--share=network",
        "--socket=wayland",
        "--no-talk-name=org.freedesktop.Flatpak",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if gpu {
        a.push("--device=dri".into());
    }
    for n in BUS_TALK {
        a.push(format!("--talk-name={n}"));
    }
    a.extend(fs.iter().cloned());
    if let Some(c) = command {
        a.push(format!("--command={c}"));
    }
    a.push(id.to_string());
    a
}

/// The workspace folders of a flatpak app, as flatpak options.
#[derive(Debug, Default)]
pub struct FlatpakWorkspace {
    /// `--filesystem` and `--nofilesystem` options.
    pub args: Vec<String>,
    /// Secret files that stay visible: flatpak can hide a folder below a
    /// shared folder (it puts an empty tmpfs there), but not a file.
    pub exposed: Vec<PathBuf>,
}

/// A path as flatpak reads it in `--filesystem=PATH[:MODE]`. A `:` would
/// split the path, and `~` or a name such as `home` would mean something else.
fn flatpak_path(p: &std::path::Path) -> Result<String> {
    let s = p
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", p.display()))?;
    if !s.starts_with('/') || s.contains([':', '\\']) || s.chars().any(|c| c.is_control()) {
        bail!("{s}: flatpak cannot share a path with ':', '\\' or control characters; rename it");
    }
    Ok(s.to_string())
}

/// The same workspace protection as a bwrap app: each folder gets the git
/// and secrets detectors, and the policy checks the result (no home
/// directory, no protected path, git config, hooks and pointer files
/// read-only). Then each mount becomes a flatpak option. flatpak binds the
/// longest path last, so a read-only file inside a shared folder stays
/// read-only, and the git directory is a mount point of its own, which the
/// app cannot rename.
pub fn flatpak_workspace(
    env: &HostEnv,
    spec: &mut SandboxSpec,
    m: &Manifest,
    roots: &[PathBuf],
) -> Result<FlatpakWorkspace> {
    add_roots(env, spec, m, roots)?;
    crate::policy::enforce(env, spec)?;
    let mut ws = FlatpakWorkspace::default();
    for mnt in &spec.mounts {
        if mnt.is_bind() && mnt.src.as_ref() != Some(&mnt.dst) {
            bail!(
                "flatpak cannot bind {} at another path (from {})",
                mnt.dst.display(),
                mnt.origin
            );
        }
        let p = flatpak_path(&mnt.dst)?;
        match mnt.kind {
            MountKind::Rw => ws.args.push(format!("--filesystem={p}:rw")),
            MountKind::Ro => ws.args.push(format!("--filesystem={p}:ro")),
            MountKind::MaskDir => ws.args.push(format!("--nofilesystem={p}")),
            MountKind::MaskFile => ws.exposed.push(mnt.dst.clone()),
            MountKind::Overlay => {
                bail!("flatpak cannot mount layers at {} (from {})", p, mnt.origin)
            }
            MountKind::Tmpfs => bail!(
                "flatpak cannot put an empty folder at {} (from {})",
                p,
                mnt.origin
            ),
        }
    }
    Ok(ws)
}

/// `[apps.NAME] unprotected_workspaces = true`, from the user configuration
/// only: a project file cannot turn the protection off.
pub fn unprotected_workspaces(env: &HostEnv, name: &str) -> Result<bool> {
    let user = Manifest::load(env, &env.home.join(".paddock-no-project"))?.merged;
    Ok(user
        .apps
        .get(name)
        .is_some_and(|a| a.unprotected_workspaces))
}

/// Refuse a start that shows secret files to a flatpak app, unless unprotected: the warning then.
pub fn check_exposed(name: &str, exposed: &[PathBuf], unprotected: bool) -> Result<Option<String>> {
    if exposed.is_empty() {
        return Ok(None);
    }
    let list = exposed
        .iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    if !unprotected {
        bail!(
            "{name} is a flatpak app, and flatpak cannot hide single files, so it would read these secret files:\n{list}\n\
             Move them out of the workspace folders, or put them in a folder and hide the folder with [secrets] hide. \
             To start it anyway, with these files visible, set unprotected_workspaces = true in [apps.{name}] of ~/.config/paddock/config.toml."
        );
    }
    Ok(Some(format!(
        "unprotected_workspaces is on for {name}: it can read these secret files:\n{list}"
    )))
}

/// Host variables that flatpak itself needs on the host to start the app.
const FLATPAK_HOST_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "LANG",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
    "XDG_SESSION_TYPE",
];

/// The environment of `flatpak run`. flatpak passes almost all of it to the
/// app, so it gets the same rules as a bwrap sandbox: no host control
/// channels or sockets, no cloud credentials unless named in `[env] pass`,
/// nothing named in `[env] hide`, and with `--env clean` nothing else.
pub fn flatpak_env(
    env: &HostEnv,
    m: &Manifest,
    profile: &Profile,
    inherit: bool,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for k in FLATPAK_HOST_ENV {
        if let Some(v) = env.var(k) {
            out.insert(k.to_string(), v.to_string());
        }
    }
    for (k, v) in &env.vars {
        let named = m.env.pass.contains(k) || profile.state.env_pass.contains(k);
        if out.contains_key(k)
            || !crate::policy::inheritable_var(k, v)
            || m.env.hide.contains(k)
            || !(inherit || named)
            || (crate::policy::is_cloud_secret(k) && !named)
        {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    out
}

#[cfg(any(test, not(target_os = "macos")))]
/// The merged configuration for an app's first folder. The same rule as
/// every other sandbox: a project file that widens access (for example
/// `[secrets] reveal`) needs the user's approval first. The app can edit that
/// file, so it is never trusted on its own.
fn approved_manifest(env: &HostEnv, root: &std::path::Path) -> Result<Manifest> {
    let loaded = Manifest::load(env, root)?;
    crate::manifest::require_approval(env, &loaded, false)?;
    Ok(loaded.merged)
}

/// A worktree of the first workspace folder for a flatpak app, and the
/// workspace folders with the worktree in place of that folder.
pub fn flatpak_worktree(
    env: &HostEnv,
    name: &str,
    roots: &[PathBuf],
) -> Result<(build::Worktree, Vec<PathBuf>)> {
    let w = build::create_worktree(env, &roots[0], Some(name))?;
    let mut out = roots.to_vec();
    out[0] = w.path.clone();
    Ok((w, out))
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn run_flatpak(
    env: &HostEnv,
    profile: &Profile,
    id: &str,
    roots: &[PathBuf],
    gpu: bool,
    worktree: bool,
    o: &AppOptions,
) -> Result<i32> {
    let flatpak = env.which("flatpak").context("this app needs flatpak")?;
    let net = o.network.unwrap_or(Network::Egress);
    if !matches!(net, Network::Egress | Network::None) {
        bail!("flatpak apps support network modes egress and none, not {net:?}");
    }
    // The settings belong to the repository, as for other apps.
    let m = approved_manifest(env, &roots[0])?;
    if !worktree {
        return start_flatpak(env, profile, id, roots, &m, flatpak, net, gpu, o);
    }
    let (w, roots) = flatpak_worktree(env, &o.name, roots)?;
    let r = start_flatpak(env, profile, id, &roots, &m, flatpak, net, gpu, o);
    report_worktree(env, &o.name, &w);
    r
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn start_flatpak(
    env: &HostEnv,
    profile: &Profile,
    id: &str,
    roots: &[PathBuf],
    m: &Manifest,
    flatpak: PathBuf,
    net: Network,
    gpu: bool,
    o: &AppOptions,
) -> Result<i32> {
    let mut spec = SandboxSpec::new(
        crate::state::new_id(&profile.name, &roots[0]),
        &profile.name,
        &roots[0],
        env.home.clone(),
        &env.home,
        env.uid,
        env.gid,
        &env.user,
    );
    let ws = flatpak_workspace(env, &mut spec, m, roots)?;
    let warning = check_exposed(
        &profile.name,
        &ws.exposed,
        unprotected_workspaces(env, &profile.name)?,
    )?;
    if let Some(w) = &warning {
        eprintln!("paddock: warning: {w}");
    }
    let mut cmd = flatpak_args(
        id,
        profile.app.as_ref().and_then(|a| a.command.as_deref()),
        &ws.args,
        gpu,
    );
    cmd.extend(profile.launch.args.iter().cloned());
    cmd.extend(app_args(profile, roots, &o.args));
    let (program, args) = match net {
        Network::Egress => {
            let (pasta, mut pa) = crate::backend::bwrap::pasta_prefix(env, &spec)?;
            pa.push(flatpak.display().to_string());
            pa.extend(cmd);
            (pasta, pa)
        }
        _ => {
            cmd.retain(|a| a != "--share=network");
            (flatpak, cmd)
        }
    };
    let inherit = o.inherit_env.or(m.env.inherit).unwrap_or(true);
    let mut child = Command::new(&program)
        .args(&args)
        .env_clear()
        .envs(flatpak_env(env, m, profile, inherit))
        .spawn()
        .with_context(|| format!("cannot start {}", program.display()))?;
    // Listed with the other sandboxes, so the panel shows it and its warning.
    let running = crate::state::Running {
        id: spec.id.clone(),
        harness: profile.name.clone(),
        project: roots[0].clone(),
        backend: "flatpak".into(),
        network: format!("{net:?}").to_lowercase(),
        gui: "app".into(),
        pid: child.id(),
        started: crate::state::now(),
        auth: String::new(),
        approvals: crate::approvals::shown(None).into(),
        warnings: if warning.is_some() {
            vec!["secret files visible (unprotected_workspaces)".to_string()]
        } else {
            Vec::new()
        },
        host_pid: std::process::id(),
        ..Default::default()
    };
    if let Err(e) = running.register(env) {
        log::warn!("cannot list the app with the running sandboxes: {e:#}");
    }
    let status = child.wait();
    let _ = std::fs::remove_dir_all(env.paddock_run().join(&spec.id));
    crate::sockpath::remove_run_sockets(&env.paddock_run(), env.uid, &spec.id);
    Ok(status?.code().unwrap_or(1))
}

pub fn desktop_file(env: &HostEnv, name: &str) -> PathBuf {
    env.data_dir
        .join("applications")
        .join(format!("paddock-{name}.desktop"))
}

/// A launcher entry that starts the app in a sandbox.
pub fn install(env: &HostEnv, name: &str) -> Result<PathBuf> {
    let p = app_profile(env, name)?;
    let app = p.app.clone().unwrap_or_default();
    // Never a build: see launchers::launcher_exe.
    let me = crate::launchers::launcher_exe(env)?;
    let display = if app.display_name.is_empty() {
        name.to_string()
    } else {
        app.display_name.clone()
    };
    let text = format!(
        "[Desktop Entry]\nType=Application\nName={display} (paddock)\nComment={display} in a paddock sandbox\nExec={} app run {name}\nIcon={}\nTerminal=false\nCategories=Development;\n",
        me.display(),
        app.icon.unwrap_or_else(|| "application-x-executable".into())
    );
    let f = desktop_file(env, name);
    std::fs::create_dir_all(f.parent().unwrap())?;
    std::fs::write(&f, text)?;
    Ok(f)
}

#[path = "app/links.rs"]
mod links;
pub use links::{claim_links, inbound_serve, link_desktop_file, open, release_links, valid_link};

/// Login callbacks to http://127.0.0.1:PORT. The app listens in the
/// sandbox's own network namespace, which the host browser cannot reach. When
/// `[apps.NAME] login_ports = true`, the helper in the sandbox reports the
/// ports that the app listens on, and paddock on the host listens on the same
/// ports on 127.0.0.1 only, and passes each connection through a Unix socket
/// to the helper, which connects to the port inside. A port is open on the
/// host only while the app listens on it.
#[path = "app/ports.rs"]
pub mod ports;

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
