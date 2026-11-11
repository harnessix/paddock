#[path = "app_add.rs"]
mod app_add;
#[path = "app_list.rs"]
mod app_list;
use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::spec::Network;
use paddock::state;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum AppCmd {
    /// Start an app. Its windows appear on your desktop; it sees only the workspace folders.
    Run {
        name: String,
        /// Workspace folders, read-write. Default: [apps.NAME] roots in the user configuration.
        roots: Vec<PathBuf>,
        /// A workspace folder that the macOS guest gets read-only (repeatable).
        #[arg(long, value_name = "PATH")]
        ro: Vec<PathBuf>,
        /// Bind GPU render nodes (faster rendering).
        #[arg(long)]
        gpu: bool,
        /// none, egress (default), allowlist, api, or ask (the default of
        /// paddock's macOS guest, where the first connection to each new
        /// host asks you).
        #[arg(long, short)]
        network: Option<String>,
        /// inherit (default) or clean.
        #[arg(long, value_parser = ["inherit", "clean"])]
        env: Option<String>,
        /// shared (default), isolated or isolated:NAME. Most desktop apps keep
        /// their login in the host keyring, which a sandbox cannot use, so they
        /// log in again inside the sandbox in any mode.
        #[arg(long)]
        auth: Option<String>,
        /// bwrap (default), docker, podman or srt. srt needs --network allowlist or none.
        /// On macOS: macvm runs the app in paddock's own macOS guest VM.
        #[arg(long, short)]
        backend: Option<String>,
        /// Open the first workspace folder in a new git worktree on its own
        /// branch. Default: [apps.NAME] worktree in the user configuration.
        #[arg(long)]
        worktree: bool,
        /// macOS guest only: the session honors the bench ops.
        #[arg(long, hide = true)]
        bench: bool,
        #[command(flatten)]
        inst: super::app_guest::Instance,
        /// macOS guest only: a new instance of the app, with the next free
        /// number: its own disks, data, keychain and firewall rules.
        #[arg(long, conflicts_with = "instance")]
        new: bool,
    },
    /// Add a launcher entry (Super+Space) that starts the app in a sandbox.
    /// On macOS: ~/Applications/<Name> (paddock).app, with the app's icon.
    Install {
        /// Leave it out with --drop.
        name: Option<String>,
        /// Workspace folders to save in the user configuration.
        roots: Vec<PathBuf>,
        /// Do not make paddock the handler of the app's links.
        #[arg(long)]
        no_links: bool,
        /// macOS only: make ~/Applications/Paddock Apps.app, the drag
        /// target. An app bundle dropped on it becomes a profile and runs
        /// in paddock's macOS guest.
        #[arg(long)]
        drop: bool,
    },
    /// Remove the launcher entry, and give the app's links back to their old handler.
    Uninstall {
        /// Leave it out with --drop.
        name: Option<String>,
        /// macOS only: remove the drag target.
        #[arg(long)]
        drop: bool,
    },
    /// Give a link (for example claude://...) to the running app sandbox, or start the app with it.
    Open { name: String, link: String },
    /// Make an app bundle of this Mac an app profile of the macOS guest.
    /// --dry-run --json is the precheck: the App Store, the space, the golden.
    Add(app_add::AddArgs),
    /// Remove an app profile of the macOS guest: its copy, its saved state
    /// and its launcher; the data disk goes to the trash unless --delete-data.
    Remove(app_add::RemoveArgs),
    /// List the app profiles, whether each is installed, and its folders.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Open the downloads folder of the app's macOS guest profile: every
    /// download of the app in the guest lands there.
    Files {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
    },
    /// End a session in the macOS guest VM: the app quits, the state is
    /// saved, the VM stops.
    Stop {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
        /// Seconds to wait for the end.
        #[arg(long, default_value_t = 120)]
        wait: u64,
    },
    /// One screen about a profile of the macOS guest: the session, the
    /// golden, the app, the guest's health, the sizes, the network counts,
    /// the shares and the logs, and what the next start will do.
    Status {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
        #[arg(long)]
        json: bool,
    },
    /// The firewall rules of an app in paddock's macOS guest (network
    /// mode ask): list, add and remove. Without a subcommand: list.
    Rules {
        name: String,
        #[command(subcommand)]
        cmd: Option<super::rules::RulesCmd>,
    },
    /// Reset a profile of the macOS guest. Without a flag: the saved
    /// state, the app clone, the drops and the logs go; the disk pair
    /// with the login and the files stays. --data removes the pair after
    /// a backup of it. --all moves the whole profile to the trash for 7
    /// days.
    Reset {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
        /// Also the disk pair (system and data disk): the login and the
        /// files of the guest go, after a backup in backups/.
        #[arg(long, conflicts_with = "all")]
        data: bool,
        /// The whole profile folder and its firewall rules, into the
        /// trash for 7 days.
        #[arg(long)]
        all: bool,
        /// Reset without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Back the disk pair of a profile up: a clone of the system disk
    /// and the data disk into backups/<time>/ (shared blocks, seconds),
    /// or into a folder of your choice. The session must be stopped.
    Backup {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
        /// A folder for the backup, which must not exist.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Stop the session first.
        #[arg(long)]
        stop: bool,
    },
    /// Put a backup in place of the disk pair of a profile. The pair of
    /// before goes to backups/pre-restore-<time>/. The next start is a
    /// cold boot.
    Restore {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
        /// A backup folder, its name, or a part of its name (the time).
        which: String,
    },
    /// List the backups of a profile with their sizes.
    Backups {
        name: String,
        #[command(flatten)]
        inst: super::app_guest::Instance,
    },
    #[command(flatten)]
    Grants(super::grants::GrantsCmd),
    /// Print a log of a profile of the macOS guest: the connection log
    /// (default), the event log, the shim log, the guest agent log or the
    /// guest console.
    Log {
        name: String,
        /// The event log: one line per state change of the session.
        #[arg(long, conflicts_with_all = ["shim", "guest", "console"])]
        events: bool,
        /// The log of the session helper on the host.
        #[arg(long, conflicts_with_all = ["guest", "console"])]
        shim: bool,
        /// The log of the agent in the guest, fetched at each quit.
        #[arg(long, conflicts_with = "console")]
        guest: bool,
        /// The serial console of the guest.
        #[arg(long)]
        console: bool,
        /// Keep printing new lines until the session ends.
        #[arg(long, short)]
        follow: bool,
    },
}

pub(crate) const APP_HELP: &str = "App mode runs a desktop agent app in a sandbox. Its windows are normal
windows on your desktop, but through a restricted Wayland socket: the app
cannot capture the screen, read the clipboard in the background, type into
other windows, or list them. Its session bus is private, inside the sandbox,
with a private keyring for its logins. Links that it opens go to your browser
after your approval, and its notifications appear on your desktop. With
`[apps.NAME] bus = \"host-filtered\"` in ~/.config/paddock/config.toml, its
bus is the host bus, filtered to the desktop portals, notifications and the
tray, without a keyring. It sees only its workspace folders, with the same git hook, secrets and network
rules as `paddock <harness>`. The host passthrough (paddock-host) works too.
Flatpak apps differ in some of this; see below.

Examples:
  paddock app run chatgpt-desktop ~/src/api ~/src/web
  paddock app install claude-desktop ~/src/api
  paddock app list
  paddock app status chatgpt-desktop      (macOS guest: one screen about the session)
  paddock app log chatgpt-desktop --events --follow

Electron apps run with --no-sandbox: their own sandbox needs nested user
namespaces, which paddock blocks, so paddock is the boundary. Flatpak apps
(profiles with [app] flatpak = \"ID\") run with flatpak run --sandbox inside
the same network isolation (egress or none), with flatpak's own Wayland
socket and filtered host bus, without the host passthrough or worktrees.
Their folders get the same path policy, read-only git config, hooks and
pointer files, and host environment filter as other apps. flatpak can hide
a folder but not a single file, so paddock refuses to start a flatpak app
when the secrets detector finds a file such as .env in its folders, and
names the files. `[apps.NAME] unprotected_workspaces = true` in
~/.config/paddock/config.toml starts it anyway, with a warning on every
start. To add folders later, change [apps.NAME] roots and start the app
again.";

pub(crate) fn app_cmd(env: &HostEnv, cmd: AppCmd) -> Result<i32> {
    use paddock::app;
    match cmd {
        AppCmd::Run {
            name,
            roots,
            ro,
            gpu,
            network,
            env: env_mode,
            auth,
            backend,
            worktree,
            bench,
            inst,
            new,
        } => {
            let mut roots = roots;
            let mut worktree = worktree;
            // A launcher entry has no terminal to show "no workspace folders".
            // Ask for a folder on the desktop instead.
            if roots.is_empty()
                && app::roots(env, &name, &[]).is_err()
                && !std::io::IsTerminal::is_terminal(&std::io::stderr())
            {
                match pick_folder_gui(env, &name) {
                    Some((dir, new_worktree)) => {
                        roots.push(dir);
                        worktree |= new_worktree;
                    }
                    None if env.which("zenity").is_some() => return Ok(0),
                    None => {}
                }
            }
            let network = app_network(network.as_deref(), backend.as_deref())?;
            let o = app::AppOptions {
                name: name.clone(),
                roots,
                ro,
                gpu,
                network,
                inherit_env: env_mode.as_deref().map(|e| e == "inherit"),
                auth,
                args: vec![],
                backend,
                worktree,
                guest: paddock::macvm::host_record::GuestOptions {
                    bench,
                    instance: inst.instance,
                    new_instance: new,
                    resumed_by: None,
                },
            };
            match app::run(env, &o) {
                Ok(code) => Ok(code),
                Err(e) => {
                    // A launcher entry has no terminal: show the error on the desktop too.
                    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                        app::desktop_notice(env, &app::start_error_note(&name, &e));
                    }
                    Err(e)
                }
            }
        }
        AppCmd::Install {
            name,
            roots,
            no_links,
            drop,
        } => {
            if drop {
                return install_drop_target(env, name.as_deref());
            }
            let name = name.ok_or_else(|| {
                anyhow::anyhow!("name the app: paddock app install chatgpt-desktop ~/src/project")
            })?;
            if cfg!(target_os = "macos") {
                return install_mac_launcher(env, &name, &roots);
            }
            app::app_profile(env, &name)?;
            if !roots.is_empty() {
                let rs = app::roots(env, &name, &roots)?;
                state::set_app_roots(env, &name, &rs)?;
                println!("Saved the workspace folders in [apps.{name}] of the user configuration.");
            } else {
                app::roots(env, &name, &[]).map_err(|_| {
                    anyhow::anyhow!(
                        "name the workspace folders: paddock app install {name} ~/src/project"
                    )
                })?;
            }
            let f = app::install(env, &name)?;
            println!("Wrote {}. Start it from the app launcher.", f.display());
            if !no_links {
                for (scheme, old) in app::claim_links(env, &name)? {
                    println!("{scheme}:// links now open in the {name} sandbox (before: {}). paddock app uninstall {name} gives them back.", if old.is_empty() { "no handler" } else { &old });
                }
            }
            Ok(0)
        }
        AppCmd::Open { name, link } => app::open(env, &name, &link),
        AppCmd::Uninstall { name, drop } => {
            if drop {
                let gone = paddock::launchers::macapp::uninstall_drop(env)?;
                let p = paddock::launchers::macapp::drop_path(env);
                println!(
                    "{}",
                    if gone {
                        paddock::macvm::text::launcher_removed(&p)
                    } else {
                        paddock::macvm::text::launcher_absent(&p)
                    }
                );
                return Ok(if gone { 0 } else { 1 });
            }
            let name = name.ok_or_else(|| {
                anyhow::anyhow!("name the app: paddock app uninstall chatgpt-desktop")
            })?;
            if cfg!(target_os = "macos") {
                return uninstall_mac_launcher(env, &name);
            }
            for (scheme, old) in app::release_links(env, &name)? {
                println!(
                    "{scheme}:// links open with {} again",
                    if old.is_empty() { "no handler" } else { &old }
                );
            }
            let f = app::desktop_file(env, &name);
            match std::fs::remove_file(&f) {
                Ok(()) => println!("Removed {}", f.display()),
                Err(_) => println!("{} does not exist", f.display()),
            }
            Ok(0)
        }
        AppCmd::Files { name, inst } => paddock::macvm::host::files(env, &name, inst.n()),
        AppCmd::Stop { name, inst, wait } => {
            let wait = std::time::Duration::from_secs(wait);
            let shown = paddock::macvm::instance::label(&name, inst.n());
            if paddock::macvm::host::stop(env, &name, inst.n(), wait)? {
                println!("{shown} stopped");
                Ok(0)
            } else {
                println!("{shown} is not running in the macOS guest");
                Ok(1)
            }
        }
        AppCmd::Status { name, inst, json } => {
            paddock::macvm::status::cmd(env, &name, inst.n(), json)
        }
        AppCmd::Reset {
            name,
            inst,
            data,
            all,
            yes,
        } => super::app_guest::reset_cmd(env, &name, inst.n(), data, all, yes),
        AppCmd::Backup {
            name,
            inst,
            out,
            stop,
        } => super::app_guest::backup_cmd(env, &name, inst.n(), out.as_deref(), stop),
        AppCmd::Restore { name, inst, which } => {
            super::app_guest::restore_cmd(env, &name, inst.n(), &which)
        }
        AppCmd::Backups { name, inst } => super::app_guest::backups_cmd(env, &name, inst.n()),
        AppCmd::Grants(cmd) => super::grants::run(env, cmd),
        AppCmd::Rules { name, cmd } => super::rules::rules(env, &name, cmd),
        AppCmd::Log {
            name,
            events,
            shim,
            guest,
            console,
            follow,
        } => {
            use paddock::macvm::logs::Which;
            let which = if events {
                Some(Which::Events)
            } else if shim {
                Some(Which::Shim)
            } else if guest {
                Some(Which::Agent)
            } else if console {
                Some(Which::Console)
            } else {
                None
            };
            paddock::macvm::logs::cmd(env, &name, which, follow)
        }
        AppCmd::List { json } => app_list::list(env, json),
        AppCmd::Add(a) => app_add::add(env, a),
        AppCmd::Remove(a) => app_add::remove(env, a),
    }
}

/// The network mode of `paddock app run`: `ask` is the mode of paddock's
/// macOS guest, the default there, and refused on every other route
/// with one line. The other words are the modes of every route.
fn app_network(word: Option<&str>, backend: Option<&str>) -> Result<Option<Network>> {
    match word {
        None => Ok(None),
        Some("ask") => {
            let guest = cfg!(target_os = "macos")
                && matches!(
                    backend,
                    None | Some("auto") | Some(paddock::macvm::host::BACKEND)
                );
            if !guest {
                bail!(paddock::macvm::text::ask_mode_elsewhere());
            }
            Ok(None)
        }
        Some(w) => Ok(Some(w.parse()?)),
    }
}

/// `paddock app install --drop` (macOS): the drag target in
/// ~/Applications. A name next to --drop is a mistake worth saying.
fn install_drop_target(env: &HostEnv, name: Option<&str>) -> Result<i32> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!(paddock::macvm::text::install_macos_only());
    }
    if let Some(name) = name {
        anyhow::bail!("--drop makes the drag target for every app, so it takes no app name (you named {name})");
    }
    let path = paddock::launchers::macapp::install_drop(env)?;
    println!("{}", paddock::macvm::text::drop_target_written(&path));
    Ok(0)
}

