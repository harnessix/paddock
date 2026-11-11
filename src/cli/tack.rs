use anyhow::{Context as _, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::tack::proto::{Answer, Launch, Request, Verdict};
use paddock::tack::service::{self, Paths};
use paddock::tack::{audit, client, config, import, report, status, store, tackshims};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum TackCmd {
    /// Move the values of variables from your shell startup files,
    /// environment.d and the .env files you name into the store, and put
    /// references (tack://NAME) in their place. Without --yes it only
    /// prints what it would do. It never prints a value.
    Import {
        /// The variables, such as GH_TOKEN.
        #[arg(required = true)]
        names: Vec<String>,
        /// Also read this file, such as a project .env file. Repeat it for
        /// more files.
        #[arg(long = "file", value_name = "PATH")]
        files: Vec<PathBuf>,
        /// Write: store the values and rewrite the lines.
        #[arg(long)]
        yes: bool,
        /// Overwrite a different value that the store holds already.
        #[arg(long)]
        replace: bool,
    },
    /// List the secrets of [tack] and the imported ones. Never a value.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Without sudo: put a stand-in for each allowlisted program on PATH
    /// (login shells, environment.d, sandboxes). With sudo: install tackd,
    /// the system service that decides and starts the programs, or update
    /// it to this paddock, and the stand-ins of the user who ran sudo. Run
    /// it again after each paddock update and after you change [tack].
    Setup,
    /// Without sudo: delete your stand-ins and their PATH lines. With sudo:
    /// stop tackd and delete its service, binary, socket and audit log, and
    /// the stand-ins of the user who ran sudo.
    Remove {
        /// Keep the audit log.
        #[arg(long)]
        keep_audit: bool,
    },
    /// Whether tackd is installed and runs, its version and its socket.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// The decisions that tackd made for your programs, newest last.
    /// Names only: never a value or an argument.
    Audit {
        /// How many decisions.
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
    /// Ask tackd what it would decide for a program and its arguments,
    /// started from here. It starts nothing. Exit code 0: allow, 1: deny,
    /// 3: ask.
    Check {
        /// The program and its arguments.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Start a program through tackd, with the secrets that its allowlist
    /// gives it. The stand-ins on PATH run this. Exit code: the program's;
    /// 77 when tackd refuses, 69 when tackd is installed but does not
    /// answer or cannot start it, 70 when tackd went away while it ran.
    Run {
        /// Run as the stand-in of the program: skip the stand-in folder
        /// when looking for the program, and say nothing for a program that
        /// no allowlist names.
        #[arg(long, hide = true)]
        shim: bool,
        /// The secret names of the program, for a stand-in in a sandbox.
        #[arg(long, hide = true)]
        secrets: Option<String>,
        /// The program and its arguments.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// tackd itself, as the system service runs it.
    #[command(hide = true)]
    Serve,
    /// tackd runs this as you before a program at protect = "uid" starts:
    /// it lets the helper account reach the working folder (an ACL). The
    /// grant comes on standard input.
    #[command(hide = true)]
    AclGrant,
}

pub(crate) fn run(env: &HostEnv, cmd: TackCmd) -> Result<i32> {
    match cmd {
        TackCmd::List { json } => list(env, json),
        TackCmd::Setup => setup(env),
        TackCmd::Remove { keep_audit } => remove(env, keep_audit),
        TackCmd::Status { json } => status_cmd(env, json),
        TackCmd::Audit { lines, json } => audit_cmd(lines, json),
        TackCmd::Check { command, json } => check(env, command, json),
        TackCmd::Serve => paddock::tack::daemon::main(&Paths::system()),
        TackCmd::AclGrant => acl_grant(),
        TackCmd::Run {
            shim,
            secrets,
            command,
        } => paddock::tack::standin::run(env, shim, secrets.as_deref(), &command),
        TackCmd::Import {
            names,
            files,
            yes,
            replace,
        } => {
            let cfg = config::load(env)?;
            let opts = import::Options {
                names,
                files,
                write: yes,
                replace,
            };
            let open = |_: &str, c: &config::SecretCfg| store::open(env, c);
            let mut plan = import::plan(env, &cfg, &opts, &open)?;
            for l in plan.describe(env) {
                println!("{l}");
            }
            if !yes {
                println!("\nDry run: nothing changed. Run the same command with --yes to store the values and rewrite the lines.");
                println!("Note: {}.", report::PHASE_NOTE);
                return Ok(if plan.ok() { 0 } else { 1 });
            }
            let written = plan.apply(env);
            let mut failed = false;
            for w in &written {
                let f = paddock::tack::show(env, &w.file);
                match &w.result {
                    Ok(b) => println!(
                        "wrote    {f}\n           backup: {} (to undo: cp {} {})",
                        paddock::tack::show(env, b),
                        b.display(),
                        w.file.display()
                    ),
                    Err(e) => {
                        failed = true;
                        eprintln!("error    {f}: {e}");
                    }
                }
            }
            for n in &plan.names {
                if let import::Action::Refused(why) = &n.action {
                    eprintln!("refused  {}: {why}", n.name);
                }
            }
            if !written.is_empty() {
                println!("\nThe backups hold the old files with their plain values: delete them once the new files work.");
                println!(
                    "Start a new shell, so that it reads the references. Note: {}.",
                    report::PHASE_NOTE
                );
            }
            Ok(if plan.ok() && !failed { 0 } else { 1 })
        }
    }
}

fn list(env: &HostEnv, json: bool) -> Result<i32> {
    let cfg = config::load(env)?;
    let rows = report::rows(env, &cfg);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "api_version": "paddock.tack/v1", "secrets": rows,
            }))?
        );
        return Ok(0);
    }
    if rows.is_empty() {
        println!(
            "No secrets are set up. Add [tack.secrets.NAME] to {}, or run paddock tack import NAME.",
            config::file(env).display()
        );
        return Ok(0);
    }
    for r in &rows {
        let progs = match r.programs.len() {
            0 => "no program".to_string(),
            _ => r.programs.join(", "),
        };
        println!(
            "{:<24} {:<20} {:<28} {}{}",
            r.name,
            r.store,
            r.reference,
            progs,
            if r.configured {
                ""
            } else {
                "  (imported, not in the configuration)"
            }
        );
        if !r.hosts.is_empty() {
            println!("{:<24} hosts: {}", "", r.hosts.join(", "));
        }
        if let Some(t) = r.last_import {
            println!("{:<24} last import {}", "", paddock::storage::date(t));
        }
    }
    println!("\nNote: {}.", report::PHASE_NOTE);
    Ok(0)
}

