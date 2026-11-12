//! `paddock app rules NAME [list | add | remove]`: the firewall rules of
//! a desktop app in paddock's macOS guest.

use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::macvm::firewall::{self, Port, Rules, Scope, Verdict};
use paddock::macvm::text;

#[derive(Subcommand)]
pub(crate) enum RulesCmd {
    /// List the rules of the file and the seeds of the profile.
    List,
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
        /// A rule that allows every host turns the firewall off for the app.
        #[arg(long)]
        yes_all_hosts: bool,
    },
    /// Allow a host or wildcard on port 443.
    Allow { host: String },
    /// Deny a host or wildcard on port 443.
    Deny { host: String },
    /// Remove a rule of the file by its id or host.
    Remove { id: String },
}

pub(crate) fn rules(env: &HostEnv, name: &str, cmd: Option<RulesCmd>) -> Result<i32> {
    if !paddock::macvm::host::valid_name(name) {
        bail!(text::bad_app_name(name));
    }
    let profile = paddock::app::app_profile(env, name).ok();
    let mut rules = Rules::load(env, name)?;
    if profile.is_none() && !rules.path.exists() {
        bail!(text::rules_need_macvm(name));
    }
    rules.seed(
        &firewall::seed_hosts(profile.as_ref()),
        paddock::state::now(),
    );
    match cmd.unwrap_or(RulesCmd::List) {
        RulesCmd::List => {
            let mut all = rules.all();
            let layout = paddock::macvm::layout::Layout::new(env);
            for (app, home, dir) in layout.profile_dirs() {
                if app == name && paddock::macvm::host::running(&layout, &app, &home).is_some() {
                    let path = dir.join("logs/session-rules.json");
                    eprintln!("Session rules are a potentially stale, incomplete diagnostic for {app} ({home}).");
                    if let Ok(data) = std::fs::read(path) {
                        let session = serde_json::from_slice::<serde_json::Value>(&data)
                            .ok()
                            .and_then(|v| {
                                serde_json::from_value::<Vec<firewall::Rule>>(
                                    v.get("rules").cloned().unwrap_or(v),
                                )
                                .ok()
                            });
                        if let Some(session) = session {
                            all.extend(
                                session.into_iter().filter(|r| {
                                    r.expires.is_none_or(|e| e > paddock::state::now())
                                }),
                            );
                        }
                    }
                }
            }
            if all.is_empty() {
                println!("{}", text::rules_empty(name));
                return Ok(0);
            }
            println!("{}", text::rules_header(name, &rules.path));
            for r in &all {
                println!("{}", firewall::rule_line(r));
            }
            Ok(0)
        }
        cmd @ (RulesCmd::Allow { .. } | RulesCmd::Deny { .. }) => {
            let (host, verdict) = match cmd {
                RulesCmd::Allow { host } => (host, Verdict::Allow),
                RulesCmd::Deny { host } => (host, Verdict::Deny),
                _ => unreachable!(),
            };
            let r = firewall::new_rule(
                "*",
                None,
                &host,
                Port::Num(443),
                verdict,
                Scope::Always,
                firewall::BY_USER,
                paddock::state::now(),
            );
            let added = rules.add(r, false)?.clone();
            rules.save()?;
            publish(env, name, &added, "add");
            println!("{}", text::rule_added(&added.id, &rules.path));
            Ok(0)
        }
        RulesCmd::Add {
            host,
            port,
            app,
            team,
            allow: _,
            deny,
            yes_all_hosts,
        } => {
            let verdict = if deny { Verdict::Deny } else { Verdict::Allow };
            let rule = firewall::new_rule(
                &app,
                team.as_deref(),
                &host,
                Port::parse(&port)?,
                verdict,
                Scope::Always,
                firewall::BY_USER,
                paddock::state::now(),
            );
            let added = rules.add(rule, yes_all_hosts)?.clone();
            let id = added.id.clone();
            rules.save()?;
            let target = format!("{host}:{port}");
            let value = format!("{} always", verdict.word());
            paddock::macvm::grants::note_user_rule(env, name, "add", &target, &value);
            publish(env, name, &added, "add");
            println!("{}", text::rule_added(&id, &rules.path));
            Ok(0)
        }
        RulesCmd::Remove { id } => {
            let target = rules
                .file
                .iter()
                .find(|r| r.host == id)
                .map(|r| r.id.clone())
                .unwrap_or_else(|| id.clone());
            let gone = rules.remove(&target, name)?;
            rules.save()?;
            paddock::macvm::grants::note_user_rule(env, name, "remove", &id, "-");
            publish(env, name, &gone, "remove");
            println!("{}", text::rule_removed(&id, &rules.path));
            Ok(0)
        }
    }
}

/// Ask stream consumers to refresh after an unordered rule change.
fn publish(env: &HostEnv, app: &str, r: &paddock::macvm::firewall::Rule, action: &str) {
    use paddock::eventstream::schema::GrantSource;
    paddock::eventstream::publish(
        env,
        paddock::macvm::events_stream::historical_rule_grant(app, r, action, GrantSource::You),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
            rules(&env, "sample-notes", Some(RulesCmd::List)).unwrap(),
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
                    yes_all_hosts: yes,
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
                Some(RulesCmd::Remove { id: id.clone() })
            )
            .unwrap(),
            0
        );
        assert!(rules(&env, "sample-notes", Some(RulesCmd::Remove { id })).is_err());
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
                yes_all_hosts: false,
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
                id: "*.example.com".into(),
            }),
        )
        .unwrap();
        rules(
            &env,
            name,
            Some(RulesCmd::Deny {
                host: "*.example.com".into(),
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
                host: "foo.*.example.com".into()
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
            yes_all_hosts: false,
        };
        assert_eq!(rules(&env, "sample-notes", Some(cmd)).unwrap(), 0);
        let id = Rules::load(&env, "sample-notes").unwrap().file[0]
            .id
            .clone();
        assert_eq!(
            rules(
                &env,
                "sample-notes",
                Some(RulesCmd::Remove { id: id.clone() })
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
}
