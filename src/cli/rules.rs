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
    /// Remove a rule of the file by its id.
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
            let all = rules.all();
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
            let id = rules.add(rule, yes_all_hosts)?.id.clone();
            rules.save()?;
            println!("{}", text::rule_added(&id, &rules.path));
            Ok(0)
        }
        RulesCmd::Remove { id } => {
            rules.remove(&id, name)?;
            rules.save()?;
            println!("{}", text::rule_removed(&id, &rules.path));
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(t: &std::path::Path) -> HostEnv {
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
            rules(&env, "chatgpt-desktop", Some(RulesCmd::List)).unwrap(),
            0
        );
        let add = |host: &str, deny: bool, yes: bool| {
            rules(
                &env,
                "chatgpt-desktop",
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
        let loaded = Rules::load(&env, "chatgpt-desktop").unwrap();
        assert_eq!(loaded.file.len(), 2);
        let id = loaded.file[0].id.clone();
        assert_eq!(rules(&env, "chatgpt-desktop", None).unwrap(), 0);
        assert_eq!(
            rules(
                &env,
                "chatgpt-desktop",
                Some(RulesCmd::Remove { id: id.clone() })
            )
            .unwrap(),
            0
        );
        assert!(rules(&env, "chatgpt-desktop", Some(RulesCmd::Remove { id })).is_err());
        assert_eq!(Rules::load(&env, "chatgpt-desktop").unwrap().file.len(), 1);
        // A bad port is one line.
        let e = rules(
            &env,
            "chatgpt-desktop",
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
}
