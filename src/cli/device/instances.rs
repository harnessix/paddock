//! `paddock device check|logs|stop|restart|stats`: the instances of a
//! device. `check` starts one in this process, without a run: a device
//! sandbox, or the supervisor on the host for a host device, after its
//! warning line. `stop` and
//! `restart` ask the paddock process of the run over its kv socket (the
//! `devices` op), so the owner of the instance does the work. `logs` and
//! `stats` read files: the log of the instance and the audit log.

use anyhow::{bail, Context as _, Result};
use paddock::devices::config::Run;
use paddock::devices::host::HostSource;
use paddock::devices::sandbox::state::{self, InstanceState};
use paddock::devices::sandbox::{Instance, InstanceCtx, Launcher};
use paddock::devices::DevicesCfg;
use paddock::env::HostEnv;
use std::io::{BufRead, BufReader, Read, Write};
use std::time::{Duration, Instant};

fn def_of(env: &HostEnv, name: &str) -> Result<paddock::devices::DeviceDef> {
    let cfg = DevicesCfg::load(env)?;
    cfg.user
        .iter()
        .find(|u| u.name == name)
        .cloned()
        .with_context(|| {
            format!(
                "no [devices.{name}] table in {}. `paddock device ls` lists the devices",
                env.paddock_config().join("config.toml").display()
            )
        })
}

/// `paddock device check NAME` of a shared device: the instance of the
/// user, through `paddock deviced`, with one lease held for the check.
fn check_shared(
    env: &HostEnv,
    def: &paddock::devices::DeviceDef,
    send: Option<&[u8]>,
    hold: Option<u64>,
) -> Result<String> {
    use paddock::devices::program::{Caller, Collect, Program};
    use paddock::devices::shared::{self, SharedSource};
    let name = &def.name;
    let caller = format!("check-{}-{}", paddock::state::now(), std::process::id());
    let ctx = check_ctx_for(env, &caller);
    let src = SharedSource::new(def.clone(), &ctx);
    let mut out = String::new();
    let t = Instant::now();
    let how = shared::start_kind(env, name);
    let socket = src
        .ensure_leased()
        .map_err(|e| anyhow::anyhow!("the shared instance of {name}: {e}"))?;
    let state = paddock::devices::sandbox::state::InstanceState::shared_of(env, name);
    out.push_str(&format!(
        "device {name}: shared instance ready in {} ms ({}), socket {}, pid {}, users {}\n",
        t.elapsed().as_millis(),
        how.name(),
        socket.display(),
        state.as_ref().map(|s| s.pid).unwrap_or(0),
        src.users()
    ));
    let p = Program::connect(&socket)?;
    let probe = Instant::now();
    p.health(Duration::from_secs(def.ready))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    out.push_str(&format!(
        "health answered in {} ms\n",
        probe.elapsed().as_millis()
    ));
    match p.catalog() {
        Some(c) => out.push_str(&format!(
            "catalog: {}\n",
            serde_json::to_string(&c).unwrap_or_default()
        )),
        None => {
            out.push_str("catalog: none (the program answers no GET /v1/catalog, which is fine)\n")
        }
    }
    if let Some(req) = send {
        let p = p.with_caller(Caller::for_device(def, &ctx));
        let call = paddock::devices::Call::with_cancel(
            &caller,
            Duration::from_secs(def.timeout() + 5),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let mut sink = Collect::default();
        let t = Instant::now();
        match p.call(&call, def, req, &mut sink) {
            Ok(o) => out.push_str(&format!(
                "request: {} bytes in, {} bytes out, status {}, http {}, {} ms\nresponse:\n{}\n",
                req.len(),
                sink.0.len(),
                o.status,
                o.http,
                t.elapsed().as_millis(),
                String::from_utf8_lossy(&sink.0)
            )),
            Err(f) => out.push_str(&format!(
                "request: {} bytes in, failed: {} ({:?})\n",
                req.len(),
                f.status,
                f.error
            )),
        }
    }
    if let Some(secs) = hold {
        out.push_str(&format!("holding the lease for {secs} s\n"));
        print!("{out}");
        out.clear();
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(secs));
        out.push_str(&format!("held, users {}\n", src.users()));
    }
    let users = src.users();
    src.release();
    out.push_str(&format!(
        "dropped the lease after {} ms in all ({} other user{} left). The instance stops {} s after its last user and its last request. Log: {}\n",
        t.elapsed().as_millis(),
        users.saturating_sub(1),
        if users.saturating_sub(1) == 1 { "" } else { "s" },
        def.idle,
        src.log().display()
    ));
    Ok(out)
}

