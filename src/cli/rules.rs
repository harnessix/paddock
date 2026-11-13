//! `paddock app rules NAME [list | add | allow | deny | remove | change]`:
//! the firewall rules of a desktop app in paddock's macOS guest. With
//! `--json` each command prints the JSON of `paddock::macvm::rules_cli`.

use anyhow::Result;
use clap::{Args, Subcommand};
use paddock::env::HostEnv;
use paddock::macvm::firewall::{self, Port, Verdict};
use paddock::macvm::rules_cli::{self, ChangeSpec, Ctx, Done, Lifetime, Spec};
use paddock::macvm::text;

/// How long a rule applies: --always (default), --session, --minutes.
#[derive(Args, Debug, Clone, Default)]
pub(crate) struct LifetimeArgs {
    /// The rules file: the rule applies to every start (default).
    #[arg(long, conflicts_with_all = ["session", "minutes"])]
    always: bool,
    /// The running session only: the rule ends when the app stops.
    #[arg(long, conflicts_with = "minutes")]
    session: bool,
    /// The running session only, for N minutes (default 10).
    #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "10")]
    minutes: Option<u64>,
}

impl LifetimeArgs {
    fn get(&self) -> Option<Lifetime> {
        if self.always {
            Some(Lifetime::Always)
        } else if self.session {
            Some(Lifetime::Session)
        } else {
            self.minutes.map(Lifetime::Minutes)
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum RulesCmd {
    /// List the rules of the file, of the running session and the seeds
    /// of the profile.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add a rule for a host: `x.y`, `*.x.y` (every name under x.y), an
    /// IP address, or `*` (every host, with --yes-all-hosts).
    Add {
        host: String,
        /// The port, or `*`.
        #[arg(long, default_value = "443")]
        port: String,
        /// The bundle identifier of the process, or `*` for any process
        /// of the session.
        #[arg(long, default_value = "*")]
        app: String,
        /// The team identifier of the code signature.
        #[arg(long)]
        team: Option<String>,
        /// The verdict (default).
        #[arg(long, conflicts_with = "deny")]
        allow: bool,
        #[arg(long)]
        deny: bool,
        #[command(flatten)]
        lifetime: LifetimeArgs,
        /// Also every name under the host: a second rule for `*.HOST`.
        #[arg(long)]
        site: bool,
        /// A rule that allows every host turns the firewall off for the app.
        #[arg(long)]
        yes_all_hosts: bool,
        #[arg(long)]
        json: bool,
    },
    /// Allow a host or wildcard on port 443.
    Allow {
        host: String,
        #[command(flatten)]
        lifetime: LifetimeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Deny a host or wildcard on port 443.
    Deny {
        host: String,
        #[command(flatten)]
        lifetime: LifetimeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Remove rules of the file or of the running session by id or host.
    /// Every one must exist, or nothing changes.
    Remove {
        #[arg(required = true, value_name = "ID|HOST")]
        id: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Put a new rule in place of a rule (and of its `*.` partner of
    /// --site). A flag that is left out keeps the value of the old rule.
    Change {
        id: String,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<String>,
        #[arg(long, conflicts_with = "deny")]
        allow: bool,
        #[arg(long)]
        deny: bool,
        #[command(flatten)]
        lifetime: LifetimeArgs,
        #[arg(long)]
        site: bool,
        #[arg(long)]
        yes_all_hosts: bool,
        #[arg(long)]
        json: bool,
    },
}

fn verdict_of(allow: bool, deny: bool) -> Option<Verdict> {
    match (allow, deny) {
        (_, true) => Some(Verdict::Deny),
        (true, _) => Some(Verdict::Allow),
        _ => None,
    }
}

pub(crate) fn rules(env: &HostEnv, name: &str, cmd: Option<RulesCmd>) -> Result<i32> {
    let cmd = cmd.unwrap_or(RulesCmd::List { json: false });
    let json = match &cmd {
        RulesCmd::List { json }
        | RulesCmd::Add { json, .. }
        | RulesCmd::Allow { json, .. }
        | RulesCmd::Deny { json, .. }
        | RulesCmd::Remove { json, .. }
        | RulesCmd::Change { json, .. } => *json,
    };
    let list = matches!(cmd, RulesCmd::List { .. });
    let mut ctx = match Ctx::open(env, name) {
        Ok(c) => c,
        Err(e) if json && !list => return print_outcome(env, name, &Err(e)),
        Err(e) => return Err(e),
    };
    let r = match cmd {
        RulesCmd::List { json } => return list_cmd(&ctx, json),
        RulesCmd::Allow { host, lifetime, .. } => {
            shortcut(env, &mut ctx, host, Verdict::Allow, &lifetime)
        }
        RulesCmd::Deny { host, lifetime, .. } => {
            shortcut(env, &mut ctx, host, Verdict::Deny, &lifetime)
        }
        RulesCmd::Add {
            host,
            port,
            app,
            team,
            allow: _,
            deny,
            lifetime,
            site,
            yes_all_hosts,
            ..
        } => Port::parse(&port).and_then(|port| {
            rules_cli::add(
                env,
                &mut ctx,
                &Spec {
                    host,
                    port,
                    app,
                    team,
                    verdict: if deny { Verdict::Deny } else { Verdict::Allow },
                    lifetime: lifetime.get().unwrap_or(Lifetime::Always),
                    site,
                    yes_all_hosts,
                },
            )
        }),
        RulesCmd::Remove { id, .. } => rules_cli::remove(env, &mut ctx, &id),
        RulesCmd::Change {
            id,
            host,
            port,
            allow,
            deny,
            lifetime,
            site,
            yes_all_hosts,
            ..
        } => port
            .as_deref()
            .map(Port::parse)
            .transpose()
            .and_then(|port| {
                rules_cli::change(
                    env,
                    &mut ctx,
                    &id,
                    &ChangeSpec {
                        host,
                        port,
                        verdict: verdict_of(allow, deny),
                        lifetime: lifetime.get(),
                        site,
                        yes_all_hosts,
                    },
                )
            }),
    };
    if json {
        return print_outcome(env, name, &r);
    }
    let done = r?;
    for id in &done.removed {
        if ctx.rules.find(id).is_none() && id.starts_with("session-") {
            println!("{}", text::rule_removed_session(id, name));
        } else {
            println!("{}", text::rule_removed(id, &ctx.rules.path));
        }
    }
    for r in &done.written {
        match (r.scope, r.expires) {
            (firewall::Scope::Always, _) => {
                println!("{}", text::rule_added(&r.id, &ctx.rules.path))
            }
            (_, e) => println!(
                "{}",
                text::rule_added_session(
                    &r.id,
                    name,
                    e.map(|e| e.saturating_sub(paddock::state::now()).div_ceil(60))
                )
            ),
        }
    }
    Ok(0)
}

fn shortcut(
    env: &HostEnv,
    ctx: &mut Ctx,
    host: String,
    verdict: Verdict,
    lifetime: &LifetimeArgs,
) -> Result<Done> {
    rules_cli::add(
        env,
        ctx,
        &Spec {
            host,
            port: Port::Num(443),
            app: "*".into(),
            team: None,
            verdict,
            lifetime: lifetime.get().unwrap_or(Lifetime::Always),
            site: false,
            yes_all_hosts: false,
        },
    )
}

/// The JSON of a change: exit 0 when it applied, 1 when not.
fn print_outcome(env: &HostEnv, name: &str, r: &Result<Done>) -> Result<i32> {
    let o = rules_cli::outcome(env, name, r);
    println!("{}", serde_json::to_string_pretty(&o)?);
    Ok(if o.ok { 0 } else { 1 })
}

/// `paddock app connections NAME [--limit N] [--json]`.
pub(crate) fn connections(env: &HostEnv, name: &str, limit: usize, json: bool) -> Result<i32> {
    let c = rules_cli::connections(env, name, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&c)?);
        return Ok(0);
    }
    if c.connections.is_empty() {
        println!("{}", text::connections_empty(name));
        return Ok(0);
    }
    println!("{}", text::connections_header(name, c.running));
    for r in &c.connections {
        println!(
            "{}",
            text::connection_line(
                &r.last, &r.verdict, &r.process, &r.host, r.port, &r.rule, r.count, None
            )
        );
    }
    Ok(0)
}

fn list_cmd(ctx: &Ctx, json: bool) -> Result<i32> {
    let l = rules_cli::listing(ctx);
    if json {
        println!("{}", serde_json::to_string_pretty(&l)?);
        return Ok(0);
    }
    if let Some(w) = &l.warning {
        eprintln!("{w}");
    }
    let all = ctx.all();
    if all.is_empty() {
        println!("{}", text::rules_empty(&ctx.name));
        return Ok(0);
    }
    println!("{}", text::rules_header(&ctx.name, &ctx.rules.path));
    for r in &all {
        println!("{}", firewall::rule_line(r));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paddock::macvm::firewall::Rules;

    fn env(t: &std::path::Path) -> HostEnv {
        let profiles = t.join("config/paddock/harness.d");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("sample-notes.toml"),
            "name = \"sample-notes\"\n[app]\n[app.macos]\nseed_hosts = [\"login.example.com\"]\n",
        )
        .unwrap();
        HostEnv::from_parts(
            t.join("home"),
            "demo".into(),
            1000,
            1000,
            [(
                "XDG_CONFIG_HOME".to_string(),
                t.join("config").display().to_string(),
            )]
            .into_iter()
            .collect(),
        )
    }

    #[test]
    fn list_add_remove_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        assert!(rules(&env, "../x", None).is_err());
        assert!(
            rules(&env, "no-such-app", None).is_err(),
            "no profile and no file"
        );
        // The seeds of the profile list even before the file exists.
        assert_eq!(
            rules(&env, "sample-notes", Some(RulesCmd::List { json: false })).unwrap(),
            0
        );
        let add = |host: &str, deny: bool, yes: bool| {
            rules(
                &env,
                "sample-notes",
                Some(RulesCmd::Add {
                    host: host.into(),
                    port: "443".into(),
                    app: "*".into(),
                    team: None,
                    allow: false,
                    deny,
                    lifetime: LifetimeArgs::default(),
                    site: false,
                    yes_all_hosts: yes,
                    json: false,
                }),
            )
        };
        assert!(add("*", false, false).is_err(), "item 19");
        assert_eq!(add("telemetry.example.com", true, false).unwrap(), 0);
        assert_eq!(add("*", false, true).unwrap(), 0);
        let loaded = Rules::load(&env, "sample-notes").unwrap();
        assert_eq!(loaded.file.len(), 2);
        let id = loaded.file[0].id.clone();
        assert_eq!(rules(&env, "sample-notes", None).unwrap(), 0);
        assert_eq!(
            rules(
                &env,
                "sample-notes",
                Some(RulesCmd::Remove {
                    id: vec![id.clone()],
                    json: false,
                })
            )
            .unwrap(),
            0
        );
        assert!(rules(
            &env,
            "sample-notes",
            Some(RulesCmd::Remove {
                id: vec![id],
                json: false,
            })
        )
        .is_err());
        assert_eq!(Rules::load(&env, "sample-notes").unwrap().file.len(), 1);
        // A bad port is one line.
        let e = rules(
            &env,
            "sample-notes",
            Some(RulesCmd::Add {
                host: "a.example".into(),
                port: "x".into(),
                app: "*".into(),
                team: None,
                allow: true,
                deny: false,
                lifetime: LifetimeArgs::default(),
                site: false,
                yes_all_hosts: false,
                json: false,
            }),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("port \"x\""), "{e}");
    }

    #[test]
    fn wildcard_shortcuts_add_and_remove() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        let name = "sample-notes";
        rules(
            &env,
            name,
            Some(RulesCmd::Allow {
                host: "*.example.com".into(),
                lifetime: LifetimeArgs::default(),
                json: false,
            }),
        )
        .unwrap();
        let loaded = Rules::load(&env, name).unwrap();
        assert_eq!(loaded.file[0].host, "*.example.com");
        assert_eq!(loaded.file[0].verdict, Verdict::Allow);
        rules(
            &env,
            name,
            Some(RulesCmd::Remove {
                id: vec!["*.example.com".into()],
                json: false,
            }),
        )
        .unwrap();
        rules(
            &env,
            name,
            Some(RulesCmd::Deny {
                host: "*.example.com".into(),
                lifetime: LifetimeArgs::default(),
                json: false,
            }),
        )
        .unwrap();
        assert_eq!(
            Rules::load(&env, name).unwrap().file[0].verdict,
            Verdict::Deny
        );
        assert!(rules(
            &env,
            name,
            Some(RulesCmd::Allow {
                host: "foo.*.example.com".into(),
                lifetime: LifetimeArgs::default(),
                json: false,
            })
        )
        .is_err());
    }

    #[test]
    fn rule_changes_invalidate_stream_state() {
        use paddock::eventstream::schema::{Event, StreamState};
        let t = tempfile::tempdir().unwrap();
        let mut env = env(t.path());
        env = HostEnv::from_parts(
            env.home.clone(),
            env.user.clone(),
            env.uid,
            env.gid,
            env.vars
                .clone()
                .into_iter()
                .chain([(
                    "XDG_RUNTIME_DIR".to_string(),
                    t.path().join("run").display().to_string(),
                )])
                .collect(),
        );
        let bus = paddock::eventstream::Bus::of(&env);
        let f = paddock::eventstream::bus::Follower::bind(&bus, std::process::id()).unwrap();
        let (tx, rx) = paddock::eventstream::bus::queue();
        f.serve(tx).unwrap();
        let cmd = RulesCmd::Add {
            host: "api.example.com".into(),
            port: "443".into(),
            app: "*".into(),
            team: None,
            allow: true,
            deny: false,
            lifetime: LifetimeArgs::default(),
            site: false,
            yes_all_hosts: false,
            json: false,
        };
        assert_eq!(rules(&env, "sample-notes", Some(cmd)).unwrap(), 0);
        let id = Rules::load(&env, "sample-notes").unwrap().file[0]
            .id
            .clone();
        assert_eq!(
            rules(
                &env,
                "sample-notes",
                Some(RulesCmd::Remove {
                    id: vec![id.clone()],
                    json: false,
                })
            )
            .unwrap(),
            0
        );
        for _ in 0..2 {
            let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            let l: paddock::eventstream::Line = serde_json::from_str(&line).unwrap();
            let Event::Stream(stream) = l.event else {
                panic!("{line}")
            };
            assert_eq!(stream.state, StreamState::Lost);
        }
    }

    /// The argument lists of the launcher (`PaddockArgs.swift` and
    /// `NetworkArgs.swift`) parse, and an old form that never existed does
    /// not.
    #[test]
    fn the_launcher_forms_parse() {
        // The complete command tree needs the same stack as the main thread.
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(launcher_forms)
            .unwrap()
            .join()
            .unwrap();
    }

    fn launcher_forms() {
        use clap::Parser as _;
        let ok = |args: &[&str]| {
            let mut v = vec!["paddock"];
            v.extend_from_slice(args);
            if let Err(e) = crate::cli::Cli::try_parse_from(&v) {
                panic!("{args:?}: {e}");
            }
        };
        for scope in ["--always", "--session", "--minutes"] {
            for answer in ["--allow", "--deny"] {
                ok(&[
                    "app",
                    "rules",
                    "Sample Notes",
                    "add",
                    "a.example.com",
                    answer,
                    scope,
                    "--json",
                ]);
            }
        }
        ok(&[
            "app",
            "rules",
            "Sample Notes",
            "add",
            "a.example.com",
            "--minutes",
            "5",
            "--json",
        ]);
        ok(&[
            "app",
            "rules",
            "Sample Notes",
            "remove",
            "a.example.com",
            "--json",
        ]);
        ok(&["app", "rules", "Sample Notes", "list", "--json"]);
        ok(&[
            "app",
            "rules",
            "Sample Notes",
            "add",
            "example.org",
            "--port",
            "*",
            "--allow",
            "--session",
            "--site",
            "--json",
        ]);
        ok(&[
            "app",
            "rules",
            "Sample Notes",
            "change",
            "rule-1",
            "--host",
            "a.example.com",
            "--port",
            "443",
            "--deny",
            "--always",
            "--site",
            "--json",
        ]);
        ok(&[
            "app",
            "rules",
            "Sample Notes",
            "remove",
            "rule-1",
            "rule-1-subdomains",
            "--json",
        ]);
        ok(&[
            "app",
            "connections",
            "Sample Notes",
            "--limit",
            "200",
            "--json",
        ]);
        ok(&["app", "clipboard", "Sample Notes", "--json"]);
        ok(&["app", "clipboard", "Sample Notes", "--session", "--json"]);
        ok(&[
            "app",
            "clipboard",
            "Sample Notes",
            "--to-app",
            "on",
            "--from-app",
            "off",
        ]);
        ok(&[
            "app",
            "clipboard",
            "Sample Notes",
            "--to-app",
            "on",
            "--from-app",
            "off",
            "--session",
            "--json",
        ]);
        ok(&["app", "share", "Sample Notes", "keep", "/tmp/x", "--json"]);
        ok(&["app", "share", "Sample Notes", "keep", "/tmp/x"]);
        let bad = |args: &[&str]| {
            let mut v = vec!["paddock"];
            v.extend_from_slice(args);
            assert!(crate::cli::Cli::try_parse_from(&v).is_err(), "{args:?}");
        };
        bad(&[
            "app",
            "rules",
            "n",
            "add",
            "a.example.com",
            "--session",
            "--always",
        ]);
        bad(&[
            "app",
            "rules",
            "n",
            "add",
            "a.example.com",
            "--allow",
            "--deny",
        ]);
        bad(&["app", "rules", "n", "remove", "--json"]);
    }

    #[test]
    fn json_refusals_exit_one_and_changes_exit_zero() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        let add = |host: &str, lifetime: LifetimeArgs| {
            rules(
                &env,
                "sample-notes",
                Some(RulesCmd::Add {
                    host: host.into(),
                    port: "443".into(),
                    app: "*".into(),
                    team: None,
                    allow: true,
                    deny: false,
                    lifetime,
                    site: false,
                    yes_all_hosts: false,
                    json: true,
                }),
            )
        };
        assert_eq!(add("a.example.com", LifetimeArgs::default()).unwrap(), 0);
        assert_eq!(add("*", LifetimeArgs::default()).unwrap(), 1);
        let session = LifetimeArgs {
            session: true,
            ..Default::default()
        };
        assert_eq!(add("b.example.com", session).unwrap(), 1, "not running");
        assert_eq!(Rules::load(&env, "sample-notes").unwrap().file.len(), 1);
        let remove = |id: &str| {
            rules(
                &env,
                "sample-notes",
                Some(RulesCmd::Remove {
                    id: vec![id.into()],
                    json: true,
                }),
            )
        };
        assert_eq!(remove("nope.example.com").unwrap(), 1);
        assert_eq!(remove("a.example.com").unwrap(), 0);
        // An unknown app with --json is a refusal too, not a crash.
        assert_eq!(
            rules(
                &env,
                "no-such-app",
                Some(RulesCmd::Remove {
                    id: vec!["x".into()],
                    json: true,
                })
            )
            .unwrap(),
            1
        );
        assert_eq!(
            rules(&env, "sample-notes", Some(RulesCmd::List { json: true })).unwrap(),
            0
        );
    }
}
