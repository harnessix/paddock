use anyhow::{bail, Result};
use clap::Parser;
use paddock::build::{self, Options};
use paddock::env::HostEnv;
use paddock::manifest::Manifest;
use paddock::profile::HarnessFactory;
use paddock::run;
use paddock::spec::{MountKind, Network};
use paddock::state;
use std::path::{Path, PathBuf};

use super::{RunArgs, RunArgsCli};

pub(crate) const WRAP_HELP: &str = "Examples:
  paddock wrap -- codex --approve-for-me
  paddock wrap -- env -u CLAUDE_CONFIG_DIR OMARCHY_AGENT_CLAUDE_HOME= claude --permission-mode auto

An `env` prefix keeps its variables. CLAUDE_CONFIG_DIR, CODEX_HOME and GROK_HOME choose the
account folder that the sandbox logs in with. In the home folder, the agent works in a scratch
project under ~/.local/share/paddock/scratch. PADDOCK_BYPASS=1 runs the command without a sandbox.";

impl RunArgs {
    /// `--manual` (or `--no-yolo`) and `--auto`. None: the configuration
    /// decides.
    pub(crate) fn approvals(&self) -> Option<paddock::approvals::Approvals> {
        use paddock::approvals::Approvals;
        match (self.manual, self.auto) {
            (true, _) => Some(Approvals::Manual),
            (_, true) => Some(Approvals::Auto),
            _ => None,
        }
    }

    fn options(&self, env: &HostEnv, shell: bool) -> Result<Options> {
        let mut harness = self.harness.clone();
        let mut path = self.path.clone();
        // `paddock ./dir` with a default harness in the manifest.
        if let Some(h) = &harness {
            if path.is_none() && (h.contains('/') || h == ".") {
                path = Some(PathBuf::from(h));
                harness = None;
            }
        }
        let harness = match harness {
            Some(h) => h,
            None => {
                let project = build::resolve_project(path.as_deref())?;
                let loaded = Manifest::load(env, &project)?;
                match loaded.merged.default_harness {
                    Some(h) => h,
                    None => bail!("name a harness, for example `paddock claude`, or set default_harness in paddock.toml"),
                }
            }
        };
        Ok(Options {
            harness,
            project: path,
            backend: self.backend.clone(),
            network: self.network,
            gui: self.gui,
            gpu: self.gpu,
            no_yolo: false,
            approvals: self.approvals(),
            worktree: self.worktree,
            worktree_app: None,
            allow_domains: self.allow.clone(),
            app_bus: None,
            inherit_env: self.env.as_deref().map(|e| e == "inherit"),
            auth: self.auth.clone(),
            shell,
            assume_yes: self.yes,
            args: self.args.clone(),
            child: None,
            transparent: None,
            pasture: self.pasture.clone(),
            devices: self.device.clone(),
            workbench: false,
            detach: None,
            profile_default: self.profile.as_deref() == Some(paddock::shadow::DEFAULT),
        })
    }
}