/// `paddock device check NAME [--sandbox ID] [--send REQUEST] [--hold N]`.
pub(crate) fn check(
    env: &HostEnv,
    name: &str,
    sandbox: Option<&str>,
    send: Option<&[u8]>,
    hold: Option<u64>,
) -> Result<String> {
    let def = def_of(env, name)?;
    if def.scope == paddock::devices::config::Scope::Shared {
        if let Some(id) = sandbox {
            bail!(
                "the device {name} has scope = \"shared\": one instance serves every sandbox, so there is none for {id}. Run `paddock device check {name}` without --sandbox"
            );
        }
        if let Some(why) = def.not_yet() {
            bail!("{why}");
        }
        return check_shared(env, &def, send, hold);
    }
    let host = match &def.run {
        Run::Sandbox => false,
        Run::Host { .. } => true,
        Run::Remote { .. } => {
            if sandbox.is_some() {
                bail!(
                    "[devices.{name}]: run = \"remote\" has no instance here; the paddock on the other side runs it. Run `paddock device check {name}` without --sandbox"
                );
            }
            if send.is_some() {
                bail!(
                    "[devices.{name}]: --send is not for a remote device: a request from the host would run `ssh` as you and show the card on the other side. Send one from a sandbox instead"
                );
            }
            return paddock::devices::forward::probe(env, &def);
        }
        run => bail!(
            "[devices.{name}]: run = \"{}\" has no instance to check",
            run.name()
        ),
    };
    if let Some(why) = def.not_yet() {
        bail!("{why}");
    }
    let (caller, project, harness) = match sandbox {
        Some(id) => {
            let id = paddock::ids::resolve(env, id)?;
            let r = paddock::state::Running::list(env)
                .into_iter()
                .find(|r| r.id == id)
                .with_context(|| format!("no running sandbox with id {id}"))?;
            (r.id, r.project, r.harness)
        }
        None => (
            format!("check-{}-{}", paddock::state::now(), std::process::id()),
            std::env::current_dir().unwrap_or_else(|_| env.home.clone()),
            "check".to_string(),
        ),
    };
    let ctx = InstanceCtx {
        dir: state::instance_dir(env, name, &caller),
        caller: caller.clone(),
        project,
        harness,
        chain: Vec::new(),
    };
    let mut out = String::new();
    let t = Instant::now();
    let mut inst = if host {
        // The warning line before anything starts.
        out.push_str(&paddock::devices::host::warning(&def));
        out.push('\n');
        let binary = std::env::current_exe().context("cannot find the paddock executable")?;
        let src = HostSource::with_binary(def.clone(), env, &caller, binary);
        let cwd = src.cwd();
        if !cwd.is_dir() {
            bail!(
                "cwd {} does not exist; the first request of {name} would fail with status: error",
                paddock::devices::grant::tilde(&cwd, &env.home)
            );
        }
        Started::Host(Box::new(src))
    } else {
        Started::Sandbox(Box::new(Instance::start(env, &def, &ctx, launcher(env))?))
    };
    let (started, socket) = inst.start()?;
    let p = paddock::devices::program::Program::connect(&socket)?;
    let probe = Instant::now();
    p.health(Duration::from_secs(def.ready))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let catalog = p.catalog();
    let probe = probe.elapsed();
    out.push_str(&format!(
        "device {name}: started{} in {} ms (health answered), probe {} ms, pid {}{}\n",
        if host { " on the host" } else { "" },
        started.as_millis(),
        probe.as_millis(),
        inst.pid(),
        inst.scope()
            .map(|s| format!(", scope {s}"))
            .unwrap_or_default()
    ));
    match catalog {
        Some(c) => out.push_str(&format!(
            "catalog: {}\n",
            serde_json::to_string(&c).unwrap_or_default()
        )),
        None => {
            out.push_str("catalog: none (the program answers no GET /v1/catalog, which is fine)\n")
        }
    }
    if let Some(req) = send {
        use paddock::devices::program::{Caller, Collect};
        let mut c = Caller::for_device(&def, &check_ctx(env, &ctx));
        // The thread launcher moves no path: the program reads the project
        // where it is, as the Seatbelt launcher does.
        if launcher(env) == Launcher::Thread && c.mount.is_some() {
            c.mount = Some(ctx.project.display().to_string());
        }
        let p = p.with_caller(c);
        let call = paddock::devices::Call::with_cancel(
            &caller,
            Duration::from_secs(def.timeout() + 5),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let mut sink = Collect::default();
        let t = Instant::now();
        match p.call(&call, &def, req, &mut sink) {
            Ok(o) => out.push_str(&format!(
                "request: {} bytes in, {} bytes out, status {}, http {}, {} ms\nresponse:\n{}\n",
                req.len(),
                sink.0.len(),
                o.status,
                o.http,
                t.elapsed().as_millis(),
                String::from_utf8_lossy(&sink.0)
            )),
            Err(f) => out.push_str(&format!(
                "request: {} bytes in, failed: {} ({:?})\n",
                req.len(),
                f.status,
                f.error
            )),
        }
    }
    if let Some(secs) = hold {
        out.push_str(&format!("holding the instance for {secs} s\n"));
        print!("{out}");
        out.clear();
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(secs));
    }
    let log = inst.log();
    inst.stop();
    out.push_str(&format!(
        "stopped after {} ms in all. Log: {}\n",
        t.elapsed().as_millis(),
        log.display()
    ));
    Ok(out)
}

