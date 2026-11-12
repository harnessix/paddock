//! Launch a built spec, wait for it, and clean up.
use crate::backend::{self, Backend, Launch};
use crate::build::Built;
use crate::env::HostEnv;
use crate::gui::{self, WindowCompositor};
use crate::manifest::hex;
use crate::spec::{GuiMode, Network, SandboxSpec};
use crate::state::{self, AuthSync, Running, SyncStep};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::IsTerminal;
mod capped;
mod end;
mod host_tools;
mod mcp;
pub mod memory_scope;
use capped::read_capped;
use end::{EndOfRun, ShadowRun};
mod help;
mod sshagent;
use help::help_text;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
pub fn write_files(launch: &Launch) -> Result<()> {
    for (path, text, mode) in &launch.files {
        gui::write_file(path, text, *mode)?;
    }
    Ok(())
}

/// Make sure that the harness still accepts its auto-approval flags. Flags get
/// renamed between versions; without this check a harness could start in a
/// mode that waits for approvals that nobody gives.
pub fn probe_flags(env: &HostEnv, built: &Built) -> Result<()> {
    probe_args(
        env,
        &built.harness_exe,
        &built.profile.launch.yolo_args,
        &built.profile.launch.probe_args,
        "yolo_args",
    )
}

/// Make sure that the help output (`exe probe`) still mentions every flag
/// in `args`. `what` names the profile entry in the error.
pub fn probe_args(
    env: &HostEnv,
    exe: &Path,
    args: &[String],
    probe: &[String],
    what: &str,
) -> Result<()> {
    let flags: Vec<&String> = args
        .iter()
        .filter(|a| a.starts_with('-') && a.len() > 1)
        .collect();
    if flags.is_empty() {
        return Ok(());
    }
    let meta = std::fs::metadata(exe)?;
    let key = format!(
        "{}|{}|{}|{:?}|{:?}",
        exe.display(),
        meta.len(),
        meta.mtime(),
        flags,
        probe
    );
    let cache = env
        .paddock_state()
        .join("probe")
        .join(&hex(&Sha256::digest(key.as_bytes()))[..16]);
    if cache.is_file() {
        return Ok(());
    }
    let text = help_text(exe, probe)?;
    let missing: Vec<&str> = flags
        .iter()
        .filter(|f| !text.contains(f.as_str()))
        .map(|f| f.as_str())
        .collect();
    if !missing.is_empty() {
        bail!(
            "{} {} no longer mentions {}. The harness may have renamed a flag. \
             Update {what} in a profile override under {}",
            exe.display(),
            probe.join(" "),
            missing.join(", "),
            env.paddock_config().join("harness.d").display()
        );
    }
    crate::privfs::write(&cache, "ok")?;
    Ok(())
}

/// The credentials that a run uses.
#[derive(Default)]
struct RunCredentials {
    delivered: Vec<String>,
    /// The marks of the delivered values, for the login guard of the
    /// pasture bridge. Never the values.
    marks: std::collections::BTreeMap<String, Vec<u64>>,
    gateway: Vec<String>,
    agent: Vec<String>,
    secret_files: Vec<String>,
    /// Clears the gateway routes when the run ends, which wipes their values.
    gateway_routes: Option<ClearOnDrop>,
}

/// Drops the routes of the credential gateway at the end of the run.
struct ClearOnDrop(std::sync::Arc<crate::credentials::gateway::Gateway>);

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// Hand one `deliver` value to the sandbox: an environment variable, or a
/// secret file for `as = "file"`, which sets no variable.
fn deliver(
    spec: &mut crate::spec::SandboxSpec,
    secrets: &crate::credentials::secret::SecretFiles,
    out: &mut RunCredentials,
    c: &crate::credentials::Credential,
    name: String,
    var: &str,
    value: crate::credentials::secret::Secret,
) {
    out.marks
        .insert(name.clone(), crate::pasture::bridge::marks(value.as_str()));
    if c.as_ == crate::credentials::DeliverAs::File {
        secrets.insert(&name, value);
        out.secret_files.push(name);
    } else {
        spec.set_secret_env(var, value.as_str(), &format!("credentials: deliver {name}"));
        out.delivered.push(name);
    }
}