/// `paddock run --detach`: start the run in a session of paddock's tmux
/// server and return. Prints the sandbox id and how to take over.
pub(crate) fn start_detached(env: &HostEnv, a: &RunArgs) -> Result<i32> {
    use anyhow::Context as _;
    let opts = a.options(env, false)?;
    let factory = HarnessFactory::new(env)?;
    let Some(profile) = factory.profile(&opts.harness) else {
        bail!("unknown harness '{}'", opts.harness)
    };
    if profile.detach.is_none() {
        bail!(
            "the {} profile has no [detach] table, so paddock does not know how to give its interactive interface a first prompt. Run it without --detach, or add [detach] to the profile",
            opts.harness
        );
    }
    let prompt: Vec<u8> = match (&a.prompt_file, &a.prompt) {
        (Some(f), None) => {
            let len = std::fs::metadata(f)
                .with_context(|| format!("cannot read {}", f.display()))?
                .len();
            if len > paddock::detach::MAX_PROMPT {
                bail!("{} is larger than 1 MiB", f.display());
            }
            std::fs::read(f).with_context(|| format!("cannot read {}", f.display()))?
        }
        (None, Some(t)) => t.clone().into_bytes(),
        _ => bail!("--detach needs the task: --prompt-file FILE or --prompt TEXT"),
    };
    if prompt.iter().all(u8::is_ascii_whitespace) {
        bail!("the prompt is empty");
    }
    let Some(tmux) = paddock::view::Tmux::find(env) else {
        bail!("--detach needs tmux on PATH: paddock runs the harness in a tmux session of its own. Install tmux, or run without --detach")
    };
    let project = build::resolve_project(opts.project.as_deref())?;
    let id = state::new_id(&opts.harness, &project);
    let job = paddock::detach::Job {
        id: id.clone(),
        cwd: std::env::current_dir()?,
        env: paddock::detach::caller_env(),
        run: serde_json::to_value(a)?,
    };
    // The mailbox exists before the pane starts, so a watch can begin now.
    paddock::mail::create(env, &id, &paddock::mail::Starter::host())?;
    // `paddock board` maps the run to its ticket file.
    let file = a
        .prompt_file
        .as_ref()
        .map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone()));
    paddock::board::origin::note(env, &id, file.as_deref(), &prompt, None);
    if let Err(e) = paddock::detach::launch(env, &tmux, &job, &opts.harness, &project, &prompt) {
        paddock::board::origin::forget(env, &id);
        let _ = std::fs::remove_dir_all(paddock::mail::dir(env, &id)?);
        return Err(e);
    }
    println!("{id}");
    println!("paddock view {} --write", paddock::ids::short(&id));
    println!("mail: paddock mail watch {}", paddock::ids::short(&id));
    Ok(0)
}

/// The pane of a detached run: `paddock run` as the user typed it, with
/// the id and the prompt of the job.
pub(crate) fn detach_run(job: &std::path::Path) -> Result<i32> {
    use std::io::Write as _;
    let (job, prompt) = paddock::detach::take_job(job)?;
    let env = HostEnv::detect()?;
    paddock::privfs::secure_roots(&env);
    let id = job.id.clone();
    let _ = paddock::detach::Record::update(&env, &id, |r| r.pid = Some(std::process::id()));
    let code = (|| -> Result<i32> {
        let a: RunArgs = serde_json::from_value(job.run)?;
        start_with(&env, &a, false, Some((id.clone(), prompt)))
    })();
    let code = match code {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "paddock: {e:#}");
            2
        }
    };
    // The run writes its end itself; this covers a run that never started.
    // The end event goes first: `paddock wait` reads an ended record
    // without an end event as a run that lost its end.
    paddock::mail::events::ended(&env, &id, Some(code));
    let _ = paddock::detach::Record::update(&env, &id, |r| {
        r.ended = Some(state::now());
        r.exit_code = Some(code);
    });
    let _ = writeln!(
        std::io::stderr(),
        "\npaddock: the sandbox ended at {} with exit code {code}.",
        paddock::view::local_time(state::now())
    );
    Ok(code)
}

fn start(env: &HostEnv, a: &RunArgs, shell: bool) -> Result<i32> {
    if a.detach {
        if shell {
            bail!("--detach is for `paddock run`, not for `paddock shell`");
        }
        return start_detached(env, a);
    }
    start_with(env, a, shell, None)
}

/// A run. `detach`: the id and the prompt of a detached run (the pane).
fn start_with(
    env: &HostEnv,
    a: &RunArgs,
    shell: bool,
    detach: Option<(String, Vec<u8>)>,
) -> Result<i32> {
    let horse =
        !shell && (a.horse || a.horse_id.is_some() || paddock::horse::live::settings(env)?.enabled);
    let mut opts = a.options(env, shell)?;
    opts.detach = detach.as_ref().map(|(id, _)| id.clone());
    let project = build::resolve_project(opts.project.as_deref()).ok();
    paddock::storage::guard::before_run(env, project.as_deref(), false)?;
    let mut built = build::build(env, &opts)?;
    built.horse = horse;
    // The VM backend starts the same run with paddock in the guest.
    built.spec.intent = paddock::spec::Intent {
        shell,
        // The guest has no configuration of the user: it gets the mode.
        no_yolo: built.approvals != Some(paddock::approvals::Approvals::Auto),
        args: opts.args.clone(),
        horse_id: a.horse_id.clone(),
    };
    // The first prompt: a read-only copy where the [oneshot] prompt goes.
    let prompt_home = match &detach {
        Some((_, prompt)) => {
            let home = built.spec.home_src.clone();
            let id = built.spec.id.clone();
            let prompt = paddock::mail::text::with_reminder(prompt);
            paddock::spawn::write_prompt(&home, &id, &prompt, 0o400)?;
            Some((home, id))
        }
        None => None,
    };
    let code = start_built(env, built, shell);
    if let Some((home, id)) = prompt_home {
        paddock::spawn::remove_prompt(&home, &id);
    }
    code
}