/// The launcher of the device sandbox of a check: the product one, or a
/// thread with no isolation when the test variable asks for it, as
/// `deviced` and `serve-remote` read it.
fn launcher(env: &HostEnv) -> Launcher {
    match env.var(paddock::devices::shared::LAUNCHER_VAR) {
        Some("thread") => Launcher::Thread,
        _ => Launcher::host(),
    }
}

/// What `check` started: a device sandbox, or the supervisor on the host.
enum Started {
    Sandbox(Box<Instance>),
    Host(Box<HostSource>),
}

impl Started {
    /// Start it (the sandbox instance runs already): the time the start
    /// took and the socket.
    fn start(&mut self) -> Result<(Duration, std::path::PathBuf)> {
        match self {
            Started::Sandbox(i) => Ok((i.start_time, i.socket.clone())),
            Started::Host(h) => h.ensure_started().map_err(|e| anyhow::anyhow!("{e}")),
        }
    }

    fn pid(&self) -> u32 {
        match self {
            Started::Sandbox(i) => i.pid,
            Started::Host(h) => h.pid().unwrap_or(0),
        }
    }

    fn scope(&self) -> Option<&str> {
        match self {
            Started::Sandbox(i) => i.scope.as_deref(),
            Started::Host(_) => None,
        }
    }

    fn log(&self) -> std::path::PathBuf {
        match self {
            Started::Sandbox(i) => i.log.clone(),
            Started::Host(h) => h.log(),
        }
    }

    fn stop(&mut self) {
        match self {
            Started::Sandbox(i) => i.stop(),
            Started::Host(h) => {
                h.stop();
            }
        }
    }
}