/// Ask for the registered credentials and set up the ones that the user
/// allowed twice: `deliver` values go into the environment, or into
/// `secrets` for `/dev/paddock/secret` with `as = "file"`; a `gateway`
/// credential gets a gateway on a socket (alive as long as this process),
/// a stand-in and a base URL.
fn setup_credentials(
    env: &HostEnv,
    built: &mut Built,
    backend: &str,
    quiet: bool,
    secrets: &crate::credentials::secret::SecretFiles,
    broker: Option<&std::sync::Arc<crate::broker::Broker>>,
) -> Result<RunCredentials> {
    use crate::credentials::{gateway, DeliverAs, Via};
    let say = |m: &str| {
        if !quiet {
            eprintln!("paddock: {m}");
        }
    };
    let mut plan = match crate::credentials::plan(env, &built.spec.harness, built.spawn.is_some()) {
        Ok(p) => p,
        Err(e) => {
            say(&format!("no credentials are delivered: {e:#}"));
            return Ok(RunCredentials::default());
        }
    };
    if matches!(backend, "srt" | "seatbelt") {
        plan.deliver.retain(|(n, c)| {
            let keep = c.via != Via::Gateway;
            if !keep {
                say(&format!(
                    "credential {n}: the {backend} backend cannot reach the gateway; use the bwrap, docker or podman backend"
                ));
            }
            keep
        });
    }
    if built.spec.devices.is_none() && built.spec.device_socket.is_none() {
        plan.deliver.retain(|(n, c)| {
            let keep = c.as_ != DeliverAs::File;
            if !keep {
                say(&format!(
                    "credential {n}: as = \"file\" needs /dev/paddock, which this sandbox does not have; the sandbox does not get it"
                ));
            }
            keep
        });
    }
    for n in &plan.notices {
        say(n);
    }
    if plan.deliver.is_empty() {
        return Ok(RunCredentials::default());
    }
    let approver: std::sync::Arc<dyn crate::broker::Approver> = built
        .approver
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(crate::broker::HostApprover));
    let wait = crate::broker::HostPolicy::load(env)
        .map(|p| p.wait)
        .unwrap_or(120);
    say("waiting for your answer on the approval card before the harness starts (credentials)");
    if !crate::credentials::ask(
        env,
        &built.spec,
        approver.as_ref(),
        Duration::from_secs(wait),
        &plan,
    ) {
        say("the sandbox starts without the credentials");
        return Ok(RunCredentials::default());
    }
    let (values, notices) = crate::credentials::read(env, &plan);
    for n in &notices {
        say(n);
    }
    let mut out = RunCredentials::default();
    let mut routes = std::collections::BTreeMap::new();
    let mut agents = Vec::new();
    let mut channels = Vec::new();
    let mut git = Vec::new();
    for (name, var, value) in values {
        let Some((_, c)) = plan.deliver.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        if c.via == Via::Agent && c.kind() != crate::credentials::Kind::Ssh {
            channels.push((name, c.clone()));
        } else if c.via == Via::Agent {
            agents.push((name, c.clone()));
        } else if c.via == Via::Gateway {
            let mark = gateway::new_mark();
            built.spec.set_env(
                &var,
                gateway::stand_in(value.as_str(), &mark),
                &format!("credentials: stand-in for {name}"),
            );
            if let Some(base) = &c.base_url_env {
                built.spec.set_env(
                    base,
                    format!(
                        "http://127.0.0.1:{}/s/{name}{}",
                        gateway::PORT,
                        c.base_path.as_deref().unwrap_or("")
                    ),
                    &format!("credentials: gateway for {name}"),
                );
            }
            if c.is_git() {
                git.push((name.clone(), c.hosts[0].clone()));
            }
            crate::credentials::audit(
                env,
                &built.spec,
                &name,
                &format!("gateway ready, stand-in mark {mark}"),
            );
            routes.insert(name.clone(), gateway::Route::new(c, value));
            out.gateway.push(name);
        } else {
            crate::credentials::audit(env, &built.spec, &name, "delivered");
            deliver(&mut built.spec, secrets, &mut out, c, name, &var, value);
        }
    }
    if !agents.is_empty() {
        match sshagent::start(env, built, &approver, Duration::from_secs(wait), &agents) {
            Ok(()) => out.agent = agents.into_iter().map(|(n, _)| n).collect(),
            Err(e) => say(&format!(
                "the sandbox starts without the ssh agent socket: {e:#}"
            )),
        }
    }
    if !channels.is_empty() {
        match start_channels(
            env,
            built,
            broker,
            &approver,
            Duration::from_secs(wait),
            &channels,
        ) {
            Ok(()) => out.agent.extend(channels.into_iter().map(|(n, _)| n)),
            Err(e) => say(&format!(
                "the sandbox starts without the docker and aws credentials: {e:#}"
            )),
        }
    }
    if !git.is_empty() {
        for (k, v) in crate::credentials::git_env(gateway::PORT, &git) {
            built
                .spec
                .set_env(&k, v, "credentials: git through the gateway");
        }
    }
    if !routes.is_empty() {
        let run_dir = env.paddock_run().join(&built.spec.id);
        crate::privfs::dir(&run_dir)?;
        let sock = crate::sockpath::named(env, &built.spec.id, "credentials.sock")?;
        let (e1, e2, spec) = (env.clone(), env.clone(), built.spec.clone());
        let id = built.spec.id.clone();
        let hosts: Vec<String> = routes.values().map(|r| r.host.clone()).collect();
        let g = gateway::Gateway::new(
            routes,
            gateway::host_connector(env, &hosts),
            crate::credentials::revoked_file(env, &built.spec.id),
            std::sync::Arc::new(
                move |name: &str, method: &str, host: &str, path: &str, result: &str| {
                    crate::credentials::audit_use(&e1, &spec, name, method, host, path, result);
                },
            ),
            std::sync::Arc::new(move || {
                let reason = "the credential gateway refused too many requests";
                if let Err(e) = crate::freeze::freeze(&e2, &id, false, reason) {
                    log::warn!("cannot freeze {id}: {e:#}");
                }
                let _ = crate::broker::notify::show(
                    &e2,
                    &crate::broker::notify::Note {
                        summary: "paddock froze a sandbox".into(),
                        body: format!(
                            "{id}: {reason}. Read paddock log, then run paddock thaw {s} or paddock stop {s}.",
                            s = crate::ids::short(&id)
                        ),
                        actions: Vec::new(),
                        critical: true,
                    },
                );
            }),
        );
        out.gateway_routes = Some(ClearOnDrop(g.serve(&sock)?));
        built.spec.add_mount(crate::spec::Mount::bind(
            &sock,
            gateway::SOCKET_IN_SANDBOX,
            false,
            "credentials: gateway",
        ));
        let me = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .context("cannot find the paddock executable")?;
        built
            .spec
            .add_tool("paddock", me, "credentials: gateway relay");
        built.spec.setup.insert(
            0,
            format!(
                "paddock relay --listen 127.0.0.1:{port} --socket {sock} --ready-file /tmp/.paddock-credentials >/dev/null 2>&1 &\ni=0; while [ ! -e /tmp/.paddock-credentials ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done",
                port = gateway::PORT,
                sock = gateway::SOCKET_IN_SANDBOX
            ),
        );
    }
    Ok(out)
}

/// Network mode `api` on bwrap, docker and podman: the sandbox has no
/// network. The paddock proxy (proxy.rs) on a Unix socket lets through TLS to
/// the allowed hosts, and `paddock relay` offers it on 127.0.0.1:3131 in the
/// sandbox. srt enforces the list with its own proxy, and seatbelt starts
/// its proxy with `backend::start_host_proxy`.
fn start_api_proxy(env: &HostEnv, built: &mut Built, backend: &str) -> Result<()> {
    if built.spec.network != Network::Api || matches!(backend, "srt" | "seatbelt" | "vm") {
        return Ok(());
    }
    use crate::proxy::{allow, API_PORT, API_SOCKET_IN_SANDBOX};
    let run_dir = env.paddock_run().join(&built.spec.id);
    crate::privfs::dir(&run_dir)?;
    let sock = crate::sockpath::named(env, &built.spec.id, "api-proxy.sock")?;
    let (list, _) = allow::Allowlist::parse(&built.spec.allow_domains);
    let refusals = backend::api_refusals(env, &built.spec);
    let mut cfg = backend::host_proxy_cfg(env, &built.spec);
    // The api proxy reaches no host loopback port.
    cfg.allow_loopback = Vec::new();
    cfg.api = Some(allow::ApiPolicy::api(list, Some(refusals)));
    let sock = crate::vm::agent::api_proxy(sock, cfg)?;
    built.spec.add_mount(crate::spec::Mount::bind(
        &sock,
        API_SOCKET_IN_SANDBOX,
        false,
        "network: api proxy",
    ));
    let me = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("cannot find the paddock executable")?;
    built
        .spec
        .add_tool("paddock", me, "network: api proxy relay");
    built.spec.setup.insert(
        0,
        format!(
            "paddock relay --listen 127.0.0.1:{API_PORT} --socket {API_SOCKET_IN_SANDBOX} --ready-file /tmp/.paddock-api >/dev/null 2>&1 &\ni=0; while [ ! -e /tmp/.paddock-api ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done"
        ),
    );
    Ok(())
}

