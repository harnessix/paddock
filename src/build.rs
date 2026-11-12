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
mod tests;
