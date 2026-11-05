use anyhow::Result;
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::tack::{config, import, report, store};
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
}

pub(crate) fn run(env: &HostEnv, cmd: TackCmd) -> Result<i32> {
    match cmd {
        TackCmd::List { json } => list(env, json),
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