/// The docker and aws credential channels (credentials/channels.rs): the
/// broker serves them, and the sandbox gets a docker configuration with
/// the paddock credential helper and an AWS configuration whose profiles
/// run `paddock-host credential aws`.
fn start_channels(
    env: &HostEnv,
    built: &mut Built,
    broker: Option<&std::sync::Arc<crate::broker::Broker>>,
    approver: &std::sync::Arc<dyn crate::broker::Approver>,
    wait: Duration,
    creds: &[(String, crate::credentials::Credential)],
) -> Result<()> {
    use crate::credentials::{channels, Kind};
    let broker = broker.context("the host passthrough is not running")?;
    let run_dir = env.paddock_run().join(&built.spec.id);
    crate::privfs::dir(&run_dir)?;
    let registries: Vec<String> = creds
        .iter()
        .filter(|(_, c)| c.kind() == Kind::Docker)
        .flat_map(|(_, c)| c.registries.iter().cloned())
        .collect();
    if !registries.is_empty() {
        // Its own folder: docker and buildx write next to config.json.
        let dir = run_dir.join("docker-config");
        crate::privfs::dir(&dir)?;
        gui::write_file(
            &dir.join("config.json"),
            &channels::sandbox_docker_config(&registries),
            0o644,
        )?;
        built.spec.add_mount(crate::spec::Mount::bind(
            &dir,
            channels::DOCKER_CONFIG_IN_SANDBOX,
            true,
            "credentials: docker configuration",
        ));
        built.spec.set_env(
            "DOCKER_CONFIG",
            channels::DOCKER_CONFIG_IN_SANDBOX,
            "credentials: docker credential helper",
        );
        let helper = run_dir.join("docker-credential-paddock");
        gui::write_file(
            &helper,
            "#!/bin/sh\nexec paddock host credential docker \"$@\"\n",
            0o755,
        )?;
        built.spec.add_tool(
            "docker-credential-paddock",
            helper,
            "credentials: docker credential helper",
        );
    }
    channels::setup_aws(&mut built.spec, &run_dir, creds)?;
    let ch = channels::Channels::new(
        env.clone(),
        built.spec.clone(),
        creds.to_vec(),
        crate::credentials::revoked_file(env, &built.spec.id),
        channels::approver_ask(env.clone(), built.spec.clone(), approver.clone(), wait),
    );
    if broker.channels.set(std::sync::Arc::new(ch)).is_err() {
        bail!("the credential channels are set already");
    }
    for (name, c) in creds {
        crate::credentials::audit(
            env,
            &built.spec,
            name,
            &format!("{} channel ready", c.kind().name()),
        );
    }
    Ok(())
}

pub struct Outcome {
    pub code: i32,
    pub stdout: Option<String>,
}

