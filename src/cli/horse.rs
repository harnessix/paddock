use anyhow::{bail, Context as _, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::horse::guard::{self, Op};
use paddock::horse::{self, claude, codex, dsh, Store};
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod dna;
mod registry;
mod swap;

#[derive(Subcommand)]
pub(crate) enum HorseCmd {
    /// List the sessions in the horse store.
    List {
        #[arg(long)]
        json: bool,
    },
    /// List the edges of a horse.
    Edges { horse: String },
    /// The DNA of a horse: show, set (hot-load) and history.
    Dna {
        #[command(subcommand)]
        cmd: dna::DnaCmd,
    },
    /// Published DNA: the signing key, trusted keys, indexes, resolve,
    /// publish and sync.
    Registry {
        #[command(subcommand)]
        cmd: registry::RegistryCmd,
    },
    /// Move SESSION to HORSE. The DNA of its old horse grafts into HORSE,
    /// unless the two DNAs are incompatible. The session gets the DNA of
    /// HORSE as one entry at the end.
    Move { session: String, horse: String },
    /// Combine two horses: HOST keeps its id and sessions and gains the
    /// traits of DONOR. Incompatible DNAs are refused.
    Graft { host: String, donor: String },
    /// Show where each trait of a horse comes from, its DNA events and
    /// the lineage of its sessions.
    Explain { horse: String },
    /// Show a session, its horse and its lineage.
    Show {
        session: String,
        #[arg(long)]
        json: bool,
    },
    /// The nodes of a session, root first, with the message view.
    Log {
        session: String,
        /// Show only the last N nodes.
        #[arg(short = 'n', long)]
        lines: Option<usize>,
    },
    /// Make a new session whose head is a node of SESSION.
    ///
    /// With --warm, SESSION is a Codex session that a run holds: its live
    /// thread forks ephemeral at its head, under the same cache key. The
    /// new session runs in the custody of the run, its turns stay in the
    /// store marked live, and no codec projects it. SESSION waits until
    /// the side branch closes (--close, or the end of the run).
    Fork {
        session: String,
        /// A node hash (at least 6 digits) or a native key such as seq:12.
        #[arg(long, required_unless_present_any = ["warm", "close"])]
        at: Option<String>,
        /// The horse of the new session. Default: the horse of SESSION.
        #[arg(long)]
        horse: Option<String>,
        /// A warm side branch of the live Codex thread of SESSION.
        #[arg(long, conflicts_with_all = ["at", "horse", "close"])]
        warm: bool,
        /// End the custody of the warm side branch SESSION.
        #[arg(long, conflicts_with_all = ["at", "horse"])]
        close: bool,
        /// The longest wait for the run to reach a safe point, in seconds.
        #[arg(long, default_value_t = 600)]
        wait: u64,
    },
    /// Continue a Codex SESSION at another branch under the same thread
    /// id, so the cached prefix up to the fork point stays warm. BRANCH is
    /// a node (a hash of at least 6 digits) or a session whose head is
    /// the branch head. When a run holds the session, the swap waits until
    /// no turn runs and the run loads the thread again; else the rollout
    /// under --codex-home is swapped. The old head stays in the store.
    Swap {
        session: String,
        branch: String,
        /// Stop the running turn first instead of waiting for its end.
        #[arg(long)]
        interrupt: bool,
        /// The longest wait for the running turn to end, in seconds.
        #[arg(long, default_value_t = 600)]
        wait: u64,
        /// The Codex home of a session that no run holds (it has sessions/).
        #[arg(long)]
        codex_home: Option<PathBuf>,
        /// Swap also when the prefix guard says the swap loses cache.
        #[arg(long)]
        yes: bool,
    },
    /// Queue an entry for SESSION. A run in horse mode delivers it at the
    /// next safe point (the session is idle), as the newest entry.
    Append {
        session: String,
        /// user, system, developer or note.
        #[arg(long, default_value = "user")]
        role: String,
        #[arg(long)]
        text: String,
        /// Urgent: an interactive TUI run stops the running turn with the
        /// harness's interrupt key and delivers the entry at once.
        #[arg(long)]
        interrupt: bool,
    },
    /// The live sessions of horse-mode runs, their heads, inboxes and the
    /// last verdict of the prefix guard.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Ask the prefix guard whether an operation keeps the cached prefix.
    Guard {
        session: String,
        /// A fork at this node.
        #[arg(long, value_name = "NODE")]
        fork_at: Option<String>,
        /// An entry after this node instead of at the end.
        #[arg(long, value_name = "NODE")]
        append_after: Option<String>,
        /// A switch to this model.
        #[arg(long)]
        model: Option<String>,
        /// A switch to this reasoning effort.
        #[arg(long)]
        effort: Option<String>,
        /// A change of the tool set; `add` when tools are only added.
        #[arg(long, value_parser = ["add", "change"])]
        tools: Option<String>,
        /// A change of the system prompt.
        #[arg(long)]
        system: bool,
        /// A change of the working directory.
        #[arg(long)]
        cwd: Option<String>,
        /// A move of the session to this horse.
        #[arg(long = "move", value_name = "HORSE")]
        move_to: Option<String>,
    },
    /// Read a dsh session file or session folder into the store.
    IngestDsh {
        path: PathBuf,
        /// The dsh version that wrote the session. Only the pinned version works.
        #[arg(long)]
        dsh_version: String,
        /// Put a new session into this horse. Default: a new horse.
        #[arg(long)]
        horse: Option<String>,
    },
    /// Write a session as a native dsh session file under a dsh sessions folder.
    ProjectDsh {
        session: String,
        /// The dsh sessions folder, for example $DSH_HOME/sessions.
        #[arg(long)]
        root: PathBuf,
    },
    /// Read a Codex rollout into the store.
    IngestCodex {
        path: PathBuf,
        #[arg(long)]
        codex_version: String,
        #[arg(long)]
        horse: Option<String>,
    },
    /// Write a Codex rollout under a config folder.
    ProjectCodex {
        session: String,
        #[arg(long)]
        config_dir: PathBuf,
        #[arg(long)]
        swap: bool,
    },
    /// Read a Claude Code transcript (<session id>.jsonl) into the store.
    IngestClaude {
        path: PathBuf,
        /// The Claude Code version that wrote the transcript. Only the pinned version works.
        #[arg(long)]
        claude_version: String,
        /// Put a new session into this horse. Default: a new horse.
        #[arg(long)]
        horse: Option<String>,
    },
    /// Write a session as a Claude Code transcript under a Claude Code config folder.
    ProjectClaude {
        session: String,
        /// The Claude Code config folder, for example the sandbox's ~/.claude.
        #[arg(long)]
        config_dir: PathBuf,
    },
}

pub(crate) fn run(env: &HostEnv, cmd: HorseCmd) -> Result<i32> {
    let st = Store::open(&horse::root(env))?;
    match cmd {
        HorseCmd::List { json } => {
            let all = st.sessions()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&all)?);
                return Ok(0);
            }
            for s in all {
                let n = st.session_nodes(&s)?.len();
                println!(
                    "{}  {}  {} {}  {} nodes  {}",
                    s.id,
                    s.horse,
                    s.binding.harness,
                    s.binding.native_id.as_deref().unwrap_or("(not projected)"),
                    n,
                    s.fork_of
                        .as_ref()
                        .map(|f| format!("fork of {} at {}", f.session, f.node.short()))
                        .unwrap_or_default()
                );
            }
        }
        HorseCmd::Dna { cmd } => return dna::run(&env.paddock_state(), &st, cmd),
        HorseCmd::Registry { cmd } => return registry::run(&env.paddock_state(), &st, cmd),
        HorseCmd::Move { session, horse } => {
            let m = horse::lineage::move_to(&st, &session, &horse)?;
            if let Some(v) = m.verdict.as_ref().filter(|v| !v.keeps) {
                eprintln!("paddock: {}", v.message());
            }
            println!("session {session} moved from {} to {horse}", m.from);
            if let Some(h) = m.graft {
                println!("the DNA of {} grafted into {horse}: {h}", m.from);
            }
        }
        HorseCmd::Graft { host, donor } => {
            match horse::dna::graft::graft_horses(&st, &host, &donor, "combine")? {
                Some(h) => println!("the DNA of {donor} grafted into {host}: {h}"),
                None => println!("the DNA of {host} did not change"),
            }
        }
        HorseCmd::Explain { horse } => print!("{}", horse::dna::explain::explain(&st, &horse)?),
        HorseCmd::Edges { horse } => {
            for edge in horse::hitch::edges(&st, &horse)? {
                println!("{}", serde_json::to_string(&edge)?);
            }
        }
        HorseCmd::Show { session, json } => {
            let s = st.session(&session)?;
            let h = st.horse(&s.horse)?;
            if json {
                let v = serde_json::json!({ "session": s, "horse": h });
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(0);
            }
            println!("session   {}", s.id);
            println!("horse     {} (DNA {})", h.id, h.dna.short());
            println!(
                "harness   {} {} ({})",
                s.binding.harness, s.binding.version, s.binding.container
            );
            println!(
                "native id {}",
                s.binding.native_id.as_deref().unwrap_or("(not projected)")
            );
            println!(
                "head      {}",
                s.head.as_ref().map_or("(empty)", |h| h.short())
            );
            if let Some(f) = &s.fork_of {
                println!("fork of   {} at {}", f.session, f.node.short());
            }
            for l in &s.lineage {
                println!("lineage   {} (DNA {})", l.horse, l.dna.short());
            }
        }
        HorseCmd::Log { session, lines } => {
            let s = st.session(&session)?;
            let nodes = st.session_nodes(&s)?;
            let skip = lines.map_or(0, |n| nodes.len().saturating_sub(n));
            for n in nodes.iter().skip(skip) {
                let key = n.meta.key.as_deref().unwrap_or("-");
                let mark = if n.is_live() { "  live" } else { "" };
                let line = match (s.binding.harness.as_str(), dsh::view(&n.raw)) {
                    ("dsh", Some(v)) => format!("{}: {}", v.role, v.text),
                    ("dsh", None) => dsh::row_info(&n.raw)
                        .map(|(t, _, _)| t)
                        .unwrap_or_else(|_| "?".into()),
                    ("claude", _) => match claude::view(&n.raw) {
                        Some(v) => format!("{}: {}", v.role, v.text),
                        None => claude::info(&n.raw)
                            .map(|i| i.ty)
                            .unwrap_or_else(|_| "?".into()),
                    },
                    _ => format!("{} bytes", n.raw.len()),
                };
                let line: String = line.chars().take(100).collect();
                println!(
                    "{}  {key:>8}  {}{mark}",
                    n.hash.short(),
                    line.replace('\n', " ")
                );
            }
        }
        HorseCmd::Fork {
            session,
            at,
            horse,
            warm,
            close,
            wait,
        } => {
            if warm || close {
                return side(&st, &session, close, Duration::from_secs(wait));
            }
            let at = at.unwrap_or_default();
            let s = st.session(&session)?;
            let node = st.resolve_node(&s, &at)?;
            let v = guard::check(
                &st,
                &session,
                &Op::Fork { at: node.clone() },
                horse::now_ms(),
            )?;
            st.save_verdict(&v)?;
            if !v.keeps || !v.reasons.is_empty() {
                eprintln!("paddock: {}", v.message());
            }
            let f = st.fork(&session, &node, horse.as_deref())?;
            println!("{}", f.id);
        }
        HorseCmd::Swap {
            session,
            branch,
            interrupt,
            wait,
            codex_home,
            yes,
        } => return swap::run(&st, &session, &branch, interrupt, wait, codex_home, yes),
        HorseCmd::Append {
            session,
            role,
            text,
            interrupt,
        } => {
            let e = st.queue_entry(&session, &role, &text, interrupt)?;
            let live = horse::live::records(&st)?
                .iter()
                .any(|r| r.sessions.iter().any(|x| x.session == session));
            println!("{}", e.id);
            if !live {
                eprintln!("paddock: session {session} is in no live horse-mode run; the entry waits in its inbox");
            }
        }
        HorseCmd::Status { json } => status(&st, json)?,
        HorseCmd::Guard {
            session,
            fork_at,
            append_after,
            model,
            effort,
            tools,
            system,
            cwd,
            move_to,
        } => {
            let s = st.session(&session)?;
            let node = |x: &str| st.resolve_node(&s, x);
            let mut ops = Vec::new();
            if let Some(n) = fork_at {
                ops.push(Op::Fork { at: node(&n)? });
            }
            if let Some(n) = append_after {
                ops.push(Op::Append {
                    after: Some(node(&n)?),
                });
            }
            ops.extend(model.map(Op::Model));
            ops.extend(effort.map(Op::Effort));
            ops.extend(tools.map(|t| Op::Tools {
                added_only: t == "add",
            }));
            if system {
                ops.push(Op::System);
            }
            ops.extend(cwd.map(Op::Cwd));
            ops.extend(move_to.map(|to| Op::MoveHorse { to }));
            if ops.is_empty() {
                ops.push(Op::Append { after: None });
            }
            let mut code = 0;
            for op in ops {
                let v = guard::check(&st, &session, &op, horse::now_ms())?;
                st.save_verdict(&v)?;
                println!("{}", v.message());
                if !v.keeps {
                    code = 1;
                }
            }
            return Ok(code);
        }
        HorseCmd::IngestDsh {
            path,
            dsh_version,
            horse,
        } => {
            let r = dsh::ingest(&st, &path, &dsh_version, horse.as_deref())?;
            println!(
                "{}: {} new, {} kept, {} dropped{}",
                r.session,
                r.added,
                r.kept,
                r.dropped,
                if r.torn { ", torn tail left out" } else { "" }
            );
        }
        HorseCmd::ProjectDsh { session, root } => {
            let p = dsh::project(&st, &session, &root, horse::now_ms())?;
            println!("{} {}", p.native_id, p.path.display());
        }
        HorseCmd::IngestCodex {
            path,
            codex_version,
            horse,
        } => {
            let r = codex::ingest(&st, &path, &codex_version, horse.as_deref())?;
            println!(
                "{}: {} new, {} kept{}",
                r.session,
                r.added,
                r.kept,
                if r.torn { ", torn tail left out" } else { "" }
            );
        }
        HorseCmd::ProjectCodex {
            session,
            config_dir,
            swap,
        } => {
            let p = codex::project(&st, &session, &config_dir, swap)?;
            println!("{} {}", p.native_id, p.path.display());
            if p.forked && !swap {
                println!("This fork starts with a cold cache.");
            }
        }
        HorseCmd::IngestClaude {
            path,
            claude_version,
            horse,
        } => {
            let r = claude::ingest(&st, &path, &claude_version, horse.as_deref())?;
            println!(
                "{}: {} new, {} kept{}{}",
                r.session,
                r.added,
                r.kept,
                if r.rewritten {
                    ", file was rewritten (dropped rows stay in the store)"
                } else {
                    ""
                },
                if r.torn { ", torn tail left out" } else { "" }
            );
        }
        HorseCmd::ProjectClaude {
            session,
            config_dir,
        } => {
            let p = claude::project(&st, &session, &config_dir)?;
            println!("{} {}", p.native_id, p.path.display());
        }
    }
    Ok(0)
}