/// `paddock app install NAME` (macOS): the launcher bundle with the app's
/// own name and icon, which runs the app in paddock's macOS guest.
fn install_mac_launcher(env: &HostEnv, name: &str, roots: &[PathBuf]) -> Result<i32> {
    use paddock::app;
    let profile = app::app_profile(env, name)?;
    if !roots.is_empty() {
        let rs = app::roots(env, name, roots)?;
        state::set_app_roots(env, name, &rs)?;
        println!("Saved the workspace folders in [apps.{name}] of the user configuration.");
    }
    let label = app::app_label(&profile);
    let user = paddock::manifest::Manifest::load(env, &env.home.join(".paddock-no-project"))
        .map(|m| m.merged)
        .ok();
    let cfg = user.as_ref().and_then(|u| u.apps.get(name));
    let bundle = paddock::macvm::host_app::read_installed(env, name, Some(&profile), cfg)?;
    let path =
        paddock::launchers::macapp::install(env, name, &bundle.display_name, bundle.icon.clone())?;
    println!("{}", paddock::macvm::text::launcher_written(&label, &path));
    Ok(0)
}

/// `paddock app uninstall NAME` (macOS): the launcher bundle.
fn uninstall_mac_launcher(env: &HostEnv, name: &str) -> Result<i32> {
    let profile = paddock::app::app_profile(env, name)?;
    let label = paddock::app::app_label(&profile);
    let path = paddock::launchers::macapp::launcher_path(env, &label);
    let gone = paddock::launchers::macapp::uninstall(env, &label)?;
    println!(
        "{}",
        if gone {
            paddock::macvm::text::launcher_removed(&path)
        } else {
            paddock::macvm::text::launcher_absent(&path)
        }
    );
    Ok(if gone { 0 } else { 1 })
}

