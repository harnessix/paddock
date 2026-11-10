//! Startup decisions for MCP servers, independent of harness-specific rendering.
use super::Server;
use crate::manifest::{McpCfg, McpStart};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Start,
    Lazy,
    Disabled(&'static str),
    Duplicate(String),
    UnsupportedLazy,
}

/// Decide in source order. A duplicate must have the same complete definition,
/// including environment and other harness-specific settings. Distinct names
/// remain distinct because harnesses expose tools under their server names.
pub fn decide(servers: &[Server], cfg: &McpCfg, linux: bool, supports_lazy: bool) -> Vec<Decision> {
    let mut kept: Vec<&Server> = Vec::new();
    servers
        .iter()
        .map(|server| {
            let policy = cfg
                .servers
                .get(&server.name)
                .map(|p| p.start)
                .unwrap_or(McpStart::Auto);
            if policy == McpStart::Never {
                return Decision::Disabled("disabled by [mcp] policy");
            }
            if policy == McpStart::Auto && linux && apple_only(server) {
                return Decision::Disabled("Apple-only server on Linux");
            }
            if policy == McpStart::Lazy && !supports_lazy {
                return Decision::UnsupportedLazy;
            }
            if let Some(existing) = kept
                .iter()
                .find(|s| s.name == server.name && s.definition == server.definition)
            {
                return Decision::Duplicate(existing.name.clone());
            }
            kept.push(server);
            if policy == McpStart::Lazy {
                Decision::Lazy
            } else {
                Decision::Start
            }
        })
        .collect()
}

/// Refuse an explicit policy that paddock cannot enforce for this harness.
pub fn check_supported(harness: &str, servers: &[Server], cfg: &McpCfg) -> anyhow::Result<()> {
    if !matches!(harness, "codex" | "gemini") {
        if let Some(server) = servers.iter().find(|s| {
            cfg.servers
                .get(&s.name)
                .is_some_and(|p| matches!(p.start, McpStart::Never | McpStart::Lazy))
        }) {
            anyhow::bail!(
                "paddock cannot apply the MCP start policy for {} to harness {harness}",
                server.name
            );
        }
    }
    Ok(())
}

fn apple_command<'a>(command: &str, mut args: impl Iterator<Item = &'a str>) -> bool {
    let executable = command.rsplit('/').next().unwrap_or(command);
    let package = |part: &str| part == "xcodebuildmcp" || part.starts_with("xcodebuildmcp@");
    package(executable)
        || (matches!(executable, "npx" | "npm" | "pnpm" | "yarn" | "bunx") && args.any(package))
}

fn apple_only(server: &Server) -> bool {
    server.url.is_none()
        && apple_command(
            server.command.as_deref().unwrap_or(""),
            server.args.iter().map(String::as_str),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::McpServerCfg;
    fn server(name: &str, definition: serde_json::Value) -> Server {
        Server {
            name: name.into(),
            source: String::new(),
            command: definition["command"].as_str().map(str::to_owned),
            args: vec![],
            url: None,
            trusted: true,
            definition,
        }
    }
    #[test]
    fn explicit_always_overrides_platform_filter() {
        let apple = server("xcode", serde_json::json!({"command":"xcodebuildmcp"}));
        let mut cfg = McpCfg::default();
        assert_eq!(
            decide(std::slice::from_ref(&apple), &cfg, true, false),
            vec![Decision::Disabled("Apple-only server on Linux")]
        );
        cfg.servers.insert(
            "xcode".into(),
            McpServerCfg {
                start: McpStart::Always,
            },
        );
        assert_eq!(decide(&[apple], &cfg, true, false), vec![Decision::Start]);
    }

    #[test]
    fn unsupported_harness_refuses_matching_explicit_policy() {
        let server = server("a", serde_json::json!({"command":"node"}));
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "a".into(),
            McpServerCfg {
                start: McpStart::Never,
            },
        );
        assert!(check_supported("other", std::slice::from_ref(&server), &cfg).is_err());
        assert!(check_supported("codex", &[server], &cfg).is_ok());
        assert!(check_supported("other", &[], &cfg).is_ok());
    }

    #[test]
    fn deduplicates_only_the_same_name_and_definition() {
        let a = server("a", serde_json::json!({"command":"node"}));
        let alias = server("alias", a.definition.clone());
        assert_eq!(
            decide(&[a.clone(), a, alias], &McpCfg::default(), false, false),
            vec![
                Decision::Start,
                Decision::Duplicate("a".into()),
                Decision::Start
            ]
        );
    }

    #[test]
    fn policies_platform_and_full_definition() {
        let a = server("a", serde_json::json!({"command":"node", "env":{"A":"1"}}));
        let b = server("b", a.definition.clone());
        let c = server("c", serde_json::json!({"command":"node", "env":{"A":"2"}}));
        let apple = server("xcode", serde_json::json!({"command":"xcodebuildmcp"}));
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "c".into(),
            McpServerCfg {
                start: McpStart::Lazy,
            },
        );
        assert_eq!(
            decide(
                &[a.clone(), b.clone(), c.clone(), apple.clone()],
                &cfg,
                true,
                false
            ),
            vec![
                Decision::Start,
                Decision::Start,
                Decision::UnsupportedLazy,
                Decision::Disabled("Apple-only server on Linux")
            ]
        );
        assert_eq!(
            decide(&[a, b, c, apple], &cfg, false, true),
            vec![
                Decision::Start,
                Decision::Start,
                Decision::Lazy,
                Decision::Start
            ]
        );
    }
}