fn status(st: &Store, json: bool) -> Result<()> {
    let live = horse::live::records(st)?;
    let mut rows = Vec::new();
    for r in &live {
        for x in &r.sessions {
            let s = st.session(&x.session)?;
            rows.push(serde_json::json!({
                "run": r.run,
                "horse": r.horse,
                "session": x.session,
                "native_id": x.native,
                "main": x.native == r.main,
                "state": if x.idle { "idle" } else { "running" },
                "head": s.head,
                "nodes": st.session_nodes(&s)?.len(),
                "inbox": st.inbox(&x.session)?,
                // The deliveries of this run: landed, or recorded and not landed.
                "deliveries": st.deliveries(&x.session)?.into_iter()
                    .filter(|d| d.delivered_ms >= r.started_ms)
                    .map(|d| serde_json::json!({"id": d.entry.id, "role": d.entry.role, "landed": d.landed_seq}))
                    .collect::<Vec<_>>(),
                "guard": st.guard_state(&x.session)?.last,
            }));
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no live horse-mode run");
    }
    for v in rows {
        let s = |k: &str| v[k].as_str().unwrap_or("-").to_string();
        println!(
            "{}  {}  {}  {}{}  {} nodes, head {}",
            s("run"),
            s("horse"),
            s("session"),
            s("state"),
            if v["main"] == true { ", main" } else { "" },
            v["nodes"],
            s("head").chars().take(12).collect::<String>()
        );
        for e in v["inbox"].as_array().into_iter().flatten() {
            let text: String = e["text"].as_str().unwrap_or("").chars().take(60).collect();
            println!(
                "  inbox {} {}: {}",
                e["id"].as_str().unwrap_or(""),
                e["role"].as_str().unwrap_or(""),
                text.replace('\n', " ")
            );
        }
        for d in v["deliveries"].as_array().into_iter().flatten() {
            println!(
                "  delivery {} {}: {}",
                d["id"].as_str().unwrap_or(""),
                d["role"].as_str().unwrap_or(""),
                match d["landed"].as_u64() {
                    Some(k) => format!("landed at row {k}"),
                    None => "recorded, not landed".to_string(),
                }
            );
        }
        if let Some(g) = v["guard"].as_object() {
            let keeps = g.get("keeps").and_then(|b| b.as_bool()).unwrap_or(true);
            println!(
                "  guard {}: {}",
                g.get("op").and_then(|o| o.as_str()).unwrap_or("?"),
                if keeps {
                    "keeps the prefix".to_string()
                } else {
                    format!(
                        "loses about {} tokens",
                        g.get("lost_tokens").cloned().unwrap_or_default()
                    )
                }
            );
        }
    }
    Ok(())
}

/// `paddock horse fork --warm` and `--close`: ask the run that holds the
/// Codex session to open or close its warm side branch, and wait for the
/// answer.
fn side(st: &Store, session: &str, close: bool, wait: Duration) -> Result<i32> {
    use paddock::horse::live::codex::side::request::{self, Act, State};
    let s = st.session(session)?;
    let (parent, act, side) = if close {
        let f = s
            .fork_of
            .as_ref()
            .with_context(|| format!("session {session} is no side branch: it is not a fork"))?;
        (f.session.clone(), Act::Close, Some(session))
    } else {
        if s.binding.harness != codex::HARNESS {
            bail!(
                "--warm is for Codex sessions; session {session} is a {} session: fork it with --at",
                s.binding.harness
            );
        }
        (session.to_string(), Act::Open, None)
    };
    let held = |run: Option<&str>| -> Result<Option<String>> {
        Ok(horse::live::records(st)?
            .into_iter()
            .find(|r| {
                run.is_none_or(|x| x == r.run) && r.sessions.iter().any(|x| x.session == parent)
            })
            .map(|r| r.run))
    };
    let Some(run) = held(None)? else {
        if close {
            println!("side branch {session} is in no live run: its custody ended already");
            return Ok(0);
        }
        bail!(
            "session {session} is in no live horse-mode run, so it has no live thread to fork warm; paddock horse fork {session} --at NODE makes a persisted fork, which starts cold"
        );
    };
    let r = request::ask(st, &parent, act, side)?;
    eprintln!("paddock: run {run} holds session {parent}; paddock waits until no turn runs");
    let started = Instant::now();
    let grace = Duration::from_secs(60);
    let mut checked = Instant::now();
    let answer = loop {
        if let Some(a) = request::answer(st, &parent, &r.id)? {
            break a;
        }
        if started.elapsed() > wait + grace {
            request::remove(st, &parent, &r.id)?;
            bail!(
                "run {run} did not answer within {} s; nothing changed",
                (wait + grace).as_secs()
            );
        }
        if checked.elapsed() > Duration::from_secs(1) {
            checked = Instant::now();
            // The custody may have answered just before it ended.
            if held(Some(&run))?.is_none() && request::answer(st, &parent, &r.id)?.is_none() {
                request::remove(st, &parent, &r.id)?;
                bail!("run {run} ended before it reached a safe point; nothing changed");
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    request::remove(st, &parent, &r.id)?;
    if answer.state != State::Done {
        eprintln!(
            "paddock: {}",
            answer.error.as_deref().unwrap_or("the run gave no reason")
        );
        return Ok(1);
    }
    match act {
        Act::Open => {
            if let Some(v) = &answer.verdict {
                eprintln!("paddock: {}", v.message());
            }
            eprintln!(
                "paddock: session {parent} waits until the side branch closes: paddock horse fork {} --close",
                answer.side.as_deref().unwrap_or("?")
            );
            println!("{}", answer.side.as_deref().unwrap_or_default());
        }
        Act::Close => println!(
            "side branch {session} closed after {} turns; its nodes stay in the store",
            answer.turns
        ),
    }
    Ok(0)
}