/// Ask for a workspace folder with a desktop dialog: the recent projects, or
/// any folder. For a git repository, also ask whether to open it in a new
/// worktree; that is the default answer. None when the user cancels or
/// zenity is not installed.
fn pick_folder_gui(env: &HostEnv, name: &str) -> Option<(PathBuf, bool)> {
    let dir = pick_dir_gui(env, name)?;
    let is_repo = paddock::hostgit::Layout::discover(&dir)
        .ok()
        .flatten()
        .is_some_and(|l| {
            l.try_query(&["rev-parse", "--verify", "-q", "HEAD"])
                .is_some()
        });
    if !is_repo {
        return Some((dir, false));
    }
    let zenity = env.which("zenity")?;
    const THIS: &str = "Open this folder";
    let out = std::process::Command::new(zenity)
        .args(["--question", "--title", "Open in a new worktree?", "--width=480"])
        .arg(format!(
            "--text=Open {} in a new git worktree on its own branch?\n\nThe app then cannot switch branches or overwrite changes in your checkout. When it exits, paddock lists its commits and changes.",
            dir.display()
        ))
        .args(["--ok-label=New worktree", "--cancel-label=Cancel"])
        .arg(format!("--extra-button={THIS}"))
        .output()
        .ok()?;
    if out.status.success() {
        return Some((dir, true));
    }
    (String::from_utf8_lossy(&out.stdout).trim() == THIS).then_some((dir, false))
}

