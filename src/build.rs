//! Assemble a `SandboxSpec` from the harness profile, the project detectors,
//! the manifest and the command line, then run the policy engine on it.

mod cleanup;
#[cfg(test)]
mod privacy_tests;
mod worktree;
use cleanup::{PendingShadow, PendingWorktree};
pub use worktree::{
    create_worktree, discard_worktree, worktree_counts, worktree_report, worktree_status, Worktree,
};

use crate::detect::{self, git_toplevel, Context};
use crate::env::HostEnv;
use crate::gui;
use crate::manifest::{self, Manifest};
use crate::policy;
use crate::profile::{Harness, HarnessFactory, Profile};
use crate::spec::{GuiMode, Mount, Network, SandboxSpec};
use crate::state;
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub harness: String,
    pub project: Option<PathBuf>,
    pub backend: Option<String>,
    pub network: Option<Network>,
    pub gui: Option<GuiMode>,
    pub gpu: bool,
    /// paddock adds no approval flags or settings of its own: the command
    /// decides (`paddock wrap`, desktop apps). The approvals mode does not
    /// apply then.
    pub no_yolo: bool,
    /// `--manual` (or `--no-yolo`) and `--auto` of the command line. None:
    /// the project, the agent and the user configuration decide
    /// (approvals.rs).
    pub approvals: Option<crate::approvals::Approvals>,
    /// Pass the host environment. None means the manifest decides (default true).
    pub inherit_env: Option<bool>,
    /// shared, isolated, or isolated:NAME. None means the manifest decides.
    pub auth: Option<String>,
    pub shell: bool,
    pub assume_yes: bool,
    pub args: Vec<String>,
    /// Work in a new git worktree on its own branch.
    pub worktree: bool,
    /// Name the worktree branch after this app.
    pub worktree_app: Option<String>,
    /// Extra domains for network mode `allowlist`.
    pub allow_domains: Vec<String>,
    /// App mode: the session bus, instead of `[apps.NAME] bus`.
    pub app_bus: Option<crate::manifest::AppBus>,
    /// A sandbox that another sandbox asked for (`paddock-host spawn`). It
    /// uses the parent's settings snapshot and runs the one-shot command.
    pub child: Option<crate::spawn::ChildPlan>,
    /// `paddock wrap`: the command runs as given, with its `env` prefix and
    /// shell form, and the login comes from its account folder.
    pub transparent: Option<crate::wrap::Transparent>,
    /// Join this pasture of the project (`--pasture NAME`). None: the
    /// sandbox is a pasture of its own.
    pub pasture: Option<String>,
    /// The workbench of the pasture (pasture/workbench.rs): its socket
    /// folder is writable.
    pub workbench: bool,
    /// `paddock run --detach`: the sandbox id that the caller chose. The
    /// harness gets its interactive command with the `[detach]` args.
    pub detach: Option<String>,
    /// `--profile default`: the harness runs on a private snapshot of its
    /// host data (shadow.rs). Only the command line sets it.
    pub profile_default: bool,
    /// `--device NAME` (repeatable): the devices of the user configuration
    /// that this run gets, whatever their `grant` says. A spawned child
    /// gets them from its parent, within the parent's ceiling.
    pub devices: Vec<String>,
}

/// Today's date in UTC as YYYYMMDD, for branch names.
fn today() -> String {
    ymd(state::now())
}

pub(crate) fn ymd(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}

pub struct Built {
    pub horse: bool,
    /// A detached run (`paddock run --detach`): `paddock-host done` ends it.
    pub detach: bool,
    /// For a child sandbox: where it came from. Its broker takes the depth
    /// and the host modes from here.
    pub spawn: Option<crate::spawn::ChildInfo>,
    /// Bytes for the harness's standard input (a spawn prompt).
    pub stdin: Option<Vec<u8>>,
    /// The repository or folder that the settings and the home belong to.
    /// With a worktree, the repository; otherwise the project.
    pub home_key: PathBuf,
    /// Whether the host environment was passed.
    pub inherit_env: bool,
    /// Who answers host passthrough requests. None means the user, through
    /// the panel, a notification or `paddock approvals`.
    pub approver: Option<std::sync::Arc<dyn crate::broker::Approver>>,
    /// The parent for the `parent` scope of paddock kv.
    /// None for a sandbox that the user started; spawn supplies it.
    pub parent: Option<String>,
    /// For a spawned sandbox: the devices of its parent. It gets no other.
    pub device_ceiling: Option<Vec<String>>,
    /// `--device NAME` of the run or the spawn (Options::devices).
    pub devices: Vec<String>,
    /// The run shares session history with the host (`paddock wrap` starts).
    pub shared_history: bool,
    /// Serve the echo device for `paddock doctor --escape`.
    pub test_devices: bool,
    /// Show the run in a live view when it has no terminal (view.rs).
    /// Off for the escape probe and app windows.
    pub view: bool,
    pub worktree: Option<Worktree>,
    /// The pasture it joined; None: a pasture of its own.
    pub pasture: Option<String>,
    /// The one-shot command that answers the questions of other members
    /// (pasture/bridge.rs). None outside a pasture, or without `[oneshot]`.
    pub bridge: Option<crate::pasture::bridge::Command>,
    pub auth: crate::state::AuthMode,
    pub spec: SandboxSpec,
    pub profile: Profile,
    pub harness_exe: PathBuf,
    pub manifest: Manifest,
    pub backend: String,
    /// Who approves the agent's steps. None: the command decides
    /// (`Options::no_yolo`), and paddock adds nothing.
    pub approvals: Option<crate::approvals::Approvals>,
    /// Where the mode comes from, for `paddock explain`.
    pub approvals_origin: String,
}

impl Built {
    /// The user approves the agent's steps by hand (approvals.rs).
    pub fn manual(&self) -> bool {
        self.approvals == Some(crate::approvals::Approvals::Manual)
    }
}

/// The host folders that a sandbox can write, as real paths: the project,
/// every read-write mount so far, and all sandbox homes and worktrees (a
/// sandbox of another project can write those).
pub fn sandbox_writable(env: &HostEnv, spec: &SandboxSpec) -> Vec<PathBuf> {
    let mut v = vec![
        spec.project.clone(),
        spec.home_src.clone(),
        env.paddock_data().join("homes"),
        env.paddock_data().join("worktrees"),
    ];
    v.extend(
        spec.mounts
            .iter()
            .filter(|m| m.kind == crate::spec::MountKind::Rw)
            .filter_map(|m| m.src.clone()),
    );
    v.into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect()
}