/// Run the sandbox. With `capture`, stdout is collected instead of shown.
pub fn run(
    env: &HostEnv,
    mut built: Built,
    backend: &dyn Backend,
    capture: bool,
    quiet: bool,
) -> Result<Outcome> {
    let _package_cache_lease = crate::storage::package_cache::RunCache::start(env, &built.spec)?;
    mcp::validate(env, &built)?;
    // The Codex hook trust names the home that the harness sees.
    built.spec.harness_home = backend.harness_home(&built.spec);
    // SIGTERM and SIGHUP must leave time for the device mount, login and history copies, and
    // session record to finish. Before launch, paddock notes the signal and stops. After launch it
    // forwards the signal and waits. A frozen sandbox is thawed first.
    crate::loft::push::start_notice(env, &built);
    let forwarding = Forward::install();
    // The mailbox comes first: the devices serve it.
    let mut end = EndOfRun::new(env, &built, quiet);
    // The loft sync mode syncs at each change, and once more when it stops, before `end`.
    let _loft_watch = crate::loft::sync::Watch::start(env, &built.spec.project, &built.spec.id);
    // `--profile default`: the private snapshot of the data roots, before
    // anything else starts. The run folder goes when `run` returns.
    let shadow = built.spec.shadow.clone().map(ShadowRun);
    if let Some(ShadowRun(p)) = &shadow {
        let copied = p.fill(&env.home, crate::shadow::copy_limit(env), None)?;
        if !quiet {
            eprintln!("paddock: {}", copied.line());
            let host = crate::shadow::host_copies(&built.profile, &built.harness_exe);
            if !host.is_empty() {
                eprintln!(
                    "paddock: {} runs on the host too (pid {}). The snapshot is private: nothing of this run goes back, and nothing of that one comes in",
                    built.spec.harness,
                    host.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                );
            }
        }
    }
    let _horse_lifecycle = crate::horse::lifecycle::start(env, &mut built)?;
    let (horse, _) = crate::horse::live::take_hooks();
    let horse_access = _horse_lifecycle.as_ref().map(|l| l.access()).transpose()?;
    let tty = horse.is_none() && !capture && std::io::stdin().is_terminal();
    let tty = tty && std::io::stdout().is_terminal();
    let term = crate::relay::plan(tty, crate::relay::this_terminal_is_watched, built.detach);
    let (caller_terminal, relay_on, terminal_watched) = (tty, term.relay, term.watched);
    let interactive = caller_terminal;
    // App mode: a restricted Wayland socket and a portal-only session bus.
    let _app = if built.spec.gui.mode == GuiMode::App {
        Some(crate::app::start_session(
            env,
            &mut built.spec,
            &built.profile,
        )?)
    } else {
        None
    };
    let _compositor = if built.spec.gui.mode == GuiMode::Window {
        let c = WindowCompositor::start(env, &built.spec)?;
        built.spec.gui.host_socket = Some(c.socket.clone());
        Some(c)
    } else {
        None
    };
    backend.prepare(env, &built.spec, interactive)?;
    let (repo, spawner, broker) = host_tools::broker(env, &mut built, interactive, backend.name())?;
    if let (Some(b), Some(access)) = (&broker, &horse_access) {
        let _ = b.horse.set(access.clone());
    }
    // kv and the devices are optional: without one the sandbox starts, and a notice says why.
    let kv_approver: std::sync::Arc<dyn crate::broker::Approver> = built
        .approver
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(crate::broker::HostApprover));
    let kv_project = built
        .worktree
        .as_ref()
        .map(|w| w.repo.clone())
        .unwrap_or_else(|| built.spec.project.clone());
    let lineage = crate::kv::Lineage {
        parent: built.parent.clone(),
        chain: built
            .spawn
            .as_ref()
            .map(|c| c.chain.clone())
            .unwrap_or_default(),
        can_spawn: spawner.as_ref().is_some_and(|s| s.available()),
    };
    let kv_project_path = kv_project.clone();
    let kv_pasture = built.pasture.clone();
    end.kv = crate::kv::start(
        env,
        &mut built.spec,
        kv_project,
        built.pasture.clone(),
        kv_approver,
        lineage,
        built.bridge.clone(),
    );
    if let (Some(b), Some(k)) = (&broker, &end.kv) {
        let _ = b.kv.set(k.service());
    }
    // The secret files of /dev/paddock: empty until the user allows the
    // credentials, after the install steps.
    let secrets = {
        let (e, spec) = (env.clone(), built.spec.clone());
        crate::credentials::secret::SecretFiles::new(
            Some(crate::credentials::revoked_file(env, &built.spec.id)),
            Some(std::sync::Arc::new(move |name: &str| {
                crate::credentials::audit_read(&e, &spec, name)
            })),
        )
    };
    secrets.watch();
    let grants = crate::devices::GrantCtx::for_run(env, &built);
    end.devices = crate::devices::start(
        env,
        &mut built.spec,
        backend.name(),
        built.test_devices,
        &grants,
        &built
            .spawn
            .as_ref()
            .map(|c| c.chain.clone())
            .unwrap_or_default(),
        broker
            .clone()
            .map(|b| -> crate::gateway::Handler { std::sync::Arc::new(move |r| b.handle(&r)) }),
        secrets.clone(),
        horse_access,
    );
    crate::devices::hook_control(end.kv.as_ref(), end.devices.as_ref());
    // Terminal harnesses that log in through a browser and a localhost callback.
    let _login = if built.profile.launch.login_callback && built.spec.gui.mode != GuiMode::App {
        crate::login::prepare(env, &mut built.spec, backend.name())?
    } else {
        None
    };
    backend::start_host_proxy(env, &mut built.spec, backend.name())?;
    if backend.name() == backend::seatbelt::NAME {
        if let Err(e) = backend::seatbelt::keychain::start(env, &mut built.spec, &built.auth) {
            eprintln!("paddock: warning: no keychain for this run: {e:#}");
        }
    }
    // The shared install step: a clean sandbox without the project installs
    // registry tools into the shared mise installs. srt cannot mount the
    // shared installs, so its sandboxes install everything privately.
    let scratch = env.paddock_cache().join("provision").join(&built.spec.id);
    let shared_dir = env
        .paddock_cache()
        .join("shared")
        .join(crate::mise::SHARED_CACHE);
    let shared = if backend.name() == "srt"
        || crate::mise::all_installed(&shared_dir, &built.spec.mise_shared)
    {
        None
    } else {
        built.spec.mise_shared_spec(scratch.clone())
    };
    if let Some(p) = shared {
        crate::privfs::dir(&scratch)?;
        let what = built.spec.mise_shared.join(" ");
        let result = provision(env, backend, &p, quiet, &format!("shared tools: {what}"));
        let _ = std::fs::remove_dir_all(&scratch);
        if !result? && !quiet {
            eprintln!(
                "paddock: warning: the shared install failed; the project installs the tools in its own home"
            );
        }
    }
    // The provision step, in the project sandbox with the harness's rights.
    if let Some(p) = built.spec.provision_spec() {
        let what = built.spec.provision.join("; ");
        if !provision(env, backend, &p, quiet, &what)? && !quiet {
            eprintln!("paddock: warning: provisioning failed; the harness starts anyway");
        }
    }
    // Registered credentials (credentials.rs): after the install steps, which never get them, and
    // before the harness starts, which waits for the user's two answers.
    let creds = setup_credentials(
        env,
        &mut built,
        backend.name(),
        quiet,
        &secrets,
        broker.as_ref(),
    )?;
    start_api_proxy(env, &mut built, backend.name())?;
    mcp::prepare(env, &mut built, backend.name())?;
    let launch = backend.render(env, &built.spec, interactive)?;
    write_files(&launch)?;

    let spec = &built.spec;
    if let Some(p) = &spec.shadow {
        // The snapshot has the host data: no seed copies and no
        // json_import. The files that paddock writes below start as the
        // snapshot's copy.
        let mut up: Vec<PathBuf> = built
            .profile
            .state
            .toml_set
            .iter()
            .map(|t| PathBuf::from(&t.file))
            .collect();
        up.extend(spec.home_settings.iter().map(|h| h.file.clone()));
        up.extend(built.profile.state.done_flags.keys().map(PathBuf::from));
        for rel in up {
            p.copy_up(&rel)
                .with_context(|| format!("cannot prepare ~/{}", rel.display()))?;
        }
        // A large settings file of the snapshot does not stop the run.
        if let Err(e) = state::toml_set(&built.profile, &spec.home_src, &spec.project) {
            log::warn!("{e:#}");
        }
        match crate::privacy::write_home(spec) {
            Ok(w) => w.iter().for_each(|w| log::warn!("{w}")),
            Err(e) => log::warn!("{e:#}"),
        }
    } else {
        state::seed(env, &built.profile, &built.auth, &spec.home_src)?;
        state::toml_set(&built.profile, &spec.home_src, &spec.project)?;
        state::json_import(
            env,
            &built.profile,
            &built.auth,
            &spec.home_src,
            &spec.project,
        )?;
        for w in crate::privacy::write_home(spec)? {
            log::warn!("{w}");
        }
    }
    end.home_written(spec);
    mcp::apply(env, &built, backend.name())?;
    let auth = AuthSync::copy_in(env, &built.profile, &built.auth, &spec.home_src)?;
    // The keeper refreshes the logins that the sandbox got without their
    // refresh token (keeper.rs), as long as this process runs.
    let kept = auth.kept_logins();
    if let (Some(kind), Some(first)) = (crate::keeper::kind_for(&built.profile.name), kept.first())
    {
        crate::keeper::start(env, &built.profile.name, kind, &built.auth, &first.store);
    }
    let history = built
        .shared_history
        .then(|| crate::history::CopyBack::new(env, &built.profile, &built.auth, &spec.home_src));
    state::write_gitconfig(env, &spec.home_src, repo.as_ref())?;
    let watched = state::snapshot(&spec.watch);
    // `paddock undo`: a copy of the project files. A worktree run has its
    // own branch, and a spawned child shares the project of its parent,
    // whose snapshot covers it.
    let project_snapshot =
        built.worktree.is_none() && built.spawn.is_none() && built.parent.is_none();
    if project_snapshot {
        crate::snapshot::before_start(env, &spec.project, &spec.id, &spec.harness, quiet);
    }
    // After paddock filled the home: its own files are not news.
    if let Some(k) = &end.kv {
        k.watch_credentials(spec.home_src.clone(), built.profile.state.auth.clone());
    }
    // The bridge of the pasture: what the login guard of the other
    // members checks, outside every sandbox.
    let bridge_dir = match (&end.kv, &kv_pasture) {
        (Some(_), Some(p)) => {
            let base = crate::pasture::bridge::dir(env, &kv_project_path, p);
            let rec = crate::pasture::bridge::MemberRecord {
                harness: spec.harness.clone(),
                home: spec.home_src.clone(),
                login: built.profile.state.auth.clone(),
                stores: auth.host_logins().into_iter().map(|l| l.store).collect(),
                delivered: creds.marks.clone(),
                oneshot: built.bridge.is_some(),
            };
            match crate::pasture::bridge::record_member(&base, &spec.id, &rec) {
                Ok(()) => Some(base),
                Err(e) => {
                    log::warn!("the pasture bridge is off for this sandbox: {e:#}");
                    None
                }
            }
        }
        _ => None,
    };
    end.bridge = bridge_dir;
    end.snapshot = project_snapshot;

    crate::boundary::record_start(env, spec);
    if !quiet {
        eprintln!(
            "paddock: {} in {} (backend {}, network {}, display {})",
            spec.harness,
            spec.project.display(),
            backend.name(),
            format!("{:?}", spec.network).to_lowercase(),
            format!("{:?}", spec.gui.mode).to_lowercase()
        );
        for n in &spec.notices {
            eprintln!("paddock: {n}");
        }
        for w in &spec.warnings {
            log::warn!("{w}");
        }
    }

    // What paddock itself put into the upper folder is not news at the end.
    let shadow_base = spec.shadow.as_ref().map(|p| p.baseline());
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args).env_clear().envs(&launch.env);
    if capture {
        cmd.stdout(Stdio::piped());
    }
    let stdin = built.stdin.take();
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    // A scope of its own, so that `paddock freeze` can freeze exactly this
    // sandbox. A spawned child is in the scope of its limits already.
    let inherited = crate::freeze::own_scope();
    let new_scope = (inherited.is_none()
        && matches!(backend.name(), "bwrap" | "srt")
        && (env.var("DBUS_SESSION_BUS_ADDRESS").is_some() || env.runtime_dir.is_some()))
    .then(|| crate::freeze::scope_name(&spec.id));
    if new_scope.is_some() {
        // paddock's start stub runs first and stops; paddock moves it into
        // the scope, and it becomes the sandbox command (same process), so
        // every process of the sandbox is in there.
        let me = std::env::current_exe().context("cannot find the paddock executable")?;
        let mut stub = Command::new(me);
        stub.arg(crate::freeze::SCOPE_EXEC)
            .arg(&launch.program)
            .args(&launch.args)
            .env_clear()
            .envs(&launch.env);
        cmd = stub;
        if capture {
            cmd.stdout(Stdio::piped());
        }
        if stdin.is_some() {
            cmd.stdin(Stdio::piped());
        }
    }
    if forwarding.stop_requested() {
        bail!("paddock was asked to stop before the sandbox started");
    }
    // The training gate (training.rs) needs the harness on a paddock pty.
    let policy = crate::training::policy(&built.manifest);
    let gate = crate::training::prepare(env, &built.profile, policy, spec, caller_terminal)?;
    if gate.is_some() && !quiet {
        eprintln!("paddock: {}", crate::training::notice(&spec.harness));
    }
    let screens = crate::screens::prepare(
        env,
        &built.profile,
        spec,
        caller_terminal,
        term.unwatched,
        built.detach,
    );
    let tap = crate::relay::Taps::of(gate.clone(), screens, crate::horse::live::take_pty_tap());
    // The live view of a run without a terminal (view.rs). paddock copies
    // each stream that is not a terminal to the caller and to the run log.
    // A watched terminal stays the harness's own. An invisible terminal is
    // relayed: the harness gets a new pty, and the same bytes go to the
    // caller and to the run log. Capture mode copies both streams.
    let mut view = if relay_on {
        crate::view::Live::start_relay(env, &spec.id, &spec.harness, quiet)
    } else if built.view && !interactive {
        let (tee_out, tee_err) = crate::view::copy_streams(
            capture,
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
        );
        crate::view::Live::start(env, &spec.id, &spec.harness, tee_out, tee_err, quiet)
    } else {
        None
    };
    end.view = view.is_some();
    if let Some(l) = &view {
        if l.tee_out {
            cmd.stdout(Stdio::piped());
        }
        if l.tee_err {
            cmd.stderr(Stdio::piped());
        }
    }
    let mut relay = if relay_on || tap.is_some() {
        let log = view.as_ref().map(|l| l.tee.sink());
        Some(
            crate::relay::Relay::start(
                libc::STDIN_FILENO,
                libc::STDOUT_FILENO,
                move |data| {
                    if let Some(s) = &log {
                        s.append(data);
                    }
                },
                tap,
            )
            .context("cannot relay the terminal")?,
        )
    } else {
        None
    };
    if let Some(r) = &relay {
        let (stdin, stdout, stderr) = r.child_stdio()?;
        cmd.stdin(stdin).stdout(stdout).stderr(stderr);
    }
    let mut terminal;
    {
        use std::os::unix::process::CommandExt;
        let take_tty = relay.is_some();
        // A child sandbox ends with the paddock process that runs it, even
        // when that one is killed. SIGTERM, so a container CLI passes it on.
        let deathsig = built.spawn.is_some();
        // A Seatbelt run gets a process group of its own for `paddock
        // freeze` (setsid makes one too). It may take the terminal.
        let group = backend.name() == backend::seatbelt::NAME && !take_tty;
        terminal = crate::freeze::Terminal::note(group);
        if take_tty || deathsig || group {
            // SAFETY: setsid and ioctl are async-signal-safe, and so are the
            // group and parent-death helpers. The slave is fd 0 with `take_tty`.
            unsafe {
                cmd.pre_exec(move || {
                    if take_tty {
                        crate::relay::take_controlling_tty()?;
                    } else if group {
                        crate::freeze::own_group()?;
                    }
                    if deathsig {
                        crate::platform::arm_parent_death_signal()?;
                    }
                    Ok(())
                });
            }
        }
    }
    if horse.is_some() {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot start {}", launch.program.display()))?;
    crate::horse::live::handoff(horse, &mut child);
    if let Some(r) = relay.as_mut() {
        r.close_slave();
    }
    let unit = memory_scope::establish(env, child.id(), new_scope, inherited, spec, quiet);
    forwarding.set_child(child.id());
    if let Some(l) = view.as_mut() {
        if let Some(o) = child.stdout.take().filter(|_| l.tee_out) {
            let sink = if capture {
                crate::view::Sink::Capture
            } else {
                crate::view::Sink::Fd(1)
            };
            l.tee.copy(o, sink);
        }
        if let Some(e) = child.stderr.take().filter(|_| l.tee_err) {
            l.tee.copy(e, crate::view::Sink::Fd(2));
        }
    }
    if let (Some(data), Some(mut w)) = (stdin, child.stdin.take()) {
        // A thread, so a harness that does not read cannot block paddock.
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut w, &data);
        });
    }
    if let Some(d) = end.devices.as_mut() {
        d.after_start(env, backend.name(), &spec.id);
    }
    // macOS: the window mark of a watched run in the caller's terminal.
    let own = terminal_watched && !relay_on && built.spawn.is_none() && !built.detach;
    let own_window = crate::mark::own_terminal_window(env, own);
    let running = Running {
        id: spec.id.clone(),
        harness: spec.harness.clone(),
        project: spec.project.clone(),
        backend: backend.name().to_string(),
        network: format!("{:?}", spec.network).to_lowercase(),
        gui: format!("{:?}", spec.gui.mode).to_lowercase(),
        pid: child.id(),
        started: state::now(),
        auth: built.auth.to_string(),
        window_pid: _compositor.as_ref().map(|c| c.pid()),
        // Short lines for the bar panel, which shows the first one.
        // Only `login_ports = true` opens every port. The filtered login
        // callback (one approved port, ten minutes) is not worth a warning.
        warnings: if spec
            .warnings
            .iter()
            .any(|w| w.starts_with("login_ports is on"))
        {
            vec!["login ports open on 127.0.0.1".to_string()]
        } else {
            Vec::new()
        },
        host_pid: std::process::id(),
        parent: built.spawn.as_ref().map(|c| c.parent.clone()),
        spawn_id: built.spawn.as_ref().map(|c| c.spawn_id.clone()),
        logins: auth.host_logins(),
        kept_logins: kept.clone(),
        watched: terminal_watched,
        relay: relay_on,
        terminal_window: own_window.map(|w| w.id),
        terminal_pid: own_window.map(|w| w.pid),
        unit,
        kv_project: Some(kv_project_path),
        pasture: kv_pasture,
        credentials: creds.delivered,
        gateway: creds.gateway,
        agent: creds.agent,
        secret_files: creds.secret_files,
    };
    if let Err(e) = running.register(env) {
        log::warn!("cannot register the running sandbox: {e:#}");
    }
    // The ration of a pasture member: after the registration, so that its
    // quota counts from the start.
    if let Some(p) = &running.pasture {
        let ration = crate::pasture::load(env)
            .ok()
            .and_then(|all| all.get(p)?.rations.get(&spec.harness).cloned());
        if let Some(r) = ration {
            match crate::pasture::apply_ration(env, &spec.id, &spec.harness, &r) {
                Ok(notes) if !quiet => notes.iter().for_each(|n| eprintln!("paddock: {n}")),
                Ok(_) => {}
                Err(e) => log::warn!("cannot set the ration of {}: {e:#}", spec.harness),
            }
        }
    }
    // Kept after exit, so `paddock files` can list what the sandbox left.
    // A shadow run leaves nothing.
    let session = match spec.shadow {
        Some(_) => Ok(()),
        None => crate::recover::Session::begin(
            env,
            &spec.id,
            &spec.harness,
            &spec.project,
            &spec.home_src,
        ),
    };
    if let Err(e) = session {
        if !quiet {
            log::warn!("cannot record the session for `paddock files`: {e:#}");
        }
    }
    if !quiet {
        let home_key = built
            .worktree
            .as_ref()
            .map(|w| w.repo.clone())
            .unwrap_or_else(|| spec.project.clone());
        let _ = state::record_recent(env, &home_key, &spec.harness);
    }

    // pasta hides the host addresses that exist at start. Watch for new ones (a VPN
    // that comes up later) and report them: they stay reachable until the next start.
    let _space = crate::storage::guard::Watch::start(env, &spec.project);
    let new_addrs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let monitor = (backend.name() == "bwrap" && spec.network == Network::Egress)
        .then(|| EgressAddrMonitor::start(env.clone(), running.clone(), new_addrs.clone()));

    let live = HistorySync::start(history, HISTORY_EVERY);
    let login_sync = LoginSync::start(auth, quiet);

    // The child owns the terminal. Ctrl-C reaches it directly; paddock waits.
    let old_int = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
    let old_quit = unsafe { libc::signal(libc::SIGQUIT, libc::SIG_IGN) };
    let mut out = None;
    if capture && view.is_none() {
        out = child.stdout.take().map(read_capped);
    }
    let sandbox_id = spec.id.clone();
    let seam = crate::spawn::HostSeam(env.clone());
    let status = crate::spawn::wait_outliving_parent(
        &mut child,
        &STOP_ASKED,
        env,
        &sandbox_id,
        &seam,
        crate::spawn::STOP_GRACE,
    )?;
    terminal.take_back();
    unsafe {
        libc::signal(libc::SIGINT, old_int);
        libc::signal(libc::SIGQUIT, old_quit);
    }
    use std::os::unix::process::ExitStatusExt;
    let code = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
    end.code = Some(code);
    if let Some(r) = relay.take() {
        r.finish();
    }
    let gate_line = gate.as_ref().and_then(|g| {
        let setting = built.profile.privacy.training.as_ref();
        crate::training::summary(&g.result()?, setting.map(|t| t.setting.as_str()))
    });
    if let Some(line) = gate_line.filter(|_| !quiet) {
        eprintln!("paddock: {line}");
    }
    if let Some(l) = view.take() {
        let kept = l.finish(code);
        if capture {
            out = Some(String::from_utf8_lossy(&kept.unwrap_or_default()).into_owned());
        }
    }
    if forwarding.hung_up() {
        // The terminal is gone: a write to it would fail and end the exit
        // steps half way.
        crate::platform::quiet_stdio();
    }
    // The children of this sandbox end with it.
    if let Some(s) = &spawner {
        s.stop_all(Duration::from_secs(30));
    }
    // Dropping the monitor stops and joins its thread.
    drop(monitor);
    let late = new_addrs.lock().unwrap().clone();
    if !late.is_empty() && !quiet {
        eprintln!(
            "paddock: warning: the host got new addresses while the sandbox ran ({}). Services on them were reachable from the sandbox. The next start hides them.",
            late.join(", ")
        );
    }
    let (history, copied) = live.finish();
    let auth = login_sync.finish();
    let synced = match auth.copy_out() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("cannot copy credentials back: {e:#}");
            state::SyncResult {
                saved: vec![],
                conflicts: vec![],
            }
        }
    };
    match history.as_ref().map(|h| h.copy_out()).unwrap_or(Ok(vec![])) {
        Ok(files) if copied + files.len() > 0 => log::info!(
            "copied {} session file(s) back to the host history",
            copied + files.len()
        ),
        Ok(_) => {}
        Err(e) => log::warn!("cannot copy the session history back: {e:#}"),
    }
    if !quiet {
        for f in &synced.saved {
            eprintln!("paddock: saved updated credentials to {}", f.display());
        }
        for f in &synced.conflicts {
            eprintln!(
                "paddock: warning: {} changed on the host while the sandbox ran, so the host version was kept. Log in again if a harness asks.",
                f.display()
            );
        }
        let changed = state::changed(&watched);
        if !changed.is_empty() {
            eprintln!("paddock: warning: these files changed. Host tools run them automatically, so review them before you use them on the host:");
            for f in changed {
                eprintln!("  {}", f.display());
            }
        }
    }
    let out = crate::horse::lifecycle::end(_horse_lifecycle, code, out);
    // Verified new packages go to the shared layers; the build cache keeps the target folder.
    crate::storage::layers::promote::spawn_after_run(env, spec, backend.name());
    crate::storage::buildcache::after_run(env, spec);
    if let (Some(p), Some(base)) = (&spec.shadow, &shadow_base) {
        if !quiet {
            let skip: Vec<PathBuf> = spec
                .mounts
                .iter()
                .filter_map(|m| m.dst.strip_prefix(&spec.home).ok())
                .map(Path::to_path_buf)
                .collect();
            for l in p.end_summary(base, &skip).lines() {
                eprintln!("paddock: {l}");
            }
        }
        drop(shadow);
        return Ok(Outcome { code, stdout: out });
    }
    // Share identical file data of this home with the other homes, in the
    // background too: the exit does not wait for it.
    crate::storage::dedupe::spawn_after_run(env, &spec.home_src);
    if !quiet {
        crate::recover::notice(env, &spec.id, &spec.home_src, running.started);
    }
    Ok(Outcome { code, stdout: out })
}