fn pick_dir_gui(env: &HostEnv, name: &str) -> Option<PathBuf> {
    let zenity = env.which("zenity")?;
    let label = paddock::app::app_profile(env, name)
        .ok()
        .and_then(|p| p.app.map(|a| a.display_name))
        .unwrap_or_else(|| name.to_string());
    let title = format!("Open {label} in paddock");
    const OTHER: &str = "Choose another folder…";
    // Recent projects, shown with ~. Hidden folders (caches, test runs) stay out.
    let mut projects: Vec<String> = Vec::new();
    for r in state::recent(env) {
        let Ok(rel) = r.project.strip_prefix(&env.home) else {
            continue;
        };
        if rel
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let p = format!("~/{}", rel.display());
        if !projects.contains(&p) {
            projects.push(p);
        }
    }
    let chosen = if projects.is_empty() {
        OTHER.to_string()
    } else {
        let out = std::process::Command::new(&zenity)
            .args(["--list", "--title", &title, "--width=640", "--height=420"])
            .args(["--text", "The app sees only this folder."])
            .args(["--column", "Project folder"])
            .args(&projects)
            .arg(OTHER)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    if chosen.is_empty() {
        return None;
    }
    if chosen != OTHER {
        return Some(env.expand(&chosen, &env.home));
    }
    let out = std::process::Command::new(&zenity)
        .args(["--file-selection", "--directory", "--title", &title])
        .arg(format!("--filename={}/", env.home.display()))
        .output()
        .ok()?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !dir.is_empty()).then(|| PathBuf::from(dir))
}

pub(crate) fn run(env: &HostEnv, cmd: AppCmd) -> Result<i32> {
    app_cmd(env, cmd)
}

pub(crate) fn inbound() -> Result<i32> {
    paddock::app::inbound_serve()?;
    Ok(0)
}

pub(crate) fn portal() -> Result<i32> {
    paddock::portal::serve()?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_is_the_mode_of_the_macos_guest_only() {
        assert_eq!(app_network(None, None).unwrap(), None);
        assert_eq!(
            app_network(Some("egress"), None).unwrap(),
            Some(Network::Egress)
        );
        assert!(app_network(Some("host-ish"), None).is_err());
        let e = app_network(Some("ask"), Some("seatbelt"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("macOS guest"), "{e}");
        let on_guest = app_network(Some("ask"), Some("macvm"));
        if cfg!(target_os = "macos") {
            assert_eq!(on_guest.unwrap(), None, "the default of the guest");
        } else {
            assert!(on_guest.is_err());
        }
    }
}