/// Apply supported Codex startup choices to the sandbox's private config.
/// The original host configuration is never changed.
pub fn filter_codex(home: &std::path::Path, cfg: &McpCfg, _linux: bool) -> anyhow::Result<()> {
    use anyhow::Context;
    let root = crate::safefs::Root::open(home)?;
    let path = std::path::Path::new(".codex/config.toml");
    let Some(bytes) = root.read(path, super::MAX_CONFIG)? else {
        return Ok(());
    };
    let text = String::from_utf8(bytes).context("Codex config is not UTF-8")?;
    let mut doc: toml_edit::DocumentMut = text.parse().context("invalid Codex config")?;
    let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(|s| s.as_table_like_mut())
    else {
        return Ok(());
    };
    let mut changed = false;
    for (name, item) in servers.iter_mut() {
        let Some(table) = item.as_table_like_mut() else {
            continue;
        };
        let policy = cfg
            .servers
            .get(name.get())
            .map(|p| p.start)
            .unwrap_or(McpStart::Auto);
        if policy == McpStart::Lazy {
            anyhow::bail!("paddock does not support lazy Codex MCP process startup for {name}");
        }
        if policy == McpStart::Never {
            table.insert("enabled", toml_edit::value(false));
            changed = true;
        }
    }
    if changed {
        root.write(path, doc.to_string().as_bytes(), 0o600)?;
    }
    Ok(())
}

#[cfg(test)]
mod codex_tests {
    use super::*;
    #[test]
    fn filter_uses_private_config_and_respects_choices() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".codex/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            r#"
[mcp_servers.apple]
command = "npx"
args = ["xcodebuildmcp@latest", "mcp"]
[mcp_servers.keep]
command = "xcodebuildmcp"
[mcp_servers.off]
command = "server"
[mcp_servers.unrelated]
command = "my-xcodebuildmcp-wrapper"
[mcp_servers.remote]
url = "https://example.test/mcp"
command = "xcodebuildmcp"
"#,
        )
        .unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "keep".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Always,
            },
        );
        cfg.servers.insert(
            "off".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Never,
            },
        );
        filter_codex(home.path(), &cfg, true).unwrap();
        let doc: toml::Value = std::fs::read_to_string(config).unwrap().parse().unwrap();
        assert!(doc["mcp_servers"]["apple"].get("enabled").is_none());
        assert_eq!(doc["mcp_servers"]["off"]["enabled"].as_bool(), Some(false));
        assert!(doc["mcp_servers"]["keep"].get("enabled").is_none());
        assert!(doc["mcp_servers"]["unrelated"].get("enabled").is_none());
        assert!(doc["mcp_servers"]["remote"].get("enabled").is_none());
    }
    #[test]
    fn lazy_fails_without_partially_rewriting_config() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".codex/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let original = "[mcp_servers.apple]\ncommand = \"xcodebuildmcp\"\n[mcp_servers.later]\ncommand = \"other\"\n";
        std::fs::write(&config, original).unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "later".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Lazy,
            },
        );
        assert!(filter_codex(home.path(), &cfg, true).is_err());
        assert_eq!(std::fs::read_to_string(config).unwrap(), original);
    }
    #[test]
    fn inline_tables_are_filtered() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".codex/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            "mcp_servers = { apple = { command = \"xcodebuildmcp\" } }\n",
        )
        .unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "apple".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Never,
            },
        );
        filter_codex(home.path(), &cfg, true).unwrap();
        let doc: toml::Value = std::fs::read_to_string(config).unwrap().parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["apple"]["enabled"].as_bool(),
            Some(false)
        );
    }
    #[test]
    fn symlinked_config_is_never_followed() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            outside.path(),
            "[mcp_servers.apple]\ncommand = \"xcodebuildmcp\"\n",
        )
        .unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(".codex/config.toml")).unwrap();
        assert!(filter_codex(home.path(), &McpCfg::default(), true).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.path()).unwrap(),
            "[mcp_servers.apple]\ncommand = \"xcodebuildmcp\"\n"
        );
    }
}