fn start_built(env: &HostEnv, built: build::Built, shell: bool) -> Result<i32> {
    if !shell && built.approvals == Some(paddock::approvals::Approvals::Auto) {
        if let Err(e) = run::probe_flags(env, &built) {
            if let Some(w) = &built.worktree {
                build::discard_worktree(env, w);
            }
            paddock::shadow::discard(&built.spec);
            return Err(e);
        }
    }
    let backend = match run::select_backend(env, &built) {
        Ok(b) => b,
        Err(e) => {
            if let Some(w) = &built.worktree {
                build::discard_worktree(env, w);
            }
            paddock::shadow::discard(&built.spec);
            return Err(e);
        }
    };
    let wt = built.worktree.clone();
    let code = run::run(env, built, backend.as_ref(), false, false)?.code;
    if let Some(w) = wt {
        eprintln!("\n{}", paddock::worktree::finish(env, &w).0);
    }
    Ok(code)
}

/// The approvals line of `paddock explain`, in plain words.
fn explain_approvals(a: Option<paddock::approvals::Approvals>) -> &'static str {
    use paddock::approvals::Approvals;
    match a {
        Some(Approvals::Auto) => {
            "auto: the agent approves its own steps; the sandbox is the boundary"
        }
        Some(Approvals::Manual) => "manual: you approve each step in the agent's own interface",
        None => "the command's own flags decide; paddock adds none",
    }
}