/// A `DeviceCtx` for the headers of a check request: no approver is asked.
fn check_ctx(env: &HostEnv, ctx: &InstanceCtx) -> paddock::devices::DeviceCtx {
    paddock::devices::DeviceCtx {
        env: env.clone(),
        sandbox: ctx.caller.clone(),
        harness: ctx.harness.clone(),
        project: ctx.project.clone(),
        started_by: Vec::new(),
        approver: std::sync::Arc::new(paddock::broker::DenyAll),
        instances: paddock::devices::sandbox::Instances::new(env.clone()),
    }
}

/// A `DeviceCtx` of a check that has no instance folder of its own (a
/// shared device): the current folder is the project.
fn check_ctx_for(env: &HostEnv, caller: &str) -> paddock::devices::DeviceCtx {
    paddock::devices::DeviceCtx {
        env: env.clone(),
        sandbox: caller.to_string(),
        harness: "check".to_string(),
        project: std::env::current_dir().unwrap_or_else(|_| env.home.clone()),
        started_by: Vec::new(),
        approver: std::sync::Arc::new(paddock::broker::DenyAll),
        instances: paddock::devices::sandbox::Instances::new(env.clone()),
    }
}

/// The instance of `name` that `sandbox` names, or the one instance when
/// there is exactly one, else an error that lists them.
fn pick<'a>(
    env: &HostEnv,
    name: &str,
    sandbox: Option<&str>,
    all: &'a [InstanceState],
) -> Result<Vec<&'a InstanceState>> {
    let mine: Vec<&InstanceState> = all.iter().filter(|s| s.device == name).collect();
    match sandbox {
        Some(id) => {
            let id = paddock::ids::resolve(env, id)?;
            let one = mine
                .into_iter()
                .find(|s| s.caller == id)
                .with_context(|| format!("the device {name} has no instance for {id}"))?;
            Ok(vec![one])
        }
        None if mine.len() == 1 => Ok(mine),
        None if mine.is_empty() => bail!("the device {name} has no instance"),
        None => bail!(
            "the device {name} has {} instances; name one with --sandbox ID: {}",
            mine.len(),
            mine.iter()
                .map(|s| paddock::ids::short(&s.caller))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `paddock device logs NAME [--sandbox ID] [-f]`.
pub(crate) fn logs(env: &HostEnv, name: &str, sandbox: Option<&str>, follow: bool) -> Result<i32> {
    def_of(env, name)?;
    // Logs stay after an instance ends, so list the log files, not the
    // state files.
    let dir = state::log_dir(env, name);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .collect();
    files.sort();
    let path = match sandbox {
        Some(id) => {
            let id = paddock::ids::resolve(env, id).unwrap_or_else(|_| id.to_string());
            let f = state::log_file(env, name, &id);
            if !f.exists() {
                bail!("no log of the device {name} for {id} at {}", f.display());
            }
            f
        }
        None if files.len() == 1 => files.remove(0),
        None if files.is_empty() => bail!(
            "the device {name} has no log yet (none under {})",
            dir.display()
        ),
        None => bail!(
            "the device {name} has {} logs; name one with --sandbox ID: {}",
            files.len(),
            files
                .iter()
                .filter_map(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut f =
        std::fs::File::open(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut out = std::io::stdout().lock();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    out.write_all(&buf)?;
    out.flush()?;
    if !follow {
        return Ok(0);
    }
    loop {
        std::thread::sleep(Duration::from_millis(200));
        buf.clear();
        f.read_to_end(&mut buf)?;
        if !buf.is_empty() {
            out.write_all(&buf)?;
            out.flush()?;
        }
    }
}

/// One request to the kv socket of a run.
fn kv_request(
    env: &HostEnv,
    caller: &str,
    req: &serde_json::Value,
) -> Result<paddock::kv::Response> {
    let sock = paddock::sockpath::named(env, caller, "kv.sock")?;
    let mut s = std::os::unix::net::UnixStream::connect(&sock).with_context(|| {
        format!(
            "cannot reach the paddock process of {caller} at {}: the run may have ended, or paddock kv is off for it",
            sock.display()
        )
    })?;
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    writeln!(s, "{req}")?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    serde_json::from_str(line.trim()).context("bad answer from the paddock process")
}

/// `paddock device install NAME`: hand the shared instance to systemd or
/// launchd.
pub(crate) fn install(env: &HostEnv, name: &str) -> Result<String> {
    use paddock::devices::shared::install as sinstall;
    let def = def_of(env, name)?;
    sinstall::check_shared(&def)?;
    let files = sinstall::install(env, name)?;
    let mut out = String::new();
    for f in &files {
        out.push_str(&format!("wrote {}\n", f.display()));
    }
    out.push_str(&format!("{}\n", sinstall::status_line(env, name)));
    out.push_str(&format!(
        "The socket is {}. Remove this again with: paddock device uninstall {name}\n",
        paddock::devices::shared::socket_path(env, name).display()
    ));
    Ok(out)
}

/// `paddock device uninstall NAME`.
pub(crate) fn uninstall(env: &HostEnv, name: &str) -> Result<String> {
    use paddock::devices::shared::install as sinstall;
    let def = def_of(env, name)?;
    sinstall::check_shared(&def)?;
    let files = sinstall::uninstall(env, name)?;
    if files.is_empty() {
        return Ok(format!(
            "nothing to remove: paddock wrote no unit for {name}. The first broker that needs it starts the instance\n"
        ));
    }
    let mut out = String::new();
    for f in &files {
        out.push_str(&format!("removed {}\n", f.display()));
    }
    out.push_str(&format!(
        "The first broker that needs {name} starts its shared instance again\n"
    ));
    Ok(out)
}

/// `paddock device stop|restart NAME` of a shared device: one instance per
/// user, so no run and no `--sandbox`.
fn control_shared(
    env: &HostEnv,
    action: &str,
    def: &paddock::devices::DeviceDef,
) -> Result<String> {
    use paddock::devices::shared;
    let name = &def.name;
    if action == "stop" {
        let was = shared::stop_instance(env, name).map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(if was {
            format!("stopped the shared instance of {name}. The next request starts a new one\n")
        } else {
            format!("the device {name} has no running shared instance\n")
        });
    }
    match shared::restart_instance(env, def).map_err(|e| anyhow::anyhow!("{e}"))? {
        Some(took) => Ok(format!(
            "restarted the shared instance of {name} (started in {} ms). It serves the table as it is now\n",
            took.as_millis()
        )),
        // Without a user manager the instance belongs to the run that
        // starts it, so this command only ends the old one.
        None => Ok(format!(
            "stopped the shared instance of {name}. The next request of a run starts one that serves the table as it is now. `paddock device install {name}` hands the instance to the user manager, and then a restart starts it here\n"
        )),
    }
}

/// `paddock device stop|restart NAME [--sandbox ID | --all]`.
pub(crate) fn control(
    env: &HostEnv,
    action: &str,
    name: &str,
    sandbox: Option<&str>,
    all: bool,
) -> Result<String> {
    let def = def_of(env, name)?;
    if def.scope == paddock::devices::config::Scope::Shared {
        if let Some(id) = sandbox {
            bail!(
                "the device {name} has scope = \"shared\": one instance serves every sandbox of this user, so there is none for {id}. Run `paddock device {action} {name}` without --sandbox"
            );
        }
        return control_shared(env, action, &def);
    }
    let live: Vec<InstanceState> = InstanceState::list(env)
        .into_iter()
        .filter(|s| s.alive())
        .collect();
    let targets: Vec<&InstanceState> = if all {
        live.iter().filter(|s| s.device == name).collect()
    } else {
        pick(env, name, sandbox, &live)?
    };
    if targets.is_empty() {
        return Ok(format!("the device {name} has no running instance\n"));
    }
    let mut out = String::new();
    for t in targets {
        let req = serde_json::json!({ "v": 1, "op": "devices", "action": action, "key": name });
        let r = kv_request(env, &t.caller, &req)?;
        if r.status != paddock::kv::Status::Done {
            bail!("{action} of {name} for {}: {}", t.caller, r.message);
        }
        let verb = if action == "stop" {
            "stopped"
        } else {
            "restarted"
        };
        out.push_str(&format!("{verb} the device {name} of {}", t.caller));
        if let Some(ms) = r.result.get("started_ms").and_then(|v| v.as_u64()) {
            out.push_str(&format!(" (started in {ms} ms)"));
        }
        out.push('\n');
    }
    Ok(out)
}

/// `paddock device stats NAME [--since 7d] [--json]`.
pub(crate) fn stats(env: &HostEnv, name: &str, since: Option<&str>, json: bool) -> Result<String> {
    def_of(env, name)?;
    let window = paddock::devices::ledger::parse_since(since.unwrap_or("7d"))?;
    let from = paddock::state::now().saturating_sub(window);
    let text = std::fs::read_to_string(paddock::broker::audit_file(env)).unwrap_or_default();
    let mut requests = 0u64;
    let mut errors = 0u64;
    let mut cancels = 0u64;
    let mut durations: Vec<u64> = Vec::new();
    let mut connections = 0u64;
    let mut conn_errors = 0u64;
    let (mut bytes_in, mut bytes_out) = (0u64, 0u64);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["device"].as_str() != Some(name) || v["time"].as_u64().unwrap_or(0) < from {
            continue;
        }
        match v["capability"].as_str() {
            Some("device") => {
                requests += 1;
                let status = v["status"].as_str().unwrap_or("");
                match status {
                    "cancelled" => cancels += 1,
                    "exit 0" | "ok" | "200" | "204" | "allowed" => {}
                    s if s.starts_with("exit ") || s.starts_with("signal ") => errors += 1,
                    "waiting" | "rejected" | "declined" | "denied" | "busy" => {}
                    _ => errors += 1,
                }
                if let Some(d) = v["duration_ms"].as_u64() {
                    durations.push(d);
                }
            }
            Some(paddock::devices::sandbox::connect::AUDIT_CAPABILITY) => {
                connections += 1;
                if v["status"].as_str() != Some("ok") {
                    conn_errors += 1;
                }
                bytes_in += v["bytes_in"].as_u64().unwrap_or(0);
                bytes_out += v["bytes_out"].as_u64().unwrap_or(0);
            }
            _ => {}
        }
    }
    durations.sort_unstable();
    let pct = |p: usize| -> Option<u64> {
        if durations.is_empty() {
            return None;
        }
        let i = (durations.len() * p).div_ceil(100).saturating_sub(1);
        durations.get(i.min(durations.len() - 1)).copied()
    };
    let (p50, p95) = (pct(50), pct(95));
    if json {
        let v = serde_json::json!({
            "device": name,
            "since_seconds": window,
            "requests": requests,
            "errors": errors,
            "cancels": cancels,
            "p50_ms": p50,
            "p95_ms": p95,
            "connect": { "connections": connections, "errors": conn_errors, "bytes_in": bytes_in, "bytes_out": bytes_out },
        });
        return Ok(format!("{}\n", serde_json::to_string_pretty(&v)?));
    }
    let ms = |v: Option<u64>| v.map(|n| format!("{n} ms")).unwrap_or_else(|| "-".into());
    let mut out = format!(
        "device {name}, the last {}:\n  requests  {requests}\n  errors    {errors}\n  cancels   {cancels}\n  p50       {}\n  p95       {}\n",
        since.unwrap_or("7d"),
        ms(p50),
        ms(p95)
    );
    if connections > 0 {
        out.push_str(&format!(
            "  connect   {connections} connections ({conn_errors} failed), {bytes_in} bytes to the device, {bytes_out} bytes from it\n"
        ));
    }
    Ok(out)
}