pub fn select_backend(env: &HostEnv, built: &Built) -> Result<Box<dyn Backend>> {
    backend::docker::set_image_cfg(backend::docker::ImageCfg {
        image: built.manifest.docker.image.clone(),
        packages: built.manifest.docker.packages.clone(),
        gui: built.spec.gui.enabled(),
        app: built.spec.gui.mode == crate::spec::GuiMode::App,
        keyring: built.spec.keyring,
    });
    backend::select(env, &built.spec, &crate::vm::select::wanted(env, built)?)
}

/// Start the host passthrough for this sandbox and give the sandbox the
/// `paddock-host` command. See broker.rs.
/// Returns the repository snapshot that `git-push` and `forge` work from.
type BrokerStart = (
    Option<crate::broker::git::Repo>,
    Option<std::sync::Arc<crate::spawn::Spawner>>,
    Option<std::sync::Arc<crate::broker::Broker>>,
);

fn start_broker(
    env: &HostEnv,
    built: &mut Built,
    interactive: bool,
    backend: &str,
) -> Result<BrokerStart> {
    let run_dir = env.paddock_run().join(&built.spec.id);
    crate::privfs::dir(&run_dir)?;
    let sock = crate::sockpath::named(env, &built.spec.id, "host.sock")?;
    let mut policy = crate::broker::HostPolicy::load(env)?;
    // A child never gets a mode that its parent did not have.
    if let Some(c) = &built.spawn {
        policy.modes = crate::spawn::child_modes(&policy.modes, &c.modes);
        if c.role == crate::spawn::Role::Reviewer {
            policy
                .modes
                .insert("git-push".into(), crate::broker::Mode::Deny);
        }
    }
    let human: std::sync::Arc<dyn crate::broker::Approver> =
        std::sync::Arc::new(crate::broker::HostApprover);
    let approver: std::sync::Arc<dyn crate::broker::Approver> = match built.approver.clone() {
        Some(a) => a,
        None => match crate::approver::resolve(env, interactive) {
            Ok(Some((hook, source))) => {
                eprintln!(
                    "paddock: the agent approver answers host requests for: {}",
                    hook.capabilities.join(", ")
                );
                std::sync::Arc::new(crate::approver::Chain {
                    hook,
                    source,
                    human,
                })
            }
            Ok(None) => human,
            Err(e) => {
                eprintln!("paddock: the agent approver is off for this run: {e:#}");
                human
            }
        },
    };
    // Taken before the sandbox runs: only these remotes count as known.
    // The sandbox sees .git/config read-only, but it can name other remotes
    // with `git -c` or its own ~/.gitconfig.
    let repo = crate::broker::git::Repo::snapshot(&built.spec.project);
    let ctx = crate::broker::Context {
        id: built.spec.id.clone(),
        harness: built.spec.harness.clone(),
        project: built.spec.project.clone(),
        repo: repo.clone(),
        chain: built
            .spawn
            .as_ref()
            .map(|c| c.chain.clone())
            .unwrap_or_default(),
        depth: built.spawn.as_ref().map_or(0, |c| c.depth),
        role: built.spawn.as_ref().map(|c| c.role),
    };
    // Children of this sandbox (`paddock-host spawn`). A bad [spawn] table
    // turns spawn off for this run; the rest of the broker still works.
    let spawner = match crate::spawn::SpawnCfg::load(env) {
        Ok(cfg) => {
            let mut ceiling = crate::spawn::Ceiling::of(built, backend, &policy.modes);
            crate::trainer::sandbox::capture_ceiling(env, built, &mut ceiling);
            ceiling.devices = crate::devices::names_for(
                env,
                built.test_devices,
                &crate::devices::GrantCtx::for_run(env, built),
            );
            match crate::spawn::Spawner::new(env, cfg, ceiling, &built.spec.project) {
                Ok(mut s) => {
                    // The card at a child's time limit waits as long as
                    // any other approval.
                    s.approval_wait = std::time::Duration::from_secs(policy.wait);
                    Some(std::sync::Arc::new(s))
                }
                Err(e) => {
                    log::warn!("spawn is off for this run: {e:#}");
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("paddock: spawn is off for this run: {e:#}");
            None
        }
    };
    let mut broker = crate::broker::Broker::new(env.clone(), ctx, policy, approver);
    broker.spawner = spawner.clone();
    broker.run = crate::broker::RunFacts {
        spawn_id: built.spawn.as_ref().map(|c| c.spawn_id.clone()),
        pasture: built.pasture.clone(),
        // A spawn child and a detached run get their task as this file.
        // Seatbelt cannot move the sandbox home to the host home's path.
        task_file: (built.spawn.is_some() || built.detach)
            .then_some(match backend {
                backend::seatbelt::NAME => &built.spec.home_src,
                _ => &built.spec.home,
            })
            .map(|h| h.join(crate::spawn::prompt_rel(&built.spec.id))),
    };
    broker.borrow_dir = backend::seatbelt::run::borrow_seen(backend, &built.spec);
    if built.detach {
        let ctl = std::sync::Arc::new(crate::detach::Ctl::new(env.clone(), built.spec.id.clone()));
        ctl.watch(built.profile.screens.error.clone());
        broker.detach = Some(ctl);
    }
    // One broker for the socket and the gateway device: the same policy,
    // session grants, approvals and audit.
    let broker = std::sync::Arc::new(broker);
    crate::broker::serve(&sock, broker.clone())?;
    built.spec.host_socket = Some(sock);
    host_tools::add(env, built, &run_dir)?;
    Ok((repo, spawner, Some(broker)))
}

/// Run one provision sandbox to the end. Its output goes to stderr. Returns
/// whether it succeeded.
fn provision(
    env: &HostEnv,
    backend: &dyn Backend,
    spec: &SandboxSpec,
    quiet: bool,
    what: &str,
) -> Result<bool> {
    let l = backend.render(env, spec, false)?;
    write_files(&l)?;
    if !quiet {
        eprintln!("paddock: provisioning: {what}");
    }
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args)
        .env_clear()
        .envs(&l.env)
        .stdin(Stdio::null());
    if quiet {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::from(std::io::stderr()));
    }
    let status = cmd
        .status()
        .with_context(|| format!("cannot start {}", l.program.display()))?;
    Ok(status.success())
}