/// The harness install mounts come from the executable and the script
/// headers it leads to (`profile::harness_closure`). A file in a folder the
/// sandbox can write could name any host path in its header, and paddock
/// would mount that path. So the harness must not come from such a folder,
/// for example through a host PATH entry inside the project.
pub fn check_harness_install(harness: &Harness, writable: &[PathBuf]) -> Result<()> {
    let all = std::iter::once(&harness.exe)
        .chain(&harness.install_mounts)
        .chain(&harness.path_dirs);
    for p in all {
        if let Some(w) = writable.iter().find(|w| p.starts_with(w)) {
            bail!(
                "the {} harness resolves to {}, inside {}, which the sandbox can write. \
                 paddock does not mount host paths that such a file names. \
                 Install the harness outside the project and the sandbox homes.",
                harness.profile.name,
                p.display(),
                w.display()
            );
        }
    }
    Ok(())
}

pub fn resolve_project(path: Option<&Path>) -> Result<PathBuf> {
    let raw = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let dir = std::fs::canonicalize(&raw)
        .with_context(|| format!("project directory {} does not exist", raw.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(git_toplevel(&dir).unwrap_or(dir))
}

/// Build a spec for a located harness.
pub fn build(env: &HostEnv, opts: &Options) -> Result<Built> {
    if let Some(name) = &opts.pasture {
        if opts.child.is_some() {
            bail!("a sandbox that another one starts cannot join a pasture");
        }
        crate::pasture::admit(env, name, &opts.harness)?;
    }
    let factory = HarnessFactory::new(env)?;
    let harness = factory.create(&opts.harness)?;
    build_with(env, opts, harness)
}

pub fn build_with(env: &HostEnv, opts: &Options, harness: Harness) -> Result<Built> {
    let memory = crate::memory::limit::Config::load(env)?;
    let project = resolve_project(opts.project.as_deref())?;
    // The project path is resolved, so compare it with the resolved home
    // too: `$HOME` may name a symlink.
    if crate::policy::homes(env)
        .iter()
        .any(|h| project == *h || h.starts_with(&project))
    {
        bail!(
            "refusing to use {} as the project: it is the home directory or contains it",
            project.display()
        );
    }
    // A child uses the settings that its parent had when it started. The
    // project file may have changed since: the parent can write it.
    let m = match &opts.child {
        Some(c) => c.manifest.clone(),
        None => {
            let mut loaded = Manifest::load(env, &project)?;
            // `--network` wins over the mode of the project file.
            loaded.base.cli_network = opts.network.is_some();
            manifest::require_approval(env, &loaded, opts.assume_yes)?;
            loaded.merged
        }
    };
    crate::devices::check_requested(env, &opts.devices)?;
    // Who approves the agent's steps (approvals.rs). A spawned child runs
    // one task without a terminal, so nobody could answer it by hand.
    let approvals = if opts.no_yolo {
        crate::approvals::Choice {
            mode: crate::approvals::Approvals::Manual,
            origin: "the command: paddock adds no approval flags".into(),
        }
    } else if opts.child.is_some() {
        crate::approvals::Choice {
            mode: crate::approvals::Approvals::Auto,
            origin: "spawned sandbox: a one-shot task has nobody to answer it".into(),
        }
    } else {
        crate::approvals::of(&m, &opts.harness, opts.approvals)
    };
    let approvals_mode = (!opts.no_yolo).then_some(approvals.mode);
    // In manual mode paddock writes none of the settings that skip the
    // agent's own questions (bypass warning, project trust).
    let manual = approvals_mode == Some(crate::approvals::Approvals::Manual);
    let auto = approvals_mode == Some(crate::approvals::Approvals::Auto);
    let profile = harness.profile.clone();
    // The settings and the sandbox home belong to the repository; the agent
    // works in the worktree.
    let home_key = project.clone();
    let worktree = if let Some(c) = opts.child.as_ref().filter(|c| !c.trainer) {
        Some(crate::spawn::create_worktree(env, &project, c)?)
    } else if opts.worktree {
        Some(create_worktree(
            env,
            &project,
            opts.worktree_app.as_deref(),
        )?)
    } else {
        None
    };
    let mut pending = PendingWorktree { env, wt: worktree };
    let project = pending
        .wt
        .as_ref()
        .map(|w| w.path.clone())
        .unwrap_or(project);

    let account = opts.transparent.as_ref().and_then(|t| t.account.as_ref());
    let auth: state::AuthMode = match account {
        Some(dir) => state::AuthMode::account(dir),
        None => opts
            .auth
            .clone()
            .or(m.auth.mode.clone())
            .unwrap_or_else(|| "shared".into())
            .parse()?,
    };
    if let state::AuthMode::Account { dir, .. } = &auth {
        if dir.as_os_str().is_empty() {
            bail!("the account mode is only for `paddock wrap`; use shared or isolated:NAME");
        }
    }
    let id = match (&opts.child, &opts.detach) {
        (Some(c), _) => c.id.clone(),
        (None, Some(id)) => id.clone(),
        (None, None) => state::new_id(&profile.name, &project),
    };
    let detach = match &opts.detach {
        Some(_) => Some(profile.detach.clone().with_context(|| {
            format!(
                "the {} profile has no [detach] table, so paddock does not know how to give its interactive interface a first prompt. Run it without --detach",
                profile.name
            )
        })?),
        None => None,
    };
    // `--profile default`: the home is an overlay on a private snapshot, or
    // a clone (macOS). Its upper folder or the clone gets the credentials.
    let shadow = if opts.profile_default {
        if opts.transparent.is_some() || opts.child.is_some() || profile.app.is_some() {
            bail!("--profile default is only for `paddock run` and `paddock shell`");
        }
        let roots = crate::shadow::roots(env, &profile)?;
        Some(crate::shadow::Plan::create(
            env,
            &id,
            roots,
            profile.state.auth.clone(),
        )?)
    } else {
        None
    };
    let home_src = match &shadow {
        Some(p) => p.home.clone(),
        None => {
            let h = state::sandbox_home(env, &profile.name, &auth, &home_key)?;
            // For `paddock du` and `paddock gc`. The sandbox can rewrite it,
            // so they read it as a hint only.
            crate::storage::write_origin(
                &h,
                &crate::storage::Origin::new(
                    &home_key,
                    &profile.name,
                    &auth,
                    profile.app.is_some(),
                ),
            )?;
            h
        }
    };
    // A failed build leaves no run folder.
    let mut pending_shadow = PendingShadow(shadow.clone());
    let mut spec = SandboxSpec::new(
        id.clone(),
        &profile.name,
        &project,
        home_src,
        &env.home,
        env.uid,
        env.gid,
        &env.user,
    );
    spec.memory_high = memory.high;
    spec.network = opts.network.or(m.network.mode).unwrap_or_default();
    manifest::check_max(m.network.max, spec.network, "the network mode")?;
    spec.publish = m.network.publish.clone();
    // An isolated login is another machine; a host login keeps the host's
    // identity.
    spec.identity = crate::identity::for_login(env, &profile.name, &auth)?;

    if let Some(p) = &shadow {
        if let Some(m) = p.overlay_mount(&env.home) {
            spec.add_mount(m);
        }
        spec.notices.push(format!(
            "profile default: the home holds a private snapshot of {}. Nothing that the run changes goes back to the host",
            p.roots.iter().map(|r| format!("~/{r}")).collect::<Vec<_>>().join(", ")
        ));
        spec.shadow = Some(p.clone());
    }
    spec.add_mount(Mount::same(&project, true, "project"));

    // The harness.
    let porigin = format!("profile {}", profile.name);
    for p in &harness.install_mounts {
        spec.add_mount(Mount::same(p, false, &format!("{porigin}: install")));
    }
    // The harness's own folder: a shell in the sandbox (`paddock shell`),
    // and tools that start the harness by name, find it. The folder is
    // already among the install mounts. It goes last on PATH, so it only
    // adds the harness: a harness in /usr/bin must not put the host's `gh`
    // before paddock's, or the host's `jq` before the project's mise tools.
    if let Some(d) = harness.exe.parent() {
        spec.inherit_path.push(d.to_path_buf());
    }
    for d in &harness.path_dirs {
        spec.add_path(d.clone());
    }
    // The host environment first, so profile, manifest and detectors override it.
    let inherit = opts.inherit_env.or(m.env.inherit).unwrap_or(true);
    if inherit {
        let mut withheld = Vec::new();
        let mut dropped = Vec::new();
        let mut count = 0;
        let account_var = profile.state.account.as_ref().map(|a| a.env.as_str());
        for (k, v) in &env.vars {
            if m.env.hide.contains(k) {
                spec.deny(crate::boundary::ENV, k, "env hide");
                continue;
            }
            if account_var == Some(k.as_str()) {
                spec.deny(crate::boundary::ENV, k, "account folder of the harness");
                continue;
            }
            if !policy::inheritable_var(k, v) || k.starts_with("LC_") {
                continue;
            }
            // Named in [env] pass or env_pass: those loops below add it.
            if policy::is_cloud_secret(k) {
                if !m.env.pass.contains(k) && !profile.state.env_pass.contains(k) {
                    dropped.push(k.clone());
                }
                continue;
            }
            // Real secrets stay on the host. The harness's own login
            // variables (env_pass) and the ones that the user hands over in
            // [env] pass are added below, with their values.
            if policy::looks_secret(k) {
                if !m.env.pass.contains(k) && !profile.state.env_pass.contains(k) {
                    withheld.push(k.clone());
                }
                continue;
            }
            count += 1;
            spec.set_env(k, v.clone(), "host env (inherited)");
        }
        for d in crate::tack::tackshims::host_path(env) {
            spec.inherit_path.push(d.clone());
        }
        // Names only, never values.
        for w in &withheld {
            spec.deny(
                crate::boundary::ENV,
                w,
                "secret-looking name; [env] pass hands it over",
            );
        }
        for d in &dropped {
            spec.deny(
                crate::boundary::ENV,
                d,
                "cloud credential; [env] pass hands it over",
            );
        }
        // Tell the user once per project, and again when the list of
        // secret-looking names changes.
        let login: Vec<String> = profile
            .state
            .env_pass
            .iter()
            .filter(|k| policy::looks_secret(k) && env.var(k).is_some())
            .cloned()
            .collect();
        let mut notice_key: Vec<String> =
            withheld.iter().map(|w| format!("withheld:{w}")).collect();
        notice_key.extend(login.iter().map(|l| format!("login:{l}")));
        notice_key.extend(dropped.iter().map(|d| format!("dropped:{d}")));
        if crate::state::env_notice_changed(env, &project, &notice_key) {
            let mut note = format!("passing {count} host environment variables");
            if !withheld.is_empty() {
                note.push_str(&format!(
                    ". These look like secrets and stay on the host: {}. To give one to the sandbox, add it to [env] pass in the user configuration (the agent can then read it)",
                    withheld.join(", ")
                ));
            }
            if !login.is_empty() {
                note.push_str(&format!(
                    ". The login variables of {} pass with their values, so the harness can sign in: {}. A later paddock keeps them on the host as well",
                    profile.name,
                    login.join(", ")
                ));
            }
            if !dropped.is_empty() {
                note.push_str(&format!(
                    ". Cloud credentials were not passed: {}. To pass one, add it to [env] pass",
                    dropped.join(", ")
                ));
            }
            note.push_str(
                ". To drop one: paddock env hide NAME. To pass only the allowlist: --env clean",
            );
            spec.notices.push(note);
        }
    }
    for (k, v) in &profile.launch.env {
        spec.set_env(k, v.clone(), &porigin);
    }
    // The host proxy, in every env mode: sandbox traffic must leave the way host
    // traffic does. A proxy on the host loopback is the one loopback port the
    // sandbox can reach.
    if !matches!(spec.network, Network::None | Network::Api) {
        for k in crate::proxy::PROXY_VARS {
            if let Some(v) = env.var(k).filter(|v| !v.is_empty()) {
                spec.set_secret_env(k, v, "network: host proxy");
                if let Some((_, host, port)) = crate::proxy::parse_url(v) {
                    if crate::proxy::is_loopback(&host)
                        && !k.to_ascii_uppercase().starts_with("NO_")
                    {
                        spec.add_host_port(port, &format!("network: host proxy in {k}"));
                    }
                }
            }
        }
    }
    for k in &profile.state.env_pass {
        if let Some(v) = env.var(k) {
            spec.set_secret_env(k, v, &format!("{porigin} env_pass"));
        }
    }
    for rel in &profile.state.import_ro {
        // The snapshot has the paths inside the data roots already.
        if shadow
            .as_ref()
            .is_some_and(|p| p.root_of(Path::new(rel)).is_some())
        {
            continue;
        }
        let host = state::host_file(env, &profile, &auth, rel);
        if host.exists() {
            spec.add_mount(Mount::same(&host, false, &format!("{porigin} import_ro")));
        }
    }

    // The manifest.
    // An approved entry is resolved again at each start. A symlink on its
    // way inside a folder the sandbox can write could point it anywhere, so
    // such a symlink is refused (see safefs::resolve_approved).
    for (list, rw, origin) in [
        (&m.mounts.rw, true, "manifest mounts.rw"),
        (&m.mounts.ro, false, "manifest mounts.ro"),
    ] {
        for raw in list {
            let p = env.expand(raw, &project);
            if !p.exists() {
                bail!("{origin}: {} does not exist", p.display());
            }
            let real = crate::safefs::resolve_approved(&p, &sandbox_writable(env, &spec))
                .with_context(|| origin.to_string())?;
            spec.add_mount(Mount::same(real, rw, origin));
        }
    }
    check_harness_install(&harness, &sandbox_writable(env, &spec))?;
    // Host history only for starts through `paddock wrap` (omarchy-agent),
    // where the Omarchy agents panel needs it. A sandbox-written transcript
    // that the host harness loads later (`claude --resume`, `codex resume`)
    // can carry instructions out of the sandbox, so other runs keep their
    // history private in the sandbox home.
    let shared_history = opts.transparent.is_some();
    if shared_history {
        crate::history::apply(env, &profile, &auth, &mut spec)?;
    }
    if let state::AuthMode::Account { dir, .. } = &auth {
        // The host writes refreshed tokens back into the account folder. A
        // folder that a sandbox can write could lead those writes anywhere.
        let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        if let Some(w) = sandbox_writable(env, &spec)
            .iter()
            .find(|w| real.starts_with(w))
        {
            bail!(
                "the account folder {} is inside {}, which a sandbox can write; paddock does not use it as a login",
                dir.display(),
                w.display()
            );
        }
    }
    for k in &m.env.pass {
        if let Some(v) = env.var(k) {
            spec.set_secret_env(k, v, "manifest env.pass");
        }
    }
    for (k, v) in &m.env.set {
        spec.set_env(k, v.clone(), "manifest env.set");
    }
    // A registered credential reaches the sandbox only through its channel
    // (credentials.rs), never with the environment, env_pass or [env] pass.
    for (name, c) in crate::credentials::load(env)? {
        if c.from == crate::credentials::Source::Env {
            spec.env.remove(c.host_var(&name));
        }
        // Its name in the sandbox too: a deliver value or a gateway
        // stand-in replaces it only after the start card.
        spec.env.remove(c.sandbox_var(&name));
    }
    // The `env` prefix of a `paddock wrap` command, as the agent gets it on
    // the host.
    if let Some(t) = &opts.transparent {
        for k in &t.env_unset {
            spec.env.remove(k);
        }
        for (k, v) in &t.env_set {
            spec.set_env(k, v.clone(), "command: env prefix");
        }
    }

    // The detectors.
    let cx = Context {
        env,
        project: &project,
        manifest: &m,
    };
    for d in detect::all() {
        d.apply(&cx, &mut spec)
            .with_context(|| format!("detector {}", d.name()))?;
    }
    detect::npm_harness(env, &harness, &mut spec)?;
    crate::storage::buildcache::apply(env, &home_key, pending.wt.is_some(), &m.setup, &mut spec)?;
    crate::storage::caches::mount(env, &profile, &mut spec);
    spec.setup.extend(m.setup.run.iter().cloned());
    spec.lan = m.network.lan == Some(true);
    if !matches!(spec.network, Network::None | Network::Api) {
        for p in &m.network.host_ports {
            spec.add_host_port(*p, "manifest network.host_ports");
        }
    }
    crate::mcp::apply(env, &profile, &m, &project, &mut spec);
    crate::privacy::push_state_settings(&profile, auto, manual, &porigin, &mut spec)?;
    crate::hooks::apply_all(env, &profile, &auth, &mut spec)?;
    if !matches!(spec.network, Network::None | Network::Api) {
        for p in &profile.network.publish {
            if !spec.publish.contains(p) {
                spec.publish.push(*p);
            }
        }
    }
    if let Some(why) = &profile.launch.wrap_on_host {
        spec.warnings.push(format!("{}: {why}", profile.name));
    }
    // Last, so neither the host environment nor a project file turns
    // telemetry back on. Only the user configuration can ([privacy]).
    let privacy = crate::privacy::mode(&m);
    crate::privacy::apply(&profile, privacy, &mut spec)?;
    if matches!(spec.network, Network::Allowlist | Network::Api) {
        let sources = [
            (
                profile.network.endpoints.clone(),
                format!("{porigin} [network] endpoints"),
            ),
            (m.network.allow.clone(), "[network] allow".to_string()),
            (opts.allow_domains.clone(), "--allow".to_string()),
        ];
        for (items, origin) in sources {
            for d in items {
                spec.allow_origins
                    .entry(d)
                    .or_insert_with(|| origin.clone());
            }
        }
        spec.allow_domains = spec.allow_origins.keys().cloned().collect();
        let macos = cfg!(target_os = "macos");
        let ignored = crate::backend::ignored_entries(spec.network, &spec.allow_domains, macos);
        spec.warnings.extend(ignored);
    }

    // Last, so no host or project variable replaces them.
    if spec.network == Network::Api {
        // Network mode api: the sandbox has no network, only the paddock
        // proxy through a local relay (run.rs). The proxy on the host uses
        // the host proxy variables.
        let url = format!("http://127.0.0.1:{}", crate::proxy::API_PORT);
        for k in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            spec.set_env(k, url.clone(), "network: api proxy");
        }
        for k in ["NO_PROXY", "no_proxy"] {
            spec.set_env(k, "127.0.0.1,localhost", "network: api proxy");
        }
    }

    // The command.
    spec.command = if opts.shell {
        let sh = if env.which("bash").is_some() {
            "bash"
        } else {
            "sh"
        };
        vec![sh.to_string()]
    } else if opts.child.is_some() {
        // The one-shot command, then `args` (spawn/model.rs). Never the prompt.
        let o = profile
            .oneshot
            .as_ref()
            .with_context(|| format!("the {} profile has no [oneshot] command", profile.name))?;
        let mut c = vec![harness.exe.display().to_string()];
        // The privacy flags are global options: before a subcommand.
        c.extend(crate::privacy::args(&profile, privacy));
        c.extend(crate::hostcli::status::harness_args(env, &profile.name));
        c.extend(o.render(&spec.home.join(crate::spawn::prompt_rel(&spec.id))));
        c.extend(opts.args.iter().cloned());
        c
    } else if let Some(t) = opts.transparent.as_ref().filter(|t| !t.shell.is_empty()) {
        // The shell form: the script starts its `$0`, which is the harness.
        // The script's arguments stay as they are.
        let mut c = t.shell.clone();
        c.push(harness.exe.display().to_string());
        c.extend(opts.args.iter().cloned());
        c
    } else {
        let mut c = vec![harness.exe.display().to_string()];
        if auto {
            c.extend(profile.launch.yolo_args.iter().cloned());
        }
        c.extend(crate::privacy::args(&profile, privacy));
        // Timers and limits in the agent's context (hostcli/status.rs).
        if opts.transparent.is_none() {
            c.extend(crate::hostcli::status::harness_args(env, &profile.name));
        }
        // Transparent: the command already has the arguments it needs.
        if opts.transparent.is_none() {
            c.extend(profile.launch.args.iter().cloned());
        }
        // An editor that opens the repository opens the worktree instead.
        let repo_arg = home_key.display().to_string();
        c.extend(opts.args.iter().map(|a| match &pending.wt {
            Some(w) if *a == repo_arg => w.path.display().to_string(),
            _ => a.clone(),
        }));
        // `--detach`: the first prompt names a read-only copy of the task.
        if let Some(d) = &detach {
            c.extend(d.render(&spec.home.join(crate::spawn::prompt_rel(&spec.id))));
        }
        c
    };

    // The display: command line, then manifest, then the desktop detector.
    let marker = gui::desktop_marker(&project);
    spec.gui.mode = match (opts.gui, m.gui.mode, &marker) {
        (Some(g), _, _) => {
            spec.gui.origin = "command line".into();
            g
        }
        (None, Some(g), _) => {
            spec.gui.origin = "manifest gui.mode".into();
            g
        }
        (None, None, Some(found)) => {
            spec.gui.origin = format!("detector desktop: {found}");
            GuiMode::Headless
        }
        _ => GuiMode::Off,
    };
    if spec.gui.mode == GuiMode::Host
        && opts.gui != Some(GuiMode::Host)
        && m.gui.mode != Some(GuiMode::Host)
    {
        bail!("gui mode 'host' must be requested explicitly");
    }
    spec.gui.gpu = opts.gpu || m.gui.gpu.unwrap_or(false);
    if let Some(w) = m.gui.width {
        spec.gui.width = w;
    }
    if let Some(h) = m.gui.height {
        spec.gui.height = h;
    }
    if spec.gui.enabled() {
        for rel in [
            ".local/share/fonts",
            ".fonts",
            ".config/fontconfig",
            ".icons",
            ".local/share/icons",
            ".config/gtk-3.0",
            ".config/gtk-4.0",
        ] {
            let host = env.home.join(rel);
            if host.exists() {
                spec.add_mount(Mount::same(&host, false, "gui: fonts and themes"));
            }
        }
    }
    // The keyring wrapper goes inside the display's private bus, so it is
    // applied first: gui::apply wraps whatever command it finds.
    // In app mode it decides between the private bus with a keyring and the
    // filtered host bus.
    if profile.launch.keyring || spec.gui.mode == GuiMode::App {
        let bus = match opts.app_bus {
            Some(b) => b,
            None => crate::app::bus_mode(env, &spec.harness)?,
        };
        crate::keyring::apply(env, &mut spec, bus)?;
    }
    gui::apply(env, &mut spec)?;

    // A reviewer child reads the files and reports. Its worktree and the
    // repository are read-only; its own home and the caches stay writable.
    if opts
        .child
        .as_ref()
        .is_some_and(|c| c.role == crate::spawn::Role::Reviewer)
    {
        crate::spawn::make_read_only(&mut spec, &[project.clone(), home_key.clone()]);
    }

    // The borrow folder: empty at the start. `paddock-host borrow` copies
    // host folders into it, and the sandbox end removes what is left.
    let borrow = crate::borrow::dir(env, &id);
    crate::privfs::dir(&borrow)?;
    spec.add_mount(Mount::bind(
        &borrow,
        crate::borrow::MOUNT,
        true,
        "borrow folder",
    ));

    // The shared folder of the pasture, and the loft with the Spec Kit store.
    let kv_project = pending
        .wt
        .as_ref()
        .map(|w| w.repo.clone())
        .unwrap_or_else(|| home_key.clone());
    if let Some(name) = &opts.pasture {
        crate::pasture::mounts::apply(env, &mut spec, &kv_project, name, opts.workbench)?;
    }
    crate::loft::mount(env, &mut spec, &kv_project)?;
    let bridge = match (&opts.pasture, &profile.oneshot, &opts.child) {
        (Some(_), Some(o), None) => Some(crate::pasture::bridge::Command::of(
            &harness.exe,
            crate::privacy::args(&profile, privacy),
            o,
            &spec.home,
        )),
        _ => None,
    };

    policy::enforce(env, &spec)?;

    let backend = opts
        .backend
        .clone()
        .or(m.backend.clone())
        .unwrap_or_else(|| "auto".into());
    Ok(Built {
        horse: false,
        spawn: None,
        stdin: None,
        home_key,
        inherit_env: inherit,
        approver: None,
        parent: None,
        device_ceiling: None,
        devices: opts.devices.clone(),
        test_devices: false,
        shared_history,
        view: true,
        detach: opts.detach.is_some(),
        worktree: {
            pending_shadow.0 = None;
            pending.keep()
        },
        pasture: opts.pasture.clone(),
        bridge,
        auth,
        spec,
        profile,
        harness_exe: harness.exe,
        manifest: m,
        backend,
        approvals: approvals_mode,
        approvals_origin: approvals.origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_from_a_sandbox_writable_folder_is_refused() {
        let profile: Profile = toml::from_str("name = \"t\"\n").unwrap();
        let mut h = Harness {
            profile,
            exe: PathBuf::from("/usr/bin/tool"),
            install_mounts: vec![PathBuf::from("/home/demo/.local/share/mise/installs/t/1")],
            path_dirs: vec![],
            source: "test".into(),
        };
        let writable = vec![PathBuf::from("/home/demo/proj")];
        assert!(check_harness_install(&h, &writable).is_ok());
        // An interpreter that a project script names: its folder is on PATH.
        h.path_dirs.push(PathBuf::from("/home/demo/proj/bin"));
        assert!(check_harness_install(&h, &writable).is_err());
        h.path_dirs.clear();
        h.exe = PathBuf::from("/home/demo/proj/node_modules/.bin/tool");
        assert!(check_harness_install(&h, &writable).is_err());
    }

    #[test]
    fn a_symlinked_home_is_refused_as_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("home")).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
        let home = tmp.path().join("link/home");
        let env = HostEnv::from_parts(home.clone(), "t".into(), 1000, 1000, Default::default());
        let h = Harness {
            profile: crate::profile::builtin("codex").unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "codex".into(),
            project: Some(home),
            assume_yes: true,
            ..Default::default()
        };
        let err = build_with(&env, &opts, h).err().expect("refused");
        assert!(err.to_string().contains("home directory"), "{err}");
    }

    /// Build a spec for a built-in profile with a stand-in executable, a
    /// user configuration and a project file.
    pub(super) fn privacy_spec(
        name: &str,
        user: &str,
        project: &str,
    ) -> (tempfile::TempDir, Built) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        let user = format!("{user}\n[compaction]\nenabled = false\n");
        std::fs::write(home.join(".config/paddock/config.toml"), user).unwrap();
        std::fs::write(proj.join("paddock.toml"), project).unwrap();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("DISABLE_TELEMETRY".to_string(), String::new());
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, vars);
        let h = Harness {
            profile: crate::profile::builtin(name).unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: name.into(),
            project: Some(proj),
            assume_yes: true,
            ..Default::default()
        };
        let built = build_with(&env, &opts, h).unwrap();
        (tmp, built)
    }

    /// A Claude Code build in a fresh home, with a user configuration and
    /// a project file.
    fn claude_with(
        user: &str,
        project_file: Option<&str>,
        approvals: Option<crate::approvals::Approvals>,
        assume_yes: bool,
    ) -> (tempfile::TempDir, PathBuf, Result<Built>) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(env.paddock_config().join("config.toml"), user).unwrap();
        if let Some(f) = project_file {
            std::fs::write(proj.join(manifest::PROJECT_FILE), f).unwrap();
        }
        let h = Harness {
            profile: crate::profile::builtin("claude").unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "claude".into(),
            project: Some(proj.clone()),
            assume_yes,
            approvals,
            ..Default::default()
        };
        let b = build_with(&env, &opts, h);
        (tmp, proj, b)
    }

    /// Manual approvals: the same sandbox, but no auto-approval flag, no
    /// bypass answer and no trust answer, so Claude Code asks as usual.
    #[test]
    fn manual_approvals_leave_the_agents_own_questions_alone() {
        use crate::approvals::Approvals;
        let flag = "--dangerously-skip-permissions".to_string();
        let (ta, _p, auto) = claude_with("", None, None, true);
        let auto = auto.unwrap();
        let (tm, _p, manual) = claude_with("", None, Some(Approvals::Manual), true);
        let manual = manual.unwrap();
        assert_eq!(auto.approvals, Some(Approvals::Auto));
        assert_eq!(auto.approvals_origin, "default");
        assert!(auto.spec.command.contains(&flag));
        assert!(auto
            .spec
            .home_settings
            .iter()
            .any(|s| s.file == Path::new(".claude.json")));
        assert_eq!(manual.approvals, Some(Approvals::Manual));
        assert_eq!(manual.approvals_origin, "command line");
        assert!(manual.manual());
        assert!(!manual.spec.command.contains(&flag));
        assert!(
            !manual.spec.home_settings.iter().any(|s| {
                s.path.iter().any(|k| {
                    k == "hasTrustDialogAccepted" || k == "skipDangerousModePermissionPrompt"
                })
            }),
            "{:?}",
            manual.spec.home_settings
        );
        // The sandbox itself does not change: the same network and the
        // same mounts, relative to each test's own folder.
        assert_eq!(auto.spec.network, manual.spec.network);
        let dsts = |b: &Built, root: &Path| -> Vec<String> {
            let root = root.display().to_string();
            b.spec
                .mounts
                .iter()
                .map(|m| m.dst.display().to_string().replace(&root, "ROOT"))
                .collect()
        };
        assert_eq!(dsts(&auto, ta.path()), dsts(&manual, tm.path()));
    }

    #[test]
    fn approvals_come_from_flag_project_agent_then_user() {
        use crate::approvals::Approvals::{Auto, Manual};
        let user = "[run]\napprovals = \"manual\"\n";
        let (_t, _p, b) = claude_with(user, None, None, false);
        let b = b.unwrap();
        assert_eq!(b.approvals, Some(Manual));
        assert_eq!(b.approvals_origin, "user configuration [run] approvals");
        let agent = "[run]\napprovals = \"manual\"\n[run.agents.claude]\napprovals = \"auto\"\n";
        let (_t, _p, b) = claude_with(agent, None, None, false);
        assert_eq!(b.unwrap().approvals, Some(Auto));
        let other = "[run.agents.codex]\napprovals = \"manual\"\n";
        let (_t, _p, b) = claude_with(other, None, None, false);
        assert_eq!(b.unwrap().approvals, Some(Auto));
        // A project file that asks for manual needs no approval.
        let proj = "[run]\napprovals = \"manual\"\n";
        let (_t, _p, b) = claude_with(agent, Some(proj), None, false);
        let b = b.unwrap();
        assert_eq!(b.approvals, Some(Manual));
        assert_eq!(b.approvals_origin, "paddock.toml [run] approvals");
        // `--auto` wins over all of them.
        let (_t, _p, b) = claude_with(user, Some(proj), Some(Auto), false);
        assert_eq!(b.unwrap().approvals, Some(Auto));
    }

    /// The agent can edit the project file: `approvals = "auto"` there
    /// needs the one-time approval, and `[run.agents]` is refused.
    #[test]
    fn a_project_file_needs_approval_for_auto_approvals() {
        use crate::approvals::Approvals::{Auto, Manual};
        let user = "[run]\napprovals = \"manual\"\n";
        let proj = "[run]\napprovals = \"auto\"\n";
        let (_t, _p, b) = claude_with(user, Some(proj), None, false);
        let e = format!("{:#}", b.err().expect("not approved"));
        assert!(e.contains("not approved"), "{e}");
        let (_t, _p, b) = claude_with(user, Some(proj), None, true);
        let b = b.unwrap();
        assert_eq!(b.approvals, Some(Auto));
        let (_t, _p, b) = claude_with(user, Some(proj), Some(Manual), true);
        assert_eq!(b.unwrap().approvals, Some(Manual));
        let agents = "[run.agents.claude]\napprovals = \"auto\"\n";
        let (_t, _p, b) = claude_with("", Some(agents), None, true);
        let e = format!("{:#}", b.err().expect("refused"));
        assert!(
            e.contains("[run.agents] is only read from the user configuration"),
            "{e}"
        );
    }

    /// A detached Claude Code run has no terminal to answer its own two
    /// questions: the bypass warning (only with auto-approval) and the
    /// project trust question.
    #[test]
    fn claude_starts_without_its_trust_and_bypass_questions() {
        let built = |no_yolo: bool| {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            let proj = tmp.path().join("proj");
            std::fs::create_dir_all(&proj).unwrap();
            let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
            let h = Harness {
                profile: crate::profile::builtin("claude").unwrap(),
                exe: PathBuf::from("/usr/bin/true"),
                install_mounts: vec![],
                path_dirs: vec![],
                source: "test".into(),
            };
            let opts = Options {
                harness: "claude".into(),
                project: Some(proj.clone()),
                assume_yes: true,
                no_yolo,
                ..Default::default()
            };
            let b = build_with(&env, &opts, h).unwrap();
            (tmp, proj, b)
        };
        let has = |b: &Built, file: &str, path: &[String]| {
            b.spec.home_settings.iter().any(|s| {
                s.file == Path::new(file) && s.path == path && s.value == serde_json::json!(true)
            })
        };
        let bypass = vec!["skipDangerousModePermissionPrompt".to_string()];
        let (_t, proj, b) = built(false); // the path getcwd sees
        let trust = vec![
            "projects".to_string(),
            std::fs::canonicalize(&proj).unwrap().display().to_string(),
            "hasTrustDialogAccepted".to_string(),
        ];
        assert!(has(&b, ".claude/settings.json", &bypass));
        assert!(has(&b, ".claude.json", &trust));
        let (_t, proj, b) = built(true);
        let trust = vec![
            "projects".to_string(),
            std::fs::canonicalize(&proj).unwrap().display().to_string(),
            "hasTrustDialogAccepted".to_string(),
        ];
        assert!(
            !has(&b, ".claude/settings.json", &bypass),
            "no bypass answer with --no-yolo"
        );
        assert!(has(&b, ".claude.json", &trust));
        // settings.json is a writable copy now, so the answer can be written.
        let p = crate::profile::builtin("claude").unwrap();
        assert!(p.state.seed.iter().any(|s| s == ".claude/settings.json"));
        assert!(!p
            .state
            .import_ro
            .iter()
            .any(|s| s == ".claude/settings.json"));
    }

    #[test]
    fn the_harness_folder_is_on_the_sandbox_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        let tools = tmp.path().join("tools/codex/1.0/bin");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
        let h = Harness {
            profile: crate::profile::builtin("codex").unwrap(),
            exe: tools.join("codex"),
            install_mounts: vec![tools.parent().unwrap().to_path_buf()],
            path_dirs: vec![],
            source: "test".into(),
        };
        for shell in [false, true] {
            let opts = Options {
                harness: "codex".into(),
                project: Some(proj.clone()),
                assume_yes: true,
                shell,
                ..Default::default()
            };
            let b = build_with(&env, &opts, h.clone()).unwrap();
            let path = b.spec.path_value();
            let dirs: Vec<&str> = path.split(':').collect();
            let at = dirs.iter().position(|d| Path::new(d) == tools);
            assert!(at.is_some(), "shell={shell}: {path}");
            // Never before paddock's own tools (gh, git) or the system tools.
            let usr_bin = dirs.iter().position(|d| *d == "/usr/bin").unwrap();
            assert!(at.unwrap() > usr_bin, "{path}");
        }
        // A harness in /usr/bin changes nothing: no second /usr/bin, and
        // the mise shims and paddock's tools stay before it.
        let opts = Options {
            harness: "codex".into(),
            project: Some(proj.clone()),
            assume_yes: true,
            ..Default::default()
        };
        let before = build_with(&env, &opts, h.clone())
            .unwrap()
            .spec
            .path_value();
        let sys = Harness {
            exe: PathBuf::from("/usr/bin/codex"),
            ..h.clone()
        };
        let after = build_with(&env, &opts, sys).unwrap().spec.path_value();
        assert_eq!(after.matches(":/usr/bin:").count(), 1, "{after}");
        assert_eq!(
            after,
            before.replace(&format!(":{}", tools.display()), ""),
            "{after}"
        );
    }

    #[test]
    fn a_pasture_member_gets_the_shared_folder() {
        let (t, b) = privacy_spec("claude", "", "");
        assert!(b.pasture.is_none());
        assert!(!b.spec.mounts.iter().any(|m| m.dst == Path::new("/pasture")));
        assert!(!b.spec.env.contains_key("PADDOCK_PASTURE"));
        drop(t);
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
        let h = Harness {
            profile: crate::profile::builtin("claude").unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "claude".into(),
            project: Some(proj.clone()),
            assume_yes: true,
            pasture: Some("team".into()),
            ..Default::default()
        };
        let b = build_with(&env, &opts, h).unwrap();
        let m = b
            .spec
            .mounts
            .iter()
            .find(|m| m.dst == Path::new("/pasture"))
            .expect("no /pasture mount");
        assert_eq!(m.kind, crate::spec::MountKind::Rw);
        let ground = crate::pasture::ground(&env, &b.home_key, "team");
        assert_eq!(m.src.as_deref(), Some(ground.as_path()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&ground).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(b.spec.env["PADDOCK_PASTURE"].value, "/pasture");
        assert_eq!(b.spec.env["PADDOCK_PASTURE_NAME"].value, "team");
        // The workbench folder: read-only for a member.
        let w = b
            .spec
            .mounts
            .iter()
            .find(|m| m.dst == Path::new("/run/paddock/workbench"))
            .expect("no workbench mount");
        assert_eq!(w.kind, crate::spec::MountKind::Ro);
        assert_eq!(
            w.src.as_deref(),
            Some(crate::pasture::workbench::dir(&env, &b.home_key, "team").as_path())
        );
        assert_eq!(
            b.spec.env["PADDOCK_WORKBENCH"].value,
            "/run/paddock/workbench"
        );
        // Port sockets: the tree is read-only, this member's folder is not.
        let ports = crate::pasture::ports::host_dir(&env, &b.home_key, "team");
        let tree = b
            .spec
            .mounts
            .iter()
            .find(|m| m.dst == Path::new(crate::pasture::ports::PORTS_MOUNT))
            .expect("no ports mount");
        assert_eq!(tree.kind, crate::spec::MountKind::Ro);
        assert_eq!(tree.src.as_deref(), Some(ports.as_path()));
        let own = crate::pasture::ports::member_dir(&ports, &b.spec.id).unwrap();
        let mine = b
            .spec
            .mounts
            .iter()
            .find(|m| m.dst == Path::new(crate::pasture::ports::OWN_MOUNT))
            .expect("no own ports mount");
        assert_eq!(mine.kind, crate::spec::MountKind::Rw);
        assert_eq!(mine.src.as_deref(), Some(own.as_path()));
        assert_eq!(
            b.spec.env[crate::pasture::ports::PORTS_ENV].value,
            crate::pasture::ports::PORTS_MOUNT
        );
        assert_eq!(
            b.spec.env[crate::pasture::ports::OWN_ENV].value,
            crate::pasture::ports::OWN_MOUNT
        );
        assert!(!ports.starts_with(&ground));
        // The admission check of `build` comes before this: a configured
        // member only.
        assert!(build(&env, &opts).is_err());
    }

    #[test]
    fn profile_ports_settings_and_host_notices_reach_the_spec() {
        // dsh serves its UI to the host browser on one loopback port.
        let (_t, b) = privacy_spec("dsh", "", "");
        assert_eq!(b.spec.publish, vec![3080]);
        let (_t, b) = privacy_spec("dsh", "[network]\nmode = \"none\"\n", "");
        assert!(b.spec.publish.is_empty());
        // A self-update switch goes into the sandbox copy of the settings.
        let (_t, b) = privacy_spec("devin", "", "");
        assert!(b
            .spec
            .home_settings
            .iter()
            .any(|s| s.path == ["auto_update"]
                && s.value == serde_json::json!(false)
                && s.origin.contains("state.settings")));
        // muse keeps its own sandbox: `paddock muse` says what that means.
        let (_t, b) = privacy_spec("muse", "", "");
        assert!(b.spec.warnings.iter().any(|w| w.contains("own sandbox")));
    }

    #[test]
    fn pure_caches_are_tmpfs_in_the_spec_unless_the_user_turns_them_off() {
        use crate::spec::MountKind;
        let tmpfs = |b: &Built| -> Vec<PathBuf> {
            b.spec
                .mounts
                .iter()
                .filter(|m| m.kind == MountKind::Tmpfs)
                .filter_map(|m| m.dst.strip_prefix(&b.spec.home).ok())
                .map(Path::to_path_buf)
                .collect()
        };
        let (_t, b) = privacy_spec("codex", "", "");
        let on = tmpfs(&b);
        for p in [
            ".cache/mesa_shader_cache",
            ".cache/fontconfig",
            ".codex/cache",
        ] {
            assert!(on.contains(&PathBuf::from(p)), "{p}: {on:?}");
        }
        // Paths only known after the app ran are for gc, never a mount.
        assert!(!on.iter().any(|p| p.to_string_lossy().contains('*')));
        // Never a login or a settings file.
        assert!(!on.iter().any(|p| p.starts_with(".codex/auth.json")));
        let (_t, b) = privacy_spec("claude", "", "");
        assert!(!tmpfs(&b).contains(&PathBuf::from(".codex/cache")));
        let (_t, b) = privacy_spec("codex", "[storage]\ntmpfs_caches = false\n", "");
        assert!(tmpfs(&b).is_empty(), "{:?}", tmpfs(&b));
    }

    #[test]
    fn an_npm_harness_shares_the_npm_cache_without_a_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        let pkg = tmp.path().join("tools/lib/node_modules/fake-agent");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(pkg.join("bin")).unwrap();
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
        let profile: Profile = toml::from_str("name = \"fake\"\n").unwrap();
        let opts = Options {
            harness: "fake".into(),
            project: Some(proj.clone()),
            assume_yes: true,
            ..Default::default()
        };
        let npm = |exe: PathBuf| {
            let h = Harness {
                profile: profile.clone(),
                exe,
                install_mounts: vec![],
                path_dirs: vec![],
                source: "test".into(),
            };
            let b = build_with(&env, &opts, h).unwrap();
            let dst = b.spec.home.join(".npm");
            b.spec
                .mounts
                .iter()
                .find(|m| m.dst == dst)
                .and_then(|m| m.src.clone())
        };
        let shared = npm(pkg.join("bin/agent.js")).expect("an npm harness gets .npm");
        assert!(shared.ends_with("npm"), "{}", shared.display());
        assert!(shared.starts_with(crate::storage::package_cache::root(&env)));
        assert_eq!(npm(PathBuf::from("/usr/bin/true")), None);
    }

    #[test]
    fn dates_for_branch_names() {
        assert_eq!(ymd(0), "19700101");
        assert_eq!(ymd(951_782_400), "20000229");
        assert_eq!(ymd(4_102_444_799), "20991231");
    }

    #[test]
    fn an_app_worktree_has_its_own_branch_and_report() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        g(&repo, &["init", "-q", "-b", "main"]);
        g(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let repo = std::fs::canonicalize(&repo).unwrap();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert(
            "XDG_DATA_HOME".to_string(),
            tmp.path().join("data").display().to_string(),
        );
        // PATH so that the sandboxed work-tree check can find bwrap and git.
        if let Ok(p) = std::env::var("PATH") {
            vars.insert("PATH".to_string(), p);
        }
        let env = HostEnv::from_parts(tmp.path().join("home"), "t".into(), 1000, 1000, vars);
        let w = create_worktree(&env, &repo, Some("vscode")).unwrap();
        assert!(w
            .branch
            .starts_with(&format!("paddock/vscode-{}-", today())));
        assert!(w
            .path
            .starts_with(tmp.path().join("data/paddock/worktrees")));
        assert!(worktree_report(&env, &w).contains("made no changes"));
        g(
            &w.path,
            &["commit", "-q", "--allow-empty", "-m", "work in the app"],
        );
        std::fs::write(w.path.join("new.txt"), "x").unwrap();
        let r = worktree_report(&env, &w);
        assert!(r.contains("work in the app"), "{r}");
        // The worktree state comes from a sandboxed `git status`. A host that
        // cannot run one reports it as unknown instead of guessing.
        let (commits, changed) = worktree_counts(&env, &w);
        assert_eq!(commits, 1);
        match changed {
            Some(n) => {
                assert_eq!(n, 1);
                assert!(r.contains("?? new.txt"), "{r}");
            }
            None => assert!(r.contains("cannot read the worktree state") || r.contains("commits")),
        }
        // The checkout itself stays on its branch.
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main");
    }

    #[test]
    fn a_failed_build_does_not_leave_a_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        g(&repo, &["init", "-q", "-b", "main"]);
        g(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let missing = tmp.path().join("no-such-mount");
        std::fs::write(
            repo.join("paddock.toml"),
            format!("[mounts]\nro = [\"{}\"]\n", missing.display()),
        )
        .unwrap();
        let repo = std::fs::canonicalize(&repo).unwrap();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert(
            "XDG_DATA_HOME".to_string(),
            tmp.path().join("data").display().to_string(),
        );
        if let Ok(p) = std::env::var("PATH") {
            vars.insert("PATH".to_string(), p);
        }
        let env = HostEnv::from_parts(tmp.path().join("home"), "t".into(), 1000, 1000, vars);
        let h = Harness {
            profile: crate::profile::builtin("claude").unwrap(),
            exe: PathBuf::from("/usr/bin/true"),
            install_mounts: vec![],
            path_dirs: vec![],
            source: "test".into(),
        };
        let opts = Options {
            harness: "claude".into(),
            project: Some(repo.clone()),
            assume_yes: true,
            worktree: true,
            ..Default::default()
        };
        let Err(err) = build_with(&env, &opts, h) else {
            panic!("build succeeded");
        };
        assert!(err.to_string().contains("does not exist"), "{err:#}");
        let listed = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout);
        assert!(
            !listed.contains("paddock/wt-"),
            "leftover worktree:\n{listed}"
        );
        let branches = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--list", "paddock/wt-*"])
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&branches.stdout);
        assert!(branches.trim().is_empty(), "leftover branch:\n{branches}");
    }
}
