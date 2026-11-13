//! `paddock gateway NAME start` and `stop`: hand the supervisor
//! (serve.rs) to the user manager.
//!
//! - Linux with a systemd user manager: a transient unit
//!   `paddock-gateway-NAME` of `systemd-run --user`. systemd starts the
//!   supervisor again when it dies by a signal (`Restart=on-abnormal`).
//! - macOS: a LaunchAgent `com.harnessix.paddock.gateway.NAME` that runs at
//!   load and again after a crash. `stop` unloads it and deletes the file.
//! - Without either: a detached paddock process in a process group of its
//!   own. It runs until `stop` or the end of the session.
//!
//! paddock never runs `openclaw daemon` or another installer of a host
//! service: the unit runs the paddock supervisor, and the supervisor runs
//! the gateway in a sandbox.

use super::launch;
use super::state::{self, Phase, StartCfg, State};
use crate::env::HostEnv;
use crate::profile::HarnessFactory;
use crate::spec::Network;
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Test only: act as if this host had no user manager.
const NO_MANAGER_VAR: &str = "PADDOCK_TEST_NO_USER_MANAGER";

/// What the user chose for a start.
#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    pub network: Option<Network>,
    pub backend: Option<String>,
    /// The port to use. None: [`choose_port`].
    pub port: Option<u16>,
}

/// The transient unit of gateway `name`.
pub fn unit_name(name: &str) -> String {
    format!("paddock-gateway-{name}")
}

/// The label of the LaunchAgent of gateway `name`.
pub fn label(name: &str) -> String {
    format!("com.harnessix.paddock.gateway.{name}")
}

fn remove_cmd(name: &str) -> String {
    format!("paddock gateway {name} stop")
}

/// How this host runs the supervisor.
/// The test switch holds on macOS too: a LaunchAgent has one label for
/// all the tests that run at the same time, and it runs in the real
/// launchd domain of the user.
pub fn how(env: &HostEnv) -> &'static str {
    if env.var(NO_MANAGER_VAR).is_some() {
        "detached"
    } else if cfg!(target_os = "macos") {
        "launch agent"
    } else if env.var("DBUS_SESSION_BUS_ADDRESS").is_some() && env.which("systemd-run").is_some() {
        "systemd-run"
    } else {
        "detached"
    }
}

/// The port of a new start: the port of the last start, else the port of
/// the profile, else one that the system gives. A port in use is skipped.
pub fn choose_port(last: Option<u16>, profile: u16) -> Result<u16> {
    for p in last.into_iter().chain([profile]) {
        if launch::free(p) {
            return Ok(p);
        }
    }
    launch::any_free()
}

fn paddock_exe(env: &HostEnv) -> PathBuf {
    crate::launchers::launcher_exe(env)
        .or_else(|_| std::env::current_exe())
        .unwrap_or_else(|_| PathBuf::from("paddock"))
}

/// The variables of the supervisor: the folders of the user, so it reads
/// the same configuration, and the test switches.
fn serve_env(env: &HostEnv) -> Vec<(String, String)> {
    let mut v = crate::units::xdg_env(env);
    if let Some(rt) = &env.runtime_dir {
        v.push(("XDG_RUNTIME_DIR".into(), rt.display().to_string()));
    }
    for k in [NO_MANAGER_VAR, launch::LAUNCHER_VAR] {
        if let Some(val) = env.var(k) {
            v.push((k.to_string(), val.to_string()));
        }
    }
    v
}

