//! `paddock device`: the devices of the user configuration and their
//! instances. `check` starts one without a run, `logs` prints the log of
//! an instance, `stop` and `restart` reach the paddock process of a run
//! over its kv socket, and `stats` sums the audit lines.

mod example;
mod instances;

pub(crate) use example::add_example;

use anyhow::{bail, Context as _, Result};
use clap::{ArgAction, Args, Subcommand};
use paddock::devices::config::{DeviceDef, ParseEnv, Run, Shape};
use paddock::devices::{DevicesCfg, BUILTIN};
use paddock::env::HostEnv;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub(crate) enum DeviceCmd {
    /// List the devices: name, shape, run, scope, state, grants, instances.
    Ls {
        /// Print the parsed tables as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one device: its table as parsed, the resolved profile of its
    /// sandbox, its credentials, and what the device sandbox reaches.
    Show { name: String },
    /// Add a [devices.NAME] table to ~/.config/paddock/config.toml. Put
    /// --exec or --serve last: the rest of the line is the command.
    /// `--example mac --host mac.local --user me` writes the worked
    /// example of the README instead: the tables, the script, and the
    /// steps for the Mac.
    Add(Box<AddArgs>),
    /// Write the reference device script: 20 lines of Python that
    /// upper-case the request, as an exec and as a serve program.
    Example {
        /// Where to write it (default: upper.py in the current folder).
        path: Option<PathBuf>,
    },
    /// Start an instance of a device without a run, wait for its health,
    /// read its catalog, print how long the start took, and stop it. A
    /// host device prints its warning line first.
    Check {
        name: String,
        /// Start it as the instance of this running sandbox would be: its
        /// project and harness.
        #[arg(long, value_name = "ID")]
        sandbox: Option<String>,
        /// Also send one request and print the response (`-` reads it from
        /// standard input). No card: you run it on the host.
        #[arg(long, value_name = "REQUEST")]
        send: Option<String>,
        /// Keep the instance for this many seconds before stopping it. For
        /// a shared device this holds one lease, as a run does, so the
        /// instance stays warm and `paddock ps` counts this check as a
        /// user.
        #[arg(long, value_name = "SECONDS")]
        hold: Option<u64>,
    },
    /// Print the log of a device instance: the supervisor's lines and the
    /// program's standard error. Never a request or a response.
    Logs {
        name: String,
        /// The sandbox whose instance it is. Without it, the one instance
        /// that has a log.
        #[arg(long, value_name = "ID")]
        sandbox: Option<String>,
        /// Keep printing new lines.
        #[arg(short, long)]
        follow: bool,
    },
    /// Stop the instance of a device. The next request starts it again.
    Stop {
        name: String,
        #[arg(long, value_name = "ID", conflicts_with = "all")]
        sandbox: Option<String>,
        /// Every instance of the device, in every run.
        #[arg(long)]
        all: bool,
    },
    /// Stop the instance of a device and start it again. Clears `failed`.
    Restart {
        name: String,
        #[arg(long, value_name = "ID", conflicts_with = "all")]
        sandbox: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Hand the shared instance of a device (scope = "shared") to the user
    /// manager: a systemd socket unit, or a LaunchAgent on macOS. systemd
    /// or launchd then owns the socket and starts the instance on the first
    /// request, and no broker starts it itself.
    Install { name: String },
    /// Remove the systemd units or the LaunchAgent of a shared device. The
    /// first broker that needs the device starts the instance again.
    Uninstall { name: String },
    /// Serve the devices of this machine to another paddock (a table with
    /// run = "remote" there). Without --socket: one request on the
    /// standard input and output, as `ssh HOST paddock device serve-remote`
    /// runs it. Only tables with a remote: grant are on.
    #[command(name = "serve-remote")]
    ServeRemote {
        /// Serve many connections on this Unix socket instead. The folder
        /// is made 0700: who can reach it can call.
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
        /// Forget the session answers of remote callers and exit.
        #[arg(long, conflicts_with = "socket")]
        forget: bool,
    },
    /// Requests, errors, cancels, the p50 and p95 duration and the connect
    /// relays of a device, from the audit log.
    Stats {
        name: String,
        /// Only the recent time, for example 7d or 12h (default: 7d).
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Raise the budget of a paid device for a run (a sandbox and the
    /// sandboxes it started).
    Budget {
        id: String,
        device: String,
        /// The amount to add, for example 1.0.
        #[arg(long)]
        add: String,
    },
    /// What paid devices spent, by sandbox and device.
    Ledger {
        #[arg(long)]
        json: bool,
        /// Only requests of this project folder.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Only requests of this recent time, for example 7d or 12h.
        #[arg(long)]
        since: Option<String>,
    },
}

#[derive(Args)]
pub(crate) struct AddArgs {
    /// The device name: /dev/paddock/NAME.
    #[arg(required_unless_present = "example")]
    name: Option<String>,
    /// Write a worked example instead of one table: `mac` builds and tests
    /// the caller's worktree on a Mac over ssh. Writes [credentials.mac-ssh],
    /// [devices.mac], [devices.mac.sandbox] and
    /// ~/.config/paddock/devices/mac/build.sh, and prints the Mac steps.
    #[arg(long, value_parser = ["mac"], conflicts_with_all = ["name", "run", "target", "device", "grant", "connect", "ro", "rw", "credential", "caller", "stream", "timeout", "max_request", "max_response", "mode", "exec", "serve"])]
    example: Option<String>,
    /// With --example mac: the Mac, as this host resolves it (default
    /// mac.local).
    #[arg(long, requires = "example", value_name = "HOST")]
    host: Option<String>,
    /// With --example mac: the account on the Mac (default: your user name).
    #[arg(long, requires = "example", value_name = "NAME")]
    user: Option<String>,
    /// With --example mac: the port of sshd on the Mac (default 22).
    #[arg(long, requires = "example", value_name = "PORT")]
    port: Option<u16>,
    /// sandbox (default), host, or remote (with --target and --device).
    #[arg(long, value_parser = ["sandbox", "host", "remote"])]
    run: Option<String>,
    /// For --run remote: ssh://HOST or unix:/PATH, the paddock that serves
    /// the device.
    #[arg(long, value_name = "T", requires = "device")]
    target: Option<String>,
    /// For --run remote: the device name on the other side.
    #[arg(long, value_name = "NAME", requires = "target")]
    device: Option<String>,
    /// pasture:NAME, project:PATH, harness:NAME or all (repeatable).
    /// Without a grant, every sandbox sees the device.
    #[arg(long, value_name = "G")]
    grant: Vec<String>,
    /// LOCAL_PORT:HOST:PORT: 127.0.0.1:LOCAL_PORT inside the device sandbox
    /// reaches HOST:PORT from the host (repeatable).
    #[arg(long, value_name = "C")]
    connect: Vec<String>,
    /// A host path the device sandbox reads (repeatable).
    #[arg(long, value_name = "P")]
    ro: Vec<String>,
    /// A host path the device sandbox writes (repeatable).
    #[arg(long, value_name = "P")]
    rw: Vec<String>,
    /// A [credentials.NAME] of the user configuration (repeatable).
    #[arg(long, value_name = "N")]
    credential: Vec<String>,
    /// project:ro: the caller's project, read-only, in the device sandbox.
    #[arg(long, value_name = "project:ro")]
    caller: Vec<String>,
    /// The response streams as the program writes it.
    #[arg(long)]
    stream: bool,
    /// Seconds per request (default 30).
    #[arg(long, value_name = "N")]
    timeout: Option<u64>,
    /// The largest request, for example 4K or 1M.
    #[arg(long, value_name = "S")]
    max_request: Option<String>,
    /// The largest response, for example 16M.
    #[arg(long, value_name = "S")]
    max_response: Option<String>,
    /// ask (default): the first request of a sandbox shows a card. allow:
    /// no card.
    #[arg(long, value_parser = ["ask", "allow"])]
    mode: Option<String>,
    /// enabled = true (default) or false.
    #[arg(long, default_value_t = true, action = ArgAction::Set, value_name = "BOOL")]
    enabled: bool,
    /// The command runs once per request: the request on stdin, the
    /// response on stdout. The rest of the line is the command.
    #[arg(long, num_args = 1.., allow_hyphen_values = true, value_name = "CMD", conflicts_with = "serve", required_unless_present_any = ["serve", "target", "example"])]
    exec: Vec<String>,
    /// The command runs once per instance and serves HTTP/1.1 on the Unix
    /// socket $PADDOCK_DEVICE_SOCKET. The rest of the line is the command.
    #[arg(long, num_args = 1.., allow_hyphen_values = true, value_name = "CMD")]
    serve: Vec<String>,
}

/// The reference script of a device program: exec and serve in one file.
pub use paddock::devices::program::EXAMPLE;

pub(crate) fn run(env: &HostEnv, cmd: DeviceCmd) -> Result<i32> {
    use paddock::devices::ledger;
    match cmd {
        DeviceCmd::Ls { json } => {
            print!("{}", ls(env, json)?);
            Ok(0)
        }
        DeviceCmd::Show { name } => {
            print!("{}", show(env, &name)?);
            Ok(0)
        }
        DeviceCmd::Add(a) => {
            if a.example.is_some() {
                print!(
                    "{}",
                    add_example(env, &a, paddock::devices::examples::keyscan)?
                );
            } else {
                print!("{}", add(env, &a)?);
            }
            Ok(0)
        }
        DeviceCmd::Example { path } => {
            let path = path.unwrap_or_else(|| PathBuf::from("upper.py"));
            print!("{}", example(&path)?);
            Ok(0)
        }
        DeviceCmd::Check {
            name,
            sandbox,
            send,
            hold,
        } => {
            let send = match send.as_deref() {
                Some("-") => {
                    let mut v = Vec::new();
                    std::io::Read::read_to_end(&mut std::io::stdin(), &mut v)?;
                    Some(v)
                }
                Some(s) => Some(s.as_bytes().to_vec()),
                None => None,
            };
            print!(
                "{}",
                instances::check(env, &name, sandbox.as_deref(), send.as_deref(), hold)?
            );
            Ok(0)
        }
        DeviceCmd::Logs {
            name,
            sandbox,
            follow,
        } => instances::logs(env, &name, sandbox.as_deref(), follow),
        DeviceCmd::Stop { name, sandbox, all } => {
            print!(
                "{}",
                instances::control(env, "stop", &name, sandbox.as_deref(), all)?
            );
            Ok(0)
        }
        DeviceCmd::Restart { name, sandbox, all } => {
            print!(
                "{}",
                instances::control(env, "restart", &name, sandbox.as_deref(), all)?
            );
            Ok(0)
        }
        DeviceCmd::Install { name } => {
            print!("{}", instances::install(env, &name)?);
            Ok(0)
        }
        DeviceCmd::Uninstall { name } => {
            print!("{}", instances::uninstall(env, &name)?);
            Ok(0)
        }
        DeviceCmd::ServeRemote { socket, forget } => {
            if forget {
                let p = paddock::devices::forward::session::path(env);
                paddock::devices::forward::session::forget(env)
                    .with_context(|| format!("cannot remove {}", p.display()))?;
                println!(
                    "forgot the session answers of remote callers ({})",
                    p.display()
                );
                return Ok(0);
            }
            paddock::devices::forward::serve::run(env, socket.as_deref())
        }
        DeviceCmd::Stats { name, since, json } => {
            print!("{}", instances::stats(env, &name, since.as_deref(), json)?);
            Ok(0)
        }
        DeviceCmd::Budget { id, device, add } => {
            let id = paddock::ids::resolve(env, &id)?;
            let add = ledger::parse_amount(&add)?;
            let (root, budget) = ledger::raise(env, &id, &device, add)?;
            println!(
                "The budget of {device} for {root} is now {}. The next request can pass.",
                ledger::money(budget)
            );
            Ok(0)
        }
        DeviceCmd::Ledger {
            json,
            project,
            since,
        } => {
            let project = project
                .map(|p| paddock::kv::host::project_of(Some(&p)))
                .transpose()?;
            let since = since.map(|s| ledger::parse_since(&s)).transpose()?;
            print!("{}", ledger::report(env, project.as_deref(), since, json)?);
            Ok(0)
        }
    }
}

pub(crate) fn config_file(env: &HostEnv) -> PathBuf {
    env.paddock_config().join("config.toml")
}

/// The scope column: `shared, 3 users` for a running shared instance.
fn scope_column(env: &HostEnv, d: &DeviceDef) -> String {
    use paddock::devices::config::Scope;
    if d.shape == Shape::HttpProxy {
        return "-".into();
    }
    if d.scope != Scope::Shared {
        return d.scope.name().into();
    }
    match paddock::devices::shared::users_of(env, &d.name) {
        Some(1) => "shared, 1 user".into(),
        Some(n) => format!("shared, {n} users"),
        None => "shared, stopped".into(),
    }
}

/// The run column: `HOST` in capitals, so a host device stands out.
fn run_column(d: &DeviceDef) -> String {
    match d.run {
        Run::Host { .. } => "HOST".into(),
        _ => d.run.name().into(),
    }
}

fn grants_column(d: &DeviceDef, home: &Path) -> String {
    match &d.grant {
        None => "every sandbox (no grant)".into(),
        Some(g) => g
            .iter()
            .map(|g| match g {
                paddock::devices::grant::Grant::Project(p) => {
                    format!("project:{}", paddock::devices::grant::tilde(p, home))
                }
                other => other.render(),
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// `paddock device ls`.
pub(crate) fn ls(env: &HostEnv, json: bool) -> Result<String> {
    let cfg = DevicesCfg::load(env)?;
    if json {
        let v = serde_json::json!({
            "mount": cfg.mount,
            "builtin": BUILTIN.iter().map(|b| serde_json::json!({
                "name": b,
                "enabled": cfg.enabled.iter().any(|e| e == b),
            })).collect::<Vec<_>>(),
            "devices": cfg.user,
        });
        return Ok(format!("{}\n", serde_json::to_string_pretty(&v)?));
    }
    let mut rows: Vec<[String; 7]> = Vec::new();
    for b in BUILTIN {
        let on = cfg.enabled.iter().any(|e| e == b);
        rows.push([
            b.to_string(),
            "builtin".into(),
            "builtin".into(),
            "-".into(),
            if on { "on" } else { "off" }.into(),
            "-".into(),
            "-".into(),
        ]);
    }
    let live = paddock::devices::sandbox::state::InstanceState::list(env);
    for d in &cfg.user {
        let running: Vec<String> = live
            .iter()
            .filter(|s| s.device == d.name && s.alive())
            .map(|s| paddock::ids::short(&s.caller).to_string())
            .collect();
        rows.push([
            d.name.clone(),
            d.shape.name().into(),
            run_column(d),
            scope_column(env, d),
            if d.enabled { "on" } else { "off" }.into(),
            grants_column(d, &env.home),
            if running.is_empty() {
                "-".into()
            } else {
                running.join(", ")
            },
        ]);
    }
    let head = [
        "NAME",
        "SHAPE",
        "RUN",
        "SCOPE",
        "STATE",
        "GRANTS",
        "INSTANCES",
    ];
    let mut width = [0usize; 7];
    for r in std::iter::once(&head.map(String::from)).chain(rows.iter()) {
        for (i, c) in r.iter().enumerate() {
            width[i] = width[i].max(c.len());
        }
    }
    let mut out = String::new();
    for r in std::iter::once(&head.map(String::from)).chain(rows.iter()) {
        let line: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = width[i]))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    if !cfg.mount {
        out.push_str("\n[devices] mount = false: no sandbox gets /dev/paddock\n");
    }
    Ok(out)
}

/// `paddock device show NAME`.
pub(crate) fn show(env: &HostEnv, name: &str) -> Result<String> {
    let cfg = DevicesCfg::load(env)?;
    let Some(d) = cfg.user.iter().find(|u| u.name == name) else {
        if BUILTIN.contains(&name) {
            let on = cfg.enabled.iter().any(|e| e == name);
            return Ok(format!(
                "{name}: a built-in device, {}. [devices] {name} = true turns it on in {}\n",
                if on { "on" } else { "off" },
                config_file(env).display()
            ));
        }
        bail!(
            "no [devices.{name}] table in {}. `paddock device ls` lists the devices",
            config_file(env).display()
        );
    };
    let mut out = String::new();
    let shared = d.scope == paddock::devices::config::Scope::Shared;
    out.push_str("# the table as parsed\n");
    out.push_str(&d.to_toml());
    out.push_str(&format!(
        "\nshape     {}\nrun       {}\nscope     {}\nstate     {}\ngrants    {}\nstream    {}\nlimits    request {} bytes, response {} bytes, timeout {} s, concurrency {}\n",
        d.shape.name(),
        run_column(d),
        d.scope.name(),
        if d.enabled { "on" } else { "off" },
        grants_column(d, &env.home),
        d.stream,
        d.limits.max_request,
        d.limits.max_response,
        d.limits.timeout,
        d.limits.concurrency
    ));
    if shared {
        out.push_str(&format!(
            "idle      {} s without a request, with no run that uses it\nsocket    {}\nstart     {}\n",
            d.idle,
            paddock::devices::shared::socket_path(env, &d.name).display(),
            paddock::devices::shared::install::status_line(env, &d.name)
        ));
        if let Some(n) = paddock::devices::shared::users_of(env, &d.name) {
            out.push_str(&format!("instance  running, {n} users\n"));
        }
    }
    match &d.run {
        Run::Host { cwd, env: vars } => {
            out.push_str("\nruns on the host as you, with your home, your keys and your network\n");
            out.push_str(&format!(
                "  cwd       {}\n",
                cwd.as_ref()
                    .map(|c| c.display().to_string())
                    .unwrap_or_else(|| format!("{} (the home)", env.home.display()))
            ));
            for (k, v) in vars {
                out.push_str(&format!("  env       {k}={v}\n"));
            }
        }
        Run::Remote { target, device } => {
            out.push_str(&format!(
                "\nruns on another machine: device {device} of {target}\n"
            ));
        }
        Run::Broker => {
            if let Some(p) = &d.http {
                out.push_str(&format!(
                    "\nreaches:\n  http: {} (methods {}; paths {})\n",
                    p.target,
                    p.methods.join(", "),
                    p.paths.join(", ")
                ));
            }
        }
        Run::Sandbox => {
            let exec = paddock::execbox::ExecCfg::load(env).unwrap_or_default();
            let p = d.profile().with_exec_base(&exec);
            out.push_str(&format!(
                "\nprofile of the device sandbox (resolved{}):\n",
                if d.profile().exec_base {
                    ", the [exec] keys of the user configuration as the base"
                } else {
                    ""
                }
            ));
            out.push_str(&format!("  network     {}\n", p.network.name()));
            if !p.allow.is_empty() {
                out.push_str(&format!("  allow       {}\n", p.allow.join(", ")));
            }
            out.push_str(&format!(
                "  home        {}\n",
                match p.home {
                    paddock::execbox::Home::Hidden => "hidden",
                    paddock::execbox::Home::ReadOnly => "read-only",
                    paddock::execbox::Home::Full => "full",
                }
            ));
            for c in &p.connect {
                out.push_str(&format!("  connect     {}\n", c.render()));
            }
            for r in &p.ro {
                out.push_str(&format!("  ro          {}\n", r.display()));
            }
            for r in &p.rw {
                out.push_str(&format!("  rw          {}\n", r.display()));
            }
            for (k, v) in &p.env {
                out.push_str(&format!("  env         {k}={v}\n"));
            }
            if !p.env_pass.is_empty() {
                out.push_str(&format!("  env_pass    {}\n", p.env_pass.join(", ")));
            }
            for c in &p.caller {
                out.push_str(&format!("  caller      {}\n", c.name()));
            }
            if let Some(m) = p.mem {
                out.push_str(&format!(
                    "  mem         {}\n",
                    paddock::spawn::format_bytes(m)
                ));
            }
            if let Some(c) = p.cpu_percent {
                out.push_str(&format!(
                    "  cpus        {}\n",
                    paddock::spawn::format_cpus(c)
                ));
            }
            if !p.credentials.is_empty() {
                let known = paddock::credentials::load(env).unwrap_or_default();
                out.push_str("\ncredentials:\n");
                for c in &p.credentials {
                    match known.get(c) {
                        Some(k) => out.push_str(&format!("  {c}  via {}\n", k.via.name())),
                        None => out.push_str(&format!(
                            "  {c}  not in [credentials] of {}\n",
                            config_file(env).display()
                        )),
                    }
                }
            }
            let reach = p.reach_lines();
            out.push_str("\nreaches (everything else of the host is absent):\n");
            if reach.is_empty() {
                out.push_str(
                    "  nothing: the system read-only, an empty home, its own /tmp, no network\n",
                );
            }
            for l in reach {
                out.push_str(&format!("  {l}\n"));
            }
        }
    }
    if let Some(why) = d.not_yet() {
        out.push_str(&format!(
            "\nnot available yet: {why}. The file exists and a request fails with EIO\n"
        ));
    }
    Ok(out)
}

/// `paddock device add`: build the table, validate it with the parser,
/// append it to the user configuration, print it.
pub(crate) fn add(env: &HostEnv, a: &AddArgs) -> Result<String> {
    use toml::Value;
    let name = a
        .name
        .as_deref()
        .context("give the device name, or --example mac")?;
    let strings = |v: &[String]| Value::Array(v.iter().map(|s| Value::String(s.clone())).collect());
    let mut t = toml::Table::new();
    let run = a.run.clone().unwrap_or_else(|| {
        if a.target.is_some() {
            "remote".into()
        } else {
            "sandbox".into()
        }
    });
    if run == "remote" {
        let (Some(target), Some(device)) = (&a.target, &a.device) else {
            bail!("--run remote needs --target ssh://HOST or unix:/PATH and --device NAME, the device on the other side");
        };
        if !a.exec.is_empty() || !a.serve.is_empty() {
            bail!("--run remote takes no --exec or --serve: the command is in the table on the other side");
        }
        t.insert("run".into(), Value::String(run.clone()));
        t.insert("target".into(), Value::String(target.clone()));
        t.insert("device".into(), Value::String(device.clone()));
    } else {
        if a.target.is_some() {
            bail!("--target and --device are for --run remote");
        }
        let (shape, command) = if !a.exec.is_empty() {
            ("exec", &a.exec)
        } else {
            ("serve", &a.serve)
        };
        t.insert("shape".into(), Value::String(shape.into()));
        t.insert("run".into(), Value::String(run.clone()));
        t.insert("command".into(), strings(command));
    }
    t.insert("enabled".into(), Value::Boolean(a.enabled));
    if !a.grant.is_empty() {
        t.insert("grant".into(), strings(&a.grant));
    }
    if let Some(m) = &a.mode {
        t.insert("mode".into(), Value::String(m.clone()));
    }
    if a.stream {
        t.insert("stream".into(), Value::Boolean(true));
    }
    if let Some(s) = a.timeout {
        t.insert("timeout".into(), Value::Integer(s as i64));
    }
    if let Some(s) = &a.max_request {
        t.insert("max_request".into(), Value::String(s.clone()));
    }
    if let Some(s) = &a.max_response {
        t.insert("max_response".into(), Value::String(s.clone()));
    }
    let mut sandbox = toml::Table::new();
    if !a.connect.is_empty() {
        sandbox.insert("connect".into(), strings(&a.connect));
    }
    if !a.ro.is_empty() {
        sandbox.insert("ro".into(), strings(&a.ro));
    }
    if !a.rw.is_empty() {
        sandbox.insert("rw".into(), strings(&a.rw));
    }
    if !a.credential.is_empty() {
        sandbox.insert("credentials".into(), strings(&a.credential));
    }
    if !a.caller.is_empty() {
        sandbox.insert("caller".into(), strings(&a.caller));
    }
    if !sandbox.is_empty() {
        t.insert("sandbox".into(), Value::Table(sandbox));
    }
    let penv = ParseEnv::host(&env.home);
    let def = DeviceDef::parse_with(name, &t, penv)?;
    let file = config_file(env);
    let cfg = DevicesCfg::load(env)?;
    if cfg.user.iter().any(|u| u.name == name) {
        bail!(
            "a [devices.{name}] table exists in {}. Edit it there, or pick another name",
            file.display()
        );
    }
    let text = def.to_toml();
    // The round trip: what goes into the file parses to what was checked.
    let back = DevicesCfg::parse_with(&text, penv)
        .with_context(|| format!("the rendered table does not parse:\n{text}"))?;
    if back.user.first() != Some(&def) {
        bail!("the rendered table does not parse back to the same device:\n{text}");
    }
    let mut existing = std::fs::read_to_string(&file).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str("\n# added by paddock device add\n");
    existing.push_str(&text);
    paddock::privfs::write(&file, existing)
        .with_context(|| format!("cannot write {}", file.display()))?;
    // The file as a whole still loads: a broken file would stop every run.
    DevicesCfg::load(env).with_context(|| format!("{} after the add", file.display()))?;
    let mut out = format!("# added to {}\n{text}", file.display());
    if let Some(why) = def.not_yet() {
        out.push_str(&format!("\nnot available yet: {why}\n"));
    }
    if def.grant.is_none() && def.profile().reaches_something() {
        out.push_str("\nnote: every sandbox sees this device, and its sandbox reaches something; add a grant (--grant pasture:NAME, project:PATH or harness:NAME)\n");
    }
    Ok(out)
}

/// `paddock device example [PATH]`.
pub(crate) fn example(path: &Path) -> Result<String> {
    if path.exists() {
        bail!("{} exists; name another file", path.display());
    }
    std::fs::write(path, EXAMPLE).with_context(|| format!("cannot write {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    let shown = if path.is_absolute() || path.starts_with(".") {
        path.display().to_string()
    } else {
        format!("./{}", path.display())
    };
    Ok(format!(
        "wrote {}\nAdd it as a device:\n  paddock device add upper --exec {shown}\nor as a service:\n  paddock device add upper --serve {shown}\n",
        path.display()
    ))
}

#[cfg(test)]
mod tests;