pub(crate) fn explain(env: &HostEnv, a: &RunArgs, json: bool) -> Result<()> {
    let opts = a.options(env, false)?;
    let built = build::build(env, &opts)?;
    let backend = run::select_backend(env, &built);
    let s = &built.spec;
    let backend_name = backend.as_ref().map(|b| b.name()).unwrap_or_else(|_| {
        if built.backend == "auto" && s.network == Network::Allowlist {
            "srt"
        } else {
            &built.backend
        }
    });
    let cache_note = match backend_name {
        "srt" => Some("srt cannot remap package caches: caches stay in its sandbox home and [cache] share does not apply."),
        "seatbelt" => Some("Seatbelt cannot remap package caches: caches stay in its sandbox home and [cache] share does not apply. Seatbelt can also reject cache overlays."),
        _ => None,
    };
    let linux = cfg!(target_os = "linux")
        || backend
            .as_ref()
            .is_ok_and(|b| matches!(b.name(), "vm" | "docker" | "podman"));
    let mcp = paddock::mcp::diet::explain(env, &built.profile, &s.project, &built.manifest, linux);
    let cleanup = || {
        let _ = std::fs::remove_dir_all(env.paddock_run().join(&s.id));
        paddock::sockpath::remove_run_sockets(&env.paddock_run(), env.uid, &s.id);
        paddock::shadow::discard(s);
        paddock::borrow::end(env, &s.id);
        if let Some(w) = &built.worktree {
            build::discard_worktree(env, w);
        }
    };
    if json {
        let mut v = serde_json::to_value(s)?;
        if let Some(obj) = v.get_mut("env").and_then(|e| e.as_object_mut()) {
            for (_, var) in obj.iter_mut() {
                if var["secret"] == true {
                    var["value"] = "(hidden)".into();
                }
            }
        }
        v["mcp"] = serde_json::to_value(&mcp)?;
        if built.profile.name == "codex" {
            v["mcp_unfiltered_sources"] = serde_json::json!([
                "nested --cd project layers",
                "selected Codex profile files",
                "plugin configuration",
                "alternate Codex account config"
            ]);
        }
        if let Some(note) = cache_note {
            v["cache_backend_note"] = note.into();
        }
        v["approvals"] = paddock::approvals::shown(built.approvals).into();
        v["approvals_origin"] = built.approvals_origin.clone().into();
        v["screens"] = paddock::screens::explain(&built.profile).into();
        v["tack"] = paddock::tack::report::explain(env, &s.env).into();
        v["backend"] = match &backend {
            Ok(b) => b.name().into(),
            Err(e) => format!("unavailable: {e}").into(),
        };
        println!("{}", serde_json::to_string_pretty(&v)?);
        cleanup();
        return Ok(());
    }
    println!("harness   {}  ({})", s.harness, built.harness_exe.display());
    println!("project   {}", s.project.display());
    println!(
        "home      {}  (from {})",
        s.home.display(),
        s.home_src.display()
    );
    match &backend {
        Ok(b) => println!("backend   {}", b.name()),
        Err(e) => println!("backend   unavailable: {e}"),
    }
    if let Some(note) = cache_note {
        println!("cache     {note}");
    }
    if let Some(line) =
        paddock::run::memory_scope::explain_line(s.memory_high, cfg!(target_os = "macos"))
    {
        println!("memory    {line}");
    }
    println!(
        "network   {:?}{}",
        s.network,
        if s.publish.is_empty() {
            String::new()
        } else {
            format!(", publish {:?}", s.publish)
        }
    );
    if matches!(s.network, Network::Api | Network::Allowlist) {
        if s.allow_domains.is_empty() {
            println!("          no allowed hosts: every connection fails");
        }
        for d in &s.allow_domains {
            let from = s.allow_origins.get(d).map(String::as_str).unwrap_or("");
            println!("  allow  {d}\n         {from}");
        }
    }
    println!(
        "display   {:?}  ({}){}",
        s.gui.mode,
        s.gui.origin,
        if s.gui.gpu { ", gpu" } else { "" }
    );
    println!(
        "approvals {}  ({})",
        explain_approvals(built.approvals),
        built.approvals_origin
    );
    if !mcp.is_empty() {
        println!("\nMCP servers:");
        for server in &mcp {
            println!("  {}  {}", server.name, server.reason);
            if !server.same_as.is_empty() {
                println!(
                    "       same configuration as {}; names remain separate",
                    server.same_as.join(", ")
                );
            }
        }
    }
    if built.profile.name == "codex" {
        println!("  MCP platform filtering does not inspect nested --cd layers, selected profile files, plugin configuration, or alternate account config.");
    }
    println!("\nmounts (everything else on the host is absent):");
    for m in s.ordered_mounts() {
        let kind = match m.kind {
            MountKind::Ro => "ro    ",
            MountKind::Rw => "rw    ",
            MountKind::Overlay => "layers",
            MountKind::Tmpfs => "tmpfs ",
            MountKind::MaskFile | MountKind::MaskDir => "hidden",
        };
        let src = match &m.src {
            Some(src) if *src != m.dst => format!("  <- {}", src.display()),
            _ => String::new(),
        };
        println!("  {kind} {}{src}\n         {}", m.dst.display(), m.origin);
    }
    for t in &s.host_tools {
        println!(
            "  tool   {}/{}  <- {}\n         {}",
            paddock::spec::TOOL_DIR,
            t.name,
            t.src.display(),
            t.origin
        );
    }
    println!("\nenvironment (besides HOME, PATH, TERM and locale):");
    for (k, v) in &s.env {
        let val = if v.secret {
            "(hidden)".to_string()
        } else {
            v.value.clone()
        };
        println!("  {k}={val}\n         {}", v.origin);
    }
    let tack = paddock::tack::report::explain(env, &s.env);
    if !tack.is_empty() {
        println!("\ntack (secrets that only allowlisted programs receive):");
        for l in tack {
            println!("{l}");
        }
    }
    if !s.home_settings.is_empty() {
        println!("\nsettings written into the sandbox home (the host files stay as they are):");
        for h in &s.home_settings {
            println!(
                "  ~/{}  {} = {}\n         {}",
                h.file.display(),
                h.path.join("."),
                h.value,
                h.origin
            );
        }
    }
    println!("\nprivate paths (the loft keeps them out of git):");
    for l in paddock::loft::host::explain(env, &s.project) {
        println!("{l}");
    }
    let hooks = paddock::hooks::explain(s);
    if !hooks.is_empty() {
        println!("\nhooks in the sandbox copies (the host files stay as they are):");
        for l in hooks {
            println!("{l}");
        }
    }
    let screens = paddock::screens::explain(&built.profile);
    if !screens.is_empty() {
        println!(
            "\nstart screens that paddock answers, and errors it names (`paddock help host-requests`):"
        );
        for l in screens {
            println!("{l}");
        }
    }
    if !s.setup.is_empty() {
        println!("\nsetup inside the sandbox:");
        for l in &s.setup {
            println!("  {}", l.lines().next().unwrap_or_default());
        }
    }
    if !s.mise_shared.is_empty() {
        println!(
            "\nshared install step (no project): mise install {}",
            s.mise_shared.join(" ")
        );
    }
    if !s.provision.is_empty() {
        println!("\nprovision step inside the sandbox:");
        for l in &s.provision {
            println!("  {l}");
        }
    }
    println!(
        "\ncommand   {}",
        s.command
            .iter()
            .map(|c| if c.len() > 60 {
                format!("{}…", &c[..60])
            } else {
                c.clone()
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    for w in &s.warnings {
        println!("warning   {w}");
    }
    if built.horse {
        println!("\n{}", horse_block(env, &built));
    }
    cleanup();
    Ok(())
}

/// The horse of a run in horse mode, for `paddock explain`.
fn horse_block(env: &HostEnv, built: &build::Built) -> String {
    use paddock::horse::{dna::explain, run::by_sandbox, Store};
    let explained = |horse: &str| {
        Store::open_host(env)
            .and_then(|st| explain::explain(&st, horse))
            .unwrap_or_else(|e| format!("cannot explain horse {horse}: {e:#}"))
    };
    let spawn = built.spawn.as_ref();
    if let Some(h) = spawn.and_then(|c| c.hitch.as_ref()) {
        let horse = h.run().horse;
        return format!(
            "horse mode: this run joins horse {horse} (hitched)\n{}",
            explained(&horse)
        );
    }
    let parent = spawn.and_then(|c| {
        let st = Store::open_host(env).ok()?;
        by_sandbox::find(&st, &c.parent).ok().flatten()
    });
    match parent {
        Some(p) => format!(
            "horse mode: this run gets a child horse of {} that inherits its DNA\n{}",
            p.horse,
            explained(&p.horse)
        ),
        None => "horse mode: this run gets a new horse with the default DNA (no traits)".into(),
    }
}

pub(crate) fn run(env: &HostEnv, a: RunArgs) -> Result<i32> {
    start(env, &a, false)
}

pub(crate) fn shell(env: &HostEnv, a: RunArgs) -> Result<i32> {
    start(env, &a, true)
}

pub(crate) fn wrap(env: HostEnv, command: Vec<String>) -> Result<i32> {
    paddock::wrap::main(env, &command)
}

pub(crate) fn external(env: &HostEnv, argv: Vec<String>) -> Result<i32> {
    let mut full = vec!["paddock run".to_string()];
    full.extend(argv);
    let a = <RunArgsCli as Parser>::try_parse_from(full).unwrap_or_else(|e| e.exit());
    start(env, &a.run, false)
}

pub(crate) fn spawn_child(env: &HostEnv, job: PathBuf) -> Result<i32> {
    paddock::spawn::child_main(env, &job)
}

pub(crate) fn relay(
    listen: &str,
    socket: &Path,
    forward: &[u16],
    ready_file: Option<&Path>,
) -> Result<i32> {
    paddock::proxy::relay(listen, socket, forward, ready_file)?;
    Ok(0)
}