/// Gemini combines `mcp.excluded` across its settings scopes. Add our
/// exclusions to the private user settings without editing a project file.
pub fn filter_gemini(
    home: &std::path::Path,
    configured: &[Server],
    cfg: &McpCfg,
    linux: bool,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let decisions = decide(configured, cfg, linux, false);
    // Gemini also discovers extension servers outside the profile files. An
    // explicit never choice is safe to apply even if discovery found no entry.
    let mut names: Vec<String> = cfg
        .servers
        .iter()
        .filter(|(_, policy)| policy.start == McpStart::Never)
        .map(|(name, _)| name.clone())
        .collect();
    for (server, decision) in configured.iter().zip(decisions) {
        match decision {
            Decision::Disabled(_) => names.push(server.name.clone()),
            Decision::UnsupportedLazy => anyhow::bail!(
                "paddock does not support lazy Gemini MCP process startup for {}",
                server.name
            ),
            _ => {}
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    let root = crate::safefs::Root::open(home)?;
    let path = std::path::Path::new(".gemini/settings.json");
    let mut doc: serde_json::Value = match root.read(path, super::MAX_CONFIG)? {
        Some(bytes) => serde_json::from_slice(&bytes).context("invalid Gemini settings")?,
        None => serde_json::json!({}),
    };
    let obj = doc
        .as_object_mut()
        .context("Gemini settings must be an object")?;
    let mcp = obj.entry("mcp").or_insert_with(|| serde_json::json!({}));
    let mcp = mcp
        .as_object_mut()
        .context("Gemini mcp settings must be an object")?;
    let excluded = mcp
        .entry("excluded")
        .or_insert_with(|| serde_json::json!([]));
    let excluded = excluded
        .as_array_mut()
        .context("Gemini mcp.excluded must be an array")?;
    for name in names {
        if !excluded.iter().any(|v| v.as_str() == Some(&name)) {
            excluded.push(serde_json::Value::String(name));
        }
    }
    root.write(path, &serde_json::to_vec_pretty(&doc)?, 0o600)?;
    Ok(())
}

#[cfg(test)]
mod gemini_tests {
    use super::*;
    fn server(name: &str, command: &str) -> Server {
        Server {
            name: name.into(),
            source: String::new(),
            command: Some(command.into()),
            args: vec![],
            url: None,
            trusted: true,
            definition: serde_json::json!({"command": command}),
        }
    }
    #[test]
    fn exclusions_keep_existing_settings_and_apply_to_all_sources() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".gemini/settings.json");
        std::fs::create_dir(home.path().join(".gemini")).unwrap();
        std::fs::write(&path, r#"{"mcp":{"excluded":["existing"]},"theme":"dark"}"#).unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "off".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Never,
            },
        );
        filter_gemini(
            home.path(),
            &[
                server("apple", "xcodebuildmcp"),
                server("off", "other"),
                server("keep", "other"),
            ],
            &cfg,
            true,
        )
        .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            doc["mcp"]["excluded"],
            serde_json::json!(["existing", "off", "apple"])
        );
        assert_eq!(doc["theme"], "dark");
    }
    #[test]
    fn never_also_excludes_an_extension_server_not_in_profile_files() {
        let home = tempfile::tempdir().unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "extension".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Never,
            },
        );
        filter_gemini(home.path(), &[], &cfg, true).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(
            &std::fs::read(home.path().join(".gemini/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(doc["mcp"]["excluded"], serde_json::json!(["extension"]));
    }

    #[test]
    fn lazy_never_rewrites_settings() {
        let home = tempfile::tempdir().unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "later".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Lazy,
            },
        );
        assert!(filter_gemini(
            home.path(),
            &[server("apple", "xcodebuildmcp"), server("later", "other")],
            &cfg,
            true
        )
        .is_err());
        assert!(!home.path().join(".gemini/settings.json").exists());
    }
}