fn need_root(env: &HostEnv, what: &str) -> Result<()> {
    paddock::update::refuse_in_sandbox(env, what)?;
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        anyhow::bail!("tackd runs on Linux and macOS only");
    }
    // SAFETY: geteuid cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("{what} changes system files: run sudo {what}");
    }
    Ok(())
}

fn is_root() -> bool {
    // SAFETY: geteuid cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The user part of setup: the stand-ins and PATH.
fn setup_user(env: &HostEnv) -> Result<i32> {
    paddock::update::refuse_in_sandbox(env, "paddock tack setup")?;
    let cfg = config::load(env)?;
    let me = paddock::update::current_binary()?;
    let names = tackshims::names(&cfg);
    for l in tackshims::install(env, &cfg, &me)? {
        println!("{l}");
    }
    if names.is_empty() {
        println!("No program is allowlisted in [tack] yet: no stand-in to write. Run paddock tack setup again after you add one.");
    } else {
        println!(
            "Stand-ins for {} are in {}, first on PATH in new login shells and in the systemd user session.",
            names.join(", "),
            paddock::tack::show(env, &tackshims::dir(env))
        );
        println!("Log in again (or start a new login shell) so that they are used. paddock doctor shows what is not covered.");
    }
    let installed = service::ours(&Paths::system().unit);
    if !installed && cfg!(target_os = "linux") {
        println!("tackd is not installed: the stand-ins run the programs with the references until you run sudo paddock tack setup.");
    }
    Ok(0)
}

/// Run the user part as the user who ran sudo, when there is one.
fn as_sudo_user(what: &str) -> Result<()> {
    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|u| u.parse::<u32>().ok());
    let gid = std::env::var("SUDO_GID")
        .ok()
        .and_then(|u| u.parse::<u32>().ok());
    let (Some(uid), Some(gid)) = (uid, gid) else {
        return Ok(());
    };
    if uid == 0 {
        return Ok(());
    }
    let Some((user, home)) = paddock::tack::daemon::passwd(uid) else {
        return Ok(());
    };
    let me = paddock::update::current_binary()?;
    use std::os::unix::process::CommandExt;
    println!(
        "
For {user}:"
    );
    let st = std::process::Command::new(&me)
        .args(["tack", what])
        .uid(uid)
        .gid(gid)
        .env_clear()
        .env("HOME", &home)
        .env("USER", &user)
        .env("LOGNAME", &user)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(&home)
        .status()
        .with_context(|| format!("cannot run paddock tack {what} as {user}"))?;
    if !st.success() {
        eprintln!(
            "paddock tack {what} as {user} failed ({st}): run paddock tack {what} without sudo"
        );
    }
    Ok(())
}