/// The LaunchAgent of the supervisor: it runs at load, and again after a
/// crash. Its output goes to the gateway log.
pub fn plist(env: &HostEnv, name: &str, paddock: &std::path::Path) -> String {
    use crate::units::xml;
    let x = |p: &std::path::Path| xml(&p.display().to_string());
    let more: String = serve_env(env)
        .iter()
        .map(|(k, v)| {
            format!(
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                xml(k),
                xml(v)
            )
        })
        .collect();
    let log = state::log_file(env, name);
    format!(
        "{}\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\t<string>{}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n\
\t\t<string>{}</string>\n\t\t<string>gateway-serve</string>\n\t\t<string>{}</string>\n\
\t</array>\n\
\t<key>RunAtLoad</key>\n\t<true/>\n\
\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>Crashed</key>\n\t\t<true/>\n\t</dict>\n\
\t<key>ProcessType</key>\n\t<string>Background</string>\n\
\t<key>StandardOutPath</key>\n\t<string>{}</string>\n\
\t<key>StandardErrorPath</key>\n\t<string>{}</string>\n\
\t<key>EnvironmentVariables</key>\n\t<dict>\n\
\t\t<key>HOME</key>\n\t\t<string>{}</string>\n\
\t\t<key>PATH</key>\n\t\t<string>{}</string>\n\
{more}\t</dict>\n\
</dict>\n\
</plist>\n",
        crate::units::plist_mark(&remove_cmd(name)),
        xml(&label(name)),
        x(paddock),
        xml(name),
        x(&log),
        x(&log),
        x(&env.home),
        crate::units::LAUNCHD_PATH,
    )
}

/// The `systemd-run` command line of the supervisor.
pub fn systemd_run_args(env: &HostEnv, name: &str, paddock: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "--user".into(),
        "--quiet".into(),
        "--collect".into(),
        format!("--unit={}", unit_name(name)),
        format!("--description=paddock: the sandboxed {name} gateway"),
        "--property=Restart=on-abnormal".into(),
        "--property=RestartSec=2".into(),
        "--property=KillMode=mixed".into(),
    ];
    for (k, val) in serve_env(env) {
        v.push(format!("--setenv={k}={val}"));
    }
    v.push("--".into());
    v.push(paddock.display().to_string());
    v.push("gateway-serve".into());
    v.push(name.to_string());
    v
}

fn run_quiet(program: &str, args: &[String]) -> Result<()> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("cannot run {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} failed with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Write the LaunchAgent of the supervisor and load it. launchd starts it
/// at once (`RunAtLoad`), at each login, and again after a crash.
#[cfg(target_os = "macos")]
fn load_agent(env: &HostEnv, name: &str, paddock: &std::path::Path) -> Result<()> {
    let file = crate::units::plist_file(env, &label(name));
    crate::units::write_ours(&[(file.clone(), plist(env, name, paddock))])?;
    let domain = crate::units::launchd_domain(env.uid);
    let service = format!("{domain}/{}", label(name));
    let _ = crate::units::launchctl(&["bootout", &service]);
    // A bootout ends the job a moment later; a bootstrap before that fails.
    let path = file.display().to_string();
    let mut last = None;
    for _ in 0..20 {
        match crate::units::launchctl(&["bootstrap", &domain, &path]) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("launchctl bootstrap failed")))
}

#[cfg(not(target_os = "macos"))]
fn load_agent(_env: &HostEnv, _name: &str, _paddock: &std::path::Path) -> Result<()> {
    bail!("a LaunchAgent is for macOS")
}