/// How often a running sandbox and the host bring its login files in line.
const LOGIN_EVERY: Duration = Duration::from_secs(2);

/// Keeps the login files of a running sandbox and the host in line, both
/// ways (`AuthSync::sync_now`), so a refreshed token reaches the host and
/// every other sandbox on the same login within seconds, not at the end.
struct LoginSync {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<AuthSync>>,
}

impl LoginSync {
    fn start(mut auth: AuthSync, quiet: bool) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            while !s.load(std::sync::atomic::Ordering::SeqCst) {
                for step in auth.sync_now() {
                    match step {
                        SyncStep::ToStore(f) if !quiet => {
                            log::info!("saved a refreshed login to {}", f.display())
                        }
                        SyncStep::ToSandbox(f) if !quiet => {
                            log::info!("the sandbox got the newer login of {}", f.display())
                        }
                        _ => {}
                    }
                }
                let start = std::time::Instant::now();
                while start.elapsed() < LOGIN_EVERY && !s.load(std::sync::atomic::Ordering::SeqCst)
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            // One last look before the copy back at the end.
            auth.sync_now();
            auth
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> AuthSync {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread
            .take()
            .expect("started")
            .join()
            .expect("login sync thread")
    }
}

/// History copied to the host while the sandbox runs, so the Omarchy agents
/// panel counts new sessions before the sandbox exits. Logins are not synced
/// here: a login goes back to its store only at exit.
struct HistorySync {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<(Option<crate::history::CopyBack>, usize)>,
}

/// How often history is copied.
const HISTORY_EVERY: Duration = Duration::from_secs(300);

impl HistorySync {
    fn start(history: Option<crate::history::CopyBack>, every: Duration) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut copied = 0;
            let mut at = Instant::now();
            while history.is_some() && !flag.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
                if at.elapsed() < every {
                    continue;
                }
                at = Instant::now();
                if let Some(h) = &history {
                    match h.copy_out() {
                        Ok(files) => copied += files.len(),
                        Err(e) => log::debug!("cannot copy the session history: {e:#}"),
                    }
                }
            }
            (history, copied)
        });
        Self { stop, thread }
    }

    /// Stop the thread and return the history and the files it copied.
    fn finish(self) -> (Option<crate::history::CopyBack>, usize) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread
            .join()
            .expect("the history thread does not panic")
    }
}