/// A secret-free explanation of startup decisions and boundary requirements.
#[derive(serde::Serialize)]
pub struct Report {
    pub name: String,
    pub reason: String,
    pub same_as: Vec<String>,
}

pub fn explain(
    env: &crate::env::HostEnv,
    profile: &crate::profile::Profile,
    project: &std::path::Path,
    manifest: &crate::manifest::Manifest,
    linux: bool,
) -> Vec<Report> {
    let servers = super::servers(env, profile, project);
    let supported = matches!(profile.name.as_str(), "codex" | "gemini");
    let decisions = decide(&servers, &manifest.mcp, linux, false);
    let mut reports: Vec<Report> = servers
        .iter()
        .zip(decisions)
        .map(|(server, decision)| {
            let same_as = servers
                .iter()
                .filter(|other| other.name != server.name && other.definition == server.definition)
                .map(|other| other.name.clone())
                .collect();
            let reason = if profile.name == "claude" {
                if manifest
                    .mcp
                    .servers
                    .get(&server.name)
                    .is_some_and(|p| matches!(p.start, McpStart::Never | McpStart::Lazy))
                {
                    "run refused: Claude MCP filtering is not supported yet".to_string()
                } else {
                    "Claude MCP filtering is not supported yet; its harness configuration applies"
                        .to_string()
                }
            } else if !supported
                && manifest
                    .mcp
                    .servers
                    .get(&server.name)
                    .is_some_and(|p| matches!(p.start, McpStart::Never | McpStart::Lazy))
            {
                "start policy is not supported by paddock for this harness".to_string()
            } else if supported && matches!(decision, Decision::Disabled(_)) {
                match decision {
                    Decision::Disabled(why) => format!("off: {why}"),
                    _ => unreachable!(),
                }
            } else if supported && decision == Decision::UnsupportedLazy {
                "off: lazy process startup is not supported by paddock for this harness".to_string()
            } else if server.definition.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                "off: disabled in the harness configuration".to_string()
            } else {
                match super::assess(env, server, manifest, project) {
                    super::Status::NeedsAllow(_) => "off: needs [mcp] allow".to_string(),
                    super::Status::NeedsPort(port) => format!("off: needs host port {port}"),
                    super::Status::NotFound(command) => format!("off: command {command} not found"),
                    super::Status::Ready { .. } | super::Status::Remote => {
                        let why = match manifest.mcp.servers.get(&server.name).map(|p| p.start) {
                            Some(McpStart::Always) => "explicit always policy",
                            _ => "default startup policy",
                        };
                        format!("eligible: {why}; harness settings can also disable it")
                    }
                }
            };
            Report {
                name: server.name.clone(),
                reason,
                same_as,
            }
        })
        .collect();
    if profile.name == "codex" {
        for (name, policy) in &manifest.mcp.servers {
            if policy.start == McpStart::Never && !servers.iter().any(|s| &s.name == name) {
                reports.push(Report {
                    name: name.clone(),
                    reason: "not found in the layers paddock reads; nested, profile, plugin and alternate account sources are not checked".to_string(),
                    same_as: Vec::new(),
                });
            }
        }
    }
    reports
}