/// Hand the supervisor to the user manager, or start it detached.
fn launch_supervisor(env: &HostEnv, name: &str, how: &str) -> Result<()> {
    let paddock = paddock_exe(env);
    match how {
        "systemd-run" => {
            let unit = format!("{}.service", unit_name(name));
            // A unit of an earlier start that failed keeps its name.
            let _ = run_quiet("systemctl", &["--user".into(), "reset-failed".into(), unit]);
            run_quiet("systemd-run", &systemd_run_args(env, name, &paddock))
        }
        "launch agent" => load_agent(env, name, &paddock),
        _ => {
            use std::os::unix::process::CommandExt as _;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(state::log_file(env, name))?;
            let mut child = std::process::Command::new(&paddock)
                .args(["gateway-serve", name])
                .env_clear()
                .envs(&env.vars)
                .stdin(std::process::Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .process_group(0)
                .spawn()
                .with_context(|| format!("cannot start {}", paddock.display()))?;
            // A command that lives on (a client that started the gateway)
            // reaps the supervisor when it ends, so no zombie stays behind.
            let _ = std::thread::Builder::new()
                .name("gateway-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
            Ok(())
        }
    }
}

/// The last lines of the gateway log, for an error.
pub fn log_tail(env: &HostEnv, name: &str, n: usize) -> String {
    let text = std::fs::read_to_string(state::log_file(env, name)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `paddock gateway NAME start`. Returns the state once the gateway
/// answers on its port.
pub fn start(env: &HostEnv, name: &str, opts: &StartOpts) -> Result<State> {
    crate::update::refuse_in_sandbox(env, &format!("paddock gateway {name} start"))?;
    let factory = HarnessFactory::new(env)?;
    let harness = factory.create(name)?;
    let shape = super::shape_of(&harness.profile)?;
    if let Some(st) = state::live(env, name) {
        if st.phase != Phase::Failed {
            return Ok(st);
        }
        // A failed supervisor exits; wait for it.
        stop(env, name)?;
    }
    let net = super::spec::network(env, opts.network)?;
    let token = state::token(env, name)?;
    let last = state::read_start(env, name).map(|c| c.port);
    let port = match opts.port {
        Some(p) if launch::free(p) => p,
        Some(p) => bail!("port {p} of 127.0.0.1 is in use: name another one, or leave --port out"),
        None => choose_port(last, shape.port)?,
    };
    let mut secrets = BTreeMap::new();
    for k in super::spec::secret_names(env, &harness.profile)? {
        if let Some(v) = env.var(&k) {
            secrets.insert(k, v.to_string());
        }
    }
    // Refuse here, in the terminal, what the supervisor would refuse in a
    // log: a spec that the policy refuses, or a host without a backend.
    let spec = super::spec::gateway(env, &harness, &shape, net, port, &token, &secrets)?;
    let backend = opts.backend.clone().unwrap_or_else(|| "auto".into());
    launch::check(env, &spec, &backend)?;
    let how = how(env);
    let cfg = StartCfg {
        port,
        network: format!("{net:?}").to_lowercase(),
        backend,
        how: how.to_string(),
        unit: match how {
            "systemd-run" => Some(unit_name(name)),
            "launch agent" => Some(label(name)),
            _ => None,
        },
    };
    state::write_start(env, name, &cfg)?;
    state::write_secrets(env, name, &secrets)?;
    state::remove(env, name);
    launch_supervisor(env, name, how)?;
    let until = Instant::now() + super::serve::READY + Duration::from_secs(5);
    loop {
        if let Some(st) = state::read(env, name) {
            match st.phase {
                Phase::Running if crate::state::alive(st.pid) => return Ok(st),
                Phase::Failed => bail!(
                    "the {name} gateway did not start: {}\n{}",
                    st.reason.unwrap_or_default(),
                    log_tail(env, name, 20)
                ),
                _ => {}
            }
        }
        if Instant::now() >= until {
            bail!(
                "the {name} gateway did not answer on port {port} within {} s. The log:\n{}",
                super::serve::READY.as_secs(),
                log_tail(env, name, 20)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `paddock gateway NAME stop`. True when a gateway ran.
pub fn stop(env: &HostEnv, name: &str) -> Result<bool> {
    let st = state::live(env, name);
    let cfg = state::read_start(env, name);
    match cfg.as_ref().map(|c| c.how.as_str()) {
        Some("systemd-run") => {
            let u = format!("{}.service", unit_name(name));
            let _ = run_quiet("systemctl", &["--user".into(), "stop".into(), u.clone()]);
            let _ = run_quiet("systemctl", &["--user".into(), "reset-failed".into(), u]);
        }
        #[cfg(target_os = "macos")]
        Some("launch agent") => {
            let file = crate::units::plist_file(env, &label(name));
            let service = format!("{}/{}", crate::units::launchd_domain(env.uid), label(name));
            let _ = crate::units::launchctl(&["bootout", &service]);
            if crate::units::ours(&file) {
                let _ = std::fs::remove_file(&file);
            }
        }
        _ => {}
    }
    if let Some(st) = &st {
        // SAFETY: a signal to the supervisor that the state file names.
        unsafe {
            libc::kill(st.pid as i32, libc::SIGTERM);
        }
        let until = Instant::now() + Duration::from_secs(10);
        while crate::state::alive(st.pid) {
            if Instant::now() >= until {
                bail!(
                    "the supervisor of the {name} gateway (pid {}) did not end within 10 s",
                    st.pid
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    state::remove(env, name);
    state::remove_secrets(env, name);
    Ok(st.is_some())
}