fn setup(env: &HostEnv) -> Result<i32> {
    if !is_root() {
        return setup_user(env);
    }
    need_root(env, "paddock tack setup")?;
    if cfg!(target_os = "linux") && !std::path::Path::new("/run/systemd/system").is_dir() {
        anyhow::bail!("tackd needs systemd as the system manager, and this host does not run it. tack stays in rootless mode");
    }
    let p = Paths::system();
    let source = paddock::update::current_binary()?;
    let rep = service::setup(&p, &source, 0, std::path::Path::new("/"), &service::System)?;
    for l in &rep.lines {
        println!("{l}");
    }
    // Wait for the new tackd to answer with this version.
    let ours = env!("CARGO_PKG_VERSION");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let st = loop {
        let st = status::read(&p, std::time::Duration::from_secs(2));
        let ready = st.running.as_ref().is_ok_and(|s| s.version == ours);
        if ready || std::time::Instant::now() > end {
            break st;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    println!();
    for l in status::lines(&st, ours) {
        println!("{l}");
    }
    let ok = st
        .running
        .as_ref()
        .is_ok_and(|s| s.version == ours && s.audit_error.is_none());
    if !ok {
        eprintln!(
            "\ntackd is installed but did not come up as version {ours}: see the lines above"
        );
        return Ok(1);
    }
    println!("\ntackd reads the [tack] table of each user from ~/.config/paddock/config.toml. Run sudo paddock tack setup again after each paddock update.");
    let helpers_ok = setup_helpers()?;
    println!("Note: {}.", report::PHASE_NOTE);
    as_sudo_user("setup")?;
    Ok(if helpers_ok { 0 } else { 1 })
}

/// The helper accounts of protect = "uid", for every user whose
/// configuration has an entry at that level (Linux). Returns whether each
/// one is ready.
fn setup_helpers() -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        use paddock::tack::{daemon, protect_setup};
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let mut people = paddock::tack::accounts::people(&passwd);
        let sudo = std::env::var("SUDO_UID")
            .ok()
            .and_then(|u| u.parse::<u32>().ok());
        if let Some(pw) = sudo
            .filter(|u| *u != 0)
            .and_then(paddock::tack::accounts::by_uid)
        {
            if !people.iter().any(|(_, u)| *u == pw.uid) {
                people.push((pw.name, pw.uid));
            }
        }
        let config = |uid: u32| {
            let home =
                daemon::home_of(uid).ok_or_else(|| format!("uid {uid} has no home folder"))?;
            daemon::read_config(&daemon::config_file(&home), uid)
        };
        let (users, problems) = protect_setup::wanted(&people, &config);
        println!("\nHelper accounts for protect = \"uid\":");
        for p in problems {
            println!("  note      {p}");
        }
        let state = std::path::Path::new(paddock::tack::STATE_DIR);
        let (lines, ok) = protect_setup::setup(state, &users, &service::System)?;
        for l in lines {
            println!("  {l}");
        }
        Ok(ok)
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!(
            "\nprotect = \"uid\" is for Linux only in this version: no helper account is made."
        );
        Ok(true)
    }
}

fn remove(env: &HostEnv, keep_audit: bool) -> Result<i32> {
    if !is_root() {
        paddock::update::refuse_in_sandbox(env, "paddock tack remove")?;
        let out = tackshims::remove(env)?;
        if out.is_empty() {
            println!("No stand-ins are set up for you: nothing to remove.");
        }
        for l in out {
            println!("{l}");
        }
        if service::ours(&Paths::system().unit) {
            println!("tackd stays installed: sudo paddock tack remove removes it.");
        }
        return Ok(0);
    }
    need_root(env, "paddock tack remove")?;
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut out = service::remove(&Paths::system(), keep_audit, &service::System)?;
    #[cfg(target_os = "linux")]
    {
        // tackd is stopped: no start grants anything while this runs.
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        out.extend(paddock::tack::protect_setup::remove(
            std::path::Path::new(paddock::tack::STATE_DIR),
            &passwd,
            &service::System,
        ));
    }
    if out.is_empty() {
        println!("tackd is not installed: nothing to remove.");
    }
    for l in out {
        println!("{l}");
    }
    as_sudo_user("remove")?;
    Ok(0)
}

fn status_cmd(env: &HostEnv, json: bool) -> Result<i32> {
    let p = Paths::system();
    let st = status::read(&p, client::TIMEOUT);
    let ours = env!("CARGO_PKG_VERSION");
    if json {
        let running = match &st.running {
            Ok(s) => serde_json::to_value(s)?,
            Err(e) => serde_json::json!({ "error": e }),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "api_version": "paddock.tack/v1",
                "paddock": ours,
                "installed": st.installed,
                "foreign_service_file": st.foreign,
                "binary": st.binary,
                "socket": st.socket,
                "tackd": running,
                "protect": protect_json(env),
            }))?
        );
        return Ok(0);
    }
    for l in status::lines_for(env, &st, ours) {
        println!("{l}");
    }
    Ok(if st.installed && st.running.is_err() {
        1
    } else {
        0
    })
}