/// The sandbox process that SIGTERM and SIGHUP are passed to (0 before it
/// starts), whether a stop came before it started, and whether a SIGHUP came.
static FORWARD_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static STOP_BEFORE_START: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A stop arrived after the sandbox started. The wait loop reads it and
/// thaws a frozen sandbox. The signal handler cannot call the freezer.
static STOP_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static HUNG_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn forward_signal(sig: libc::c_int) {
    use std::sync::atomic::Ordering;
    if sig == libc::SIGHUP {
        HUNG_UP.store(true, Ordering::SeqCst);
    }
    STOP_ASKED.store(true, Ordering::SeqCst);
    let pid = FORWARD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        // SAFETY: kill(2) is async-signal-safe.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    } else {
        STOP_BEFORE_START.store(true, Ordering::SeqCst);
    }
}

/// The handlers for SIGTERM and SIGHUP during one run. Dropping it puts the
/// previous handlers back.
struct Forward {
    old: Vec<(libc::c_int, libc::sigaction)>,
}

impl Forward {
    fn install() -> Self {
        use std::sync::atomic::Ordering;
        HUNG_UP.store(false, Ordering::SeqCst);
        STOP_BEFORE_START.store(false, Ordering::SeqCst);
        STOP_ASKED.store(false, Ordering::SeqCst);
        FORWARD_PID.store(0, Ordering::SeqCst);
        let mut old = Vec::new();
        for sig in [libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: a plain sigaction(2) with a handler that only calls
            // async-signal-safe functions.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = forward_signal as extern "C" fn(libc::c_int) as usize;
                sa.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                let mut prev: libc::sigaction = std::mem::zeroed();
                libc::sigaction(sig, &sa, &mut prev);
                old.push((sig, prev));
            }
        }
        Self { old }
    }

    /// Pass later signals to the sandbox. A stop that came before is passed
    /// on at once.
    fn set_child(&self, pid: u32) {
        use std::sync::atomic::Ordering;
        FORWARD_PID.store(pid as i32, Ordering::SeqCst);
        if STOP_BEFORE_START.swap(false, Ordering::SeqCst) {
            // SAFETY: a plain kill(2) on the sandbox process.
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }

    fn stop_requested(&self) -> bool {
        STOP_BEFORE_START.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn hung_up(&self) -> bool {
        HUNG_UP.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        for (sig, prev) in &self.old {
            // SAFETY: restores the handler that `install` saved.
            unsafe { libc::sigaction(*sig, prev, std::ptr::null_mut()) };
        }
        FORWARD_PID.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Watches for new host network addresses that become reachable while a sandbox
/// runs under bwrap with network mode egress. Joined before exit.
struct EgressAddrMonitor {
    stop_tx: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EgressAddrMonitor {
    fn start(
        env: HostEnv,
        running: crate::state::Running,
        found: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Self {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let start_addrs = crate::proxy::local_addrs();
        let thread = std::thread::spawn(move || {
            let mut info = running;
            let check = |info: &mut crate::state::Running| {
                let mut f = found.lock().unwrap();
                let mut changed = false;
                for a in crate::proxy::local_addrs() {
                    let s = a.to_string();
                    if !a.is_loopback() && !start_addrs.contains(&a) && !f.contains(&s) {
                        f.push(s);
                        changed = true;
                    }
                }
                if changed {
                    info.warnings
                        .retain(|w| !w.starts_with("new host addresses"));
                    info.warnings.insert(
                        0,
                        format!(
                            "new host addresses since start are reachable: {}",
                            f.join(", ")
                        ),
                    );
                    let _ = info.register(&env);
                }
            };
            while stop_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .is_err()
            {
                check(&mut info);
            }
            check(&mut info);
        });
        Self {
            stop_tx,
            thread: Some(thread),
        }
    }
}

impl Drop for EgressAddrMonitor {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests;