/// Compute the Codex CLI overrides from the user and project layers. The
/// project can override individual fields, so merge entries before deciding.
pub fn codex_overrides(
    env: &crate::env::HostEnv,
    profile: &crate::profile::Profile,
    project: &std::path::Path,
    cfg: &McpCfg,
    linux: bool,
) -> anyhow::Result<Vec<String>> {
    let servers = super::servers(env, profile, project);
    let decisions = decide(&servers, cfg, linux, false);
    let mut overrides = Vec::new();
    for (server, decision) in servers.iter().zip(decisions) {
        match decision {
            Decision::Disabled(_) => {
                if server.name.is_empty()
                    || !server
                        .name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    anyhow::bail!(
                        "cannot safely apply a Codex CLI override for MCP server {:?}",
                        server.name
                    );
                }
                overrides.push(format!("mcp_servers.{}.enabled=false", server.name));
            }
            Decision::UnsupportedLazy => anyhow::bail!(
                "paddock does not support lazy Codex MCP process startup for {}",
                server.name
            ),
            _ => {}
        }
    }
    Ok(overrides)
}

/// Check this installed Codex supports the enabled override, without loading a
/// user's configuration or making a model request.
pub fn probe_codex_override(
    exe: &std::path::Path,
    env: &crate::env::HostEnv,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let home = tempfile::tempdir().context("cannot create Codex MCP probe home")?;
    std::fs::write(
        home.path().join("config.toml"),
        "[mcp_servers.paddock_probe]\ncommand = \"sh\"\nenabled = true\n",
    )?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "mcp",
        "list",
        "--json",
        "-c",
        "mcp_servers.paddock_probe.enabled=false",
    ])
    .env_clear()
    .env("HOME", home.path())
    .env("CODEX_HOME", home.path());
    if let Some(path) = env.var("PATH") {
        cmd.env("PATH", path);
    }
    let (code, out, _) = crate::broker::run_capture(&mut cmd, std::time::Duration::from_secs(15))?;
    let supported = code == 0
        && serde_json::from_str::<serde_json::Value>(&out)
            .ok()
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .any(|s| s["name"] == "paddock_probe" && s["enabled"] == false)
                })
            })
            .unwrap_or(false);
    if !supported {
        anyhow::bail!("this Codex cannot verify the MCP enabled override; update Codex or change the server policy");
    }
    Ok(())
}

#[cfg(test)]
mod codex_override_tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn project_reenable_cannot_override_exclusion() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let project = t.path().join("project");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            "[mcp_servers.apple]\ncommand='xcodebuildmcp'\n[mcp_servers.off]\ncommand='sh'\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".codex/config.toml"),
            "[mcp_servers.apple]\nenabled=true\n[mcp_servers.off]\nenabled=true\n",
        )
        .unwrap();
        let env = crate::env::HostEnv::from_parts(home, "test".into(), 1, 1, BTreeMap::new());
        let profile: crate::profile::Profile =
            toml::from_str(include_str!("../../profiles/codex.toml")).unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "off".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Never,
            },
        );
        assert_eq!(
            codex_overrides(&env, &profile, &project, &cfg, true).unwrap(),
            [
                "mcp_servers.apple.enabled=false",
                "mcp_servers.off.enabled=false"
            ]
        );
        std::fs::write(
            project.join(".codex/config.toml"),
            "[mcp_servers.apple]\ncommand='sh'\n",
        )
        .unwrap();
        assert_eq!(
            codex_overrides(&env, &profile, &project, &cfg, true).unwrap(),
            ["mcp_servers.off.enabled=false"]
        );
    }
    #[test]
    fn dotted_names_are_refused_instead_of_misaddressed() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let project = t.path().join("project");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            "[mcp_servers.\"a.b\"]\ncommand='sh'\n",
        )
        .unwrap();
        let env = crate::env::HostEnv::from_parts(home, "test".into(), 1, 1, BTreeMap::new());
        let profile: crate::profile::Profile =
            toml::from_str(include_str!("../../profiles/codex.toml")).unwrap();
        let mut cfg = McpCfg::default();
        cfg.servers.insert(
            "a.b".into(),
            crate::manifest::McpServerCfg {
                start: McpStart::Never,
            },
        );
        assert!(codex_overrides(&env, &profile, &project, &cfg, true).is_err());
    }
}