/// The level of each allowlist entry, for `paddock tack status --json`.
fn protect_json(env: &HostEnv) -> serde_json::Value {
    let Ok(cfg) = config::load(env) else {
        return serde_json::Value::Null;
    };
    let rows: Vec<serde_json::Value> = cfg
        .secrets
        .iter()
        .flat_map(|(name, s)| {
            s.allow.iter().map(move |a| {
                serde_json::json!({
                    "secret": name,
                    "program": a.program,
                    "level": a.level().name(),
                    "source": paddock::tack::protect::source(a),
                    "write": a.write,
                })
            })
        })
        .collect();
    serde_json::Value::Array(rows)
}

fn audit_cmd(lines: usize, json: bool) -> Result<i32> {
    let p = Paths::system();
    let got = match client::ask(&p.socket, Request::Audit { lines }, client::TIMEOUT) {
        Ok(Answer::Audit { lines }) => lines,
        Ok(_) => anyhow::bail!("tackd answered with something else"),
        // Root can read the file while tackd is down.
        Err(e) if unsafe { libc::geteuid() } == 0 && p.audit.exists() => {
            eprintln!("note: {e}; reading {} directly", p.audit.display());
            audit::Log::new(p.audit.clone(), 0).read(None, lines)
        }
        Err(e) => return Err(e.into()),
    };
    if got.is_empty() && !json {
        println!("tackd has made no decision for you yet.");
    }
    for l in got {
        if json {
            println!("{l}");
        } else if let Ok(r) = serde_json::from_str::<audit::Record>(&l) {
            println!("{}", r.show());
        }
    }
    Ok(0)
}

fn check(env: &HostEnv, command: Vec<String>, json: bool) -> Result<i32> {
    let (prog, args) = command.split_first().expect("clap requires one word");
    let program = env
        .which(prog)
        .with_context(|| format!("{prog} is not a program on PATH"))?;
    let cwd = std::env::current_dir()?;
    let req = Request::Launch(Launch {
        program: program.display().to_string(),
        args: args.to_vec(),
        cwd: cwd.display().to_string(),
        caller_pid: std::process::id(),
    });
    let d = match client::ask(&Paths::system().socket, req, client::TIMEOUT)? {
        Answer::Decision(d) => d,
        _ => anyhow::bail!("tackd answered with something else"),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
    } else {
        println!("{}  {}", d.verdict.name(), program.display());
        println!("  reason   {}", d.reason);
        for s in &d.secrets {
            let hosts = d.hosts.get(s).filter(|h| !h.is_empty());
            match hosts {
                Some(h) => println!("  secret   {s} (hosts: {})", h.join(", ")),
                None => println!("  secret   {s}"),
            }
        }
        if let Some(p) = &d.program {
            println!("  program  {} sha256 {}", p.real, p.sha256);
            if d.verdict != Verdict::Deny {
                if let Ok(cfg) = config::load(env) {
                    let e = paddock::tack::protect::effective(
                        &cfg,
                        &d.secrets,
                        std::path::Path::new(&p.path),
                        std::path::Path::new(&p.real),
                    );
                    println!(
                        "  protect  {}",
                        match e.level {
                            paddock::tack::protect::Level::Uid => format!(
                                "uid{}: runs as your helper account; {}",
                                if e.write {
                                    ", may write the working folder"
                                } else {
                                    ", reads the working folder"
                                },
                                paddock::tack::protect::UID_GAIN
                            ),
                            paddock::tack::protect::Level::Refs =>
                                format!("refs: runs as you; {}", paddock::tack::protect::REFS_LIMIT),
                        }
                    );
                }
            }
        }
        if !d.chain.is_empty() {
            println!("  callers  {}", d.chain.join(" < "));
        }
        if d.verdict == Verdict::Ask {
            println!(
                "  note     this paddock cannot ask yet: a start that needs asking is refused"
            );
        }
        println!("  note     {}", report::PHASE_NOTE);
    }
    Ok(match d.verdict {
        Verdict::Allow => 0,
        Verdict::Deny => 1,
        Verdict::Ask => 3,
    })
}

/// `paddock tack acl-grant`: the grant on standard input, the report as
/// JSON on standard output.
pub(crate) fn acl_grant() -> Result<i32> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin().take(64 * 1024).read_to_string(&mut text)?;
        let g: paddock::tack::protect::Grant =
            serde_json::from_str(&text).context("the grant on standard input is not valid")?;
        // SAFETY: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        match paddock::tack::acl::grant(&g, me, paddock::tack::acl::MAX_FILES) {
            Ok(rep) => {
                println!("{}", serde_json::to_string(&rep)?);
                Ok(0)
            }
            Err(e) => {
                eprintln!("{e}");
                Ok(1)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("protect = \"uid\" is for Linux only in this version")
    }
}
