//! `paddock horse import` and `paddock horse adopt`: host sessions.

use anyhow::{bail, Context as _, Result};
use paddock::env::HostEnv;
use paddock::horse::host::{
    self, adopt,
    hooks::{self, Removed},
    import,
};
use paddock::horse::Store;
use std::path::PathBuf;
use std::time::Duration;

#[derive(clap::Args)]
pub(crate) struct ImportArgs {
    /// claude, codex, grok, agy or dsh.
    harness: String,
    /// Native session ids.
    ids: Vec<String>,
    /// Every session of the harness.
    #[arg(long)]
    all: bool,
    /// Every session under this folder or file, relative to the home.
    #[arg(long = "path")]
    paths: Vec<PathBuf>,
    /// Only list the chosen sessions.
    #[arg(long)]
    list: bool,
    /// The horse of the new sessions. Default: one new horse.
    #[arg(long)]
    horse: Option<String>,
    /// The home with the sessions. Default: your home.
    #[arg(long)]
    home: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(crate) struct AdoptArgs {
    /// claude, codex, grok, agy or dsh. --off, --sync and --watch without
    /// it: every adopted harness.
    harness: Option<String>,
    /// A location to watch, below the transcript folder of the harness.
    /// Default: the whole transcript folder.
    #[arg(long = "path")]
    paths: Vec<PathBuf>,
    /// The home with the sessions. Default: your home.
    #[arg(long)]
    home: Option<PathBuf>,
    /// End the adoption.
    #[arg(long, conflicts_with_all = ["sync", "watch", "status", "paths"])]
    off: bool,
    /// Read the new and changed sessions once.
    #[arg(long, conflicts_with_all = ["watch", "status", "paths"])]
    sync: bool,
    /// Read the new and changed sessions every --interval seconds.
    #[arg(long, conflicts_with_all = ["status", "paths"])]
    watch: bool,
    #[arg(long, default_value_t = 30)]
    interval: u64,
    /// Show the adopted harnesses.
    #[arg(long, conflicts_with_all = ["paths"])]
    status: bool,
    /// Do not add paddock's hook entries to the host configuration of the
    /// harness. New sessions then come in at --sync and --watch only.
    #[arg(long, conflicts_with_all = ["off", "sync", "watch", "status"])]
    no_hooks: bool,
}

#[derive(clap::Args)]
pub(crate) struct HookArgs {
    pub(crate) harness: String,
    pub(crate) event: String,
    /// The marker of paddock's own hook entries.
    #[arg(long = "mark")]
    _mark: Option<String>,
}

/// The notes of `paddock horse list`: the fork and the host origin.
pub(crate) fn list_notes(s: &paddock::horse::Session) -> String {
    let mut notes: Vec<String> = s
        .fork_of
        .as_ref()
        .map(|f| format!("fork of {} at {}", f.session, f.node.short()))
        .into_iter()
        .collect();
    if let Some(h) = &s.host {
        notes.push(format!("host session ({}), ~/{}", h.via, h.path.display()));
    }
    notes.join(", ")
}

pub(crate) fn import(env: &HostEnv, st: &Store, a: ImportArgs) -> Result<i32> {
    let ImportArgs {
        harness,
        ids,
        all,
        paths,
        list,
        horse,
        home,
    } = a;
    let harness = harness.as_str();
    let home = host::canon_home(&home.unwrap_or_else(|| env.home.clone()));
    let select = match (all, paths.is_empty(), ids.is_empty()) {
        (true, true, true) => import::Select::All,
        (false, false, true) => import::Select::Under(paths),
        (false, true, false) => import::Select::Ids(ids),
        (false, true, true) if list => import::Select::All,
        _ => bail!("choose the sessions: give native ids, or --path, or --all (one of them)"),
    };
    let (found, missing) = import::choose(&home, harness, &select)?;
    for m in &missing {
        eprintln!(
            "paddock: no {harness} session {m} under ~/{}",
            host::root(harness)?
        );
    }
    if list {
        for f in &found {
            println!("{}", f.rel.display());
        }
        return Ok(i32::from(!missing.is_empty()));
    }
    let staging = env.paddock_state().join("horse").join("staging");
    let r = import::import(
        st,
        &staging,
        harness,
        &home,
        &found,
        horse.as_deref(),
        "import",
    )?;
    for i in &r.imported {
        println!(
            "{}\t{}\t{} {}\t{} row(s){}",
            i.session,
            i.horse,
            harness,
            i.native_id,
            i.rows,
            if i.new { ", new" } else { "" }
        );
    }
    for (rel, why) in &r.failed {
        eprintln!("paddock: {} did not come in: {why}", rel.display());
    }
    if found.is_empty() {
        eprintln!("paddock: no {harness} session to import");
    }
    Ok(i32::from(!r.failed.is_empty() || !missing.is_empty()))
}

fn print_swept(s: &adopt::Swept) {
    let new = s.imported.iter().filter(|i| i.new).count();
    println!(
        "{}: {} new session(s), {} changed, {} unchanged, {} not readable",
        s.harness,
        new,
        s.imported.len() - new,
        s.unchanged,
        s.failed.len()
    );
    for (rel, why) in &s.failed {
        eprintln!("paddock: {} did not come in: {why}", rel.display());
    }
    for w in &s.warnings {
        eprintln!("paddock: warning: {w}");
    }
}

pub(crate) fn adopt(env: &HostEnv, st: &Store, a: AdoptArgs) -> Result<i32> {
    let AdoptArgs {
        harness,
        paths,
        home,
        off,
        sync,
        watch,
        interval,
        status,
        no_hooks,
    } = a;
    let state = env.paddock_state();
    if status {
        let s = adopt::load(&state)?;
        println!(
            "state: {}",
            adopt::dir(&state).join("adoption.json").display()
        );
        if s.harnesses.is_empty() {
            println!("no harness is adopted");
        }
        for a in s.harnesses.values() {
            println!(
                "{}\thorse {}\t{} session(s)\t{} not readable\thome {}",
                a.harness,
                a.horse,
                a.seen.len(),
                a.errors.len(),
                a.home.display()
            );
            for l in &a.locations {
                println!("  ~/{}", l.display());
            }
            match &a.hooks {
                Some(h) => println!(
                    "  hooks: {} entr{} in ~/{}",
                    h.entries.len(),
                    if h.entries.len() == 1 { "y" } else { "ies" },
                    h.file.display()
                ),
                None => println!("  hooks: none (new sessions come in at --sync and --watch)"),
            }
            // The record says what paddock put in; the file says what is
            // there now.
            if let Some(h) = &a.hooks {
                match hooks::missing(&a.home, h) {
                    Ok(m) if m.is_empty() => {}
                    Ok(m) => println!(
                        "  {} of the hook entries are not in ~/{}: `paddock horse adopt {}` puts them in again",
                        m.len(),
                        h.file.display(),
                        a.harness
                    ),
                    Err(e) => println!(
                        "  paddock cannot read its hook entries in ~/{}: {e:#}",
                        h.file.display()
                    ),
                }
            }
            if let Some(h) = a.hooks.as_ref().filter(|h| h.program_missing()) {
                println!(
                    "  the hook program {} is gone: each turn shows a hook error. `paddock horse adopt {}` puts the entries in again",
                    h.program.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                    a.harness
                );
            }
        }
        return Ok(0);
    }
    if off {
        let names: Vec<String> = match harness {
            Some(h) => vec![h],
            None => adopt::load(&state)?.harnesses.into_keys().collect(),
        };
        for h in names {
            match adopt::off(&state, &h)? {
                Some((a, removed)) => {
                    let hooks = match removed {
                        Some(Removed::Restored) => {
                            "the hook file is back as it was before the adoption"
                        }
                        Some(Removed::Stripped) => {
                            "paddock's hook entries are out; your other entries stay"
                        }
                        Some(Removed::Deleted) => "the hook file that paddock made is gone",
                        Some(Removed::Nothing) => "the hook file had none of paddock's entries",
                        None => "it had no hook entries",
                    };
                    println!(
                        "{h}: adoption is off; its sessions stay in horse {}; {hooks}",
                        a.horse
                    )
                }
                None => println!("{h} is not adopted"),
            }
        }
        return Ok(0);
    }
    if sync || watch {
        loop {
            for s in adopt::sweep(st, &state, harness.as_deref())? {
                print_swept(&s);
            }
            if !watch {
                return Ok(0);
            }
            std::thread::sleep(Duration::from_secs(interval.max(1)));
        }
    }
    let Some(harness) = harness else {
        bail!("name the harness to adopt, for example `paddock horse adopt claude`");
    };
    let home = home.unwrap_or_else(|| env.home.clone());
    let exe = if no_hooks {
        None
    } else {
        let exe =
            std::env::current_exe().context("cannot find the paddock program for the hooks")?;
        Some(hooks::stable_program(&exe, &env.path))
    };
    let s = adopt::adopt(st, &state, &harness, &home, &paths, exe.as_deref())?;
    print_swept(&s);
    let a = adopt::load(&state)?.harnesses.remove(&harness);
    if let Some(a) = a {
        if let Some(h) = &a.hooks {
            println!(
                "{harness}: paddock added {} hook entr{} to ~/{}{}",
                h.entries.len(),
                if h.entries.len() == 1 { "y" } else { "ies" },
                h.file.display(),
                match &h.backup {
                    Some(b) => format!("; the file before the change is in {}", b.display()),
                    None => String::new(),
                }
            );
            if harness == "codex" {
                println!("codex: Codex asks you to trust the new hook entries at its next start");
            }
        }
        println!(
            "{harness}: adopted into horse {}; `paddock horse adopt --off {harness}` ends it",
            a.horse
        );
    }
    Ok(0)
}

/// The host hook: never an error for the harness. Its output goes to
/// stdout, its errors to the hook log.
pub(crate) fn hook(env: &HostEnv, harness: &str, event: &str) -> i32 {
    use std::io::Read as _;
    let state = env.paddock_state();
    let mut input = Vec::new();
    let _ = std::io::stdin().take(1 << 20).read_to_end(&mut input);
    let mut out = std::io::stdout().lock();
    let r = Store::open_host(env).and_then(|st| {
        host::hookcmd::run(
            &st,
            &state,
            harness,
            event,
            &input,
            &adopt::Bounds::default(),
            &mut out,
        )
    });
    if let Err(e) = r {
        host::hookcmd::log_error(&state, harness, event, &e);
    }
    0
}

/// What `paddock horse append` says of a session that no run holds.
pub(crate) fn append_note(env: &HostEnv, st: &Store, session: &str) -> String {
    let s = st.session(session).ok();
    let Some(h) = s.as_ref().filter(|s| s.host.is_some()) else {
        return format!(
            "session {session} is in no live horse-mode run; the entry waits in its inbox"
        );
    };
    let harness = &h.binding.harness;
    let hooked = adopt::load(&env.paddock_state())
        .ok()
        .and_then(|a| a.harnesses.get(harness.as_str()).map(|a| a.hooks.is_some()))
        .unwrap_or(false);
    if harness == "claude" && hooked {
        format!("session {session} is a host session; the entry goes out when its turn ends, or with your next prompt")
    } else {
        format!("session {session} is a host session of {harness} that paddock cannot deliver to; the entry waits in its inbox")
    }
}
