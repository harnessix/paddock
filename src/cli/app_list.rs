use anyhow::Result;
use paddock::app;
use paddock::env::HostEnv;
use paddock::profile::HarnessFactory;

pub(super) fn list(env: &HostEnv, json: bool) -> Result<i32> {
    if json {
        println!("{}", profiles_json(env));
        return Ok(0);
    }
    for line in paddock::macvm::host::list_lines(env) {
        println!("{line}");
    }
    let f = HarnessFactory::new(env)?;
    for name in f.names() {
        let Some(p) = f.profile(&name) else { continue };
        if p.app.is_none() {
            continue;
        }
        let found = match p.app.as_ref().and_then(|a| a.flatpak.clone()) {
            Some(id) => format!("flatpak {id}"),
            None => f
                .create(&name)
                .map(|h| h.exe.display().to_string())
                .unwrap_or_else(|_| "not installed".into()),
        };
        let launcher = if app::desktop_file(env, &name).exists() {
            "launcher entry"
        } else {
            "no launcher entry"
        };
        let roots = app::roots(env, &name, &[])
            .map(|r| {
                r.iter()
                    .map(|x| x.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|_| "no folders".into());
        println!(
            "{name:<16} {found}\n{:<16} {launcher}; folders: {roots}",
            ""
        );
        if app::unprotected_workspaces(env, &name).unwrap_or(false) {
            let what = if p.app.as_ref().is_some_and(|a| a.flatpak.is_some()) {
                "secret files in its folders are visible to it"
            } else {
                "has no effect: it applies only to flatpak apps"
            };
            println!("{:<16} warning: unprotected_workspaces is on: {what}", "");
        }
    }
    Ok(0)
}

/// `paddock app list --json`: every app profile of the macOS guest. A
/// profile is listed when it has a folder (it ran once), an `[apps.NAME]
/// bundle` of `paddock app add`, or a built-in `[app.macos]` bundle that
/// is installed on this Mac. Each has the status of `paddock app status
/// --json` (state, sizes, next start) for the default home.
fn profiles_json(env: &HostEnv) -> serde_json::Value {
    profiles_json_at(
        env,
        std::path::Path::new(paddock::macvm::host_app::SYSTEM_ROOT),
    )
}

fn profiles_json_at(env: &HostEnv, app_root: &std::path::Path) -> serde_json::Value {
    use paddock::macvm::{golden, host, host_app, layout::Layout, status};
    use std::collections::BTreeMap;
    let layout = Layout::new(env);
    // (name, home) -> the profile folder, when there is one.
    let mut all: BTreeMap<(String, String), Option<std::path::PathBuf>> = BTreeMap::new();
    for (name, home, dir) in layout.profile_dirs() {
        all.insert((name, home), Some(dir));
    }
    let manifest =
        paddock::manifest::Manifest::load(env, &env.home.join(".paddock-no-project")).ok();
    let cfg_of = |name: &str| {
        manifest
            .as_ref()
            .and_then(|m| m.merged.apps.get(name).cloned())
    };
    let factory = HarnessFactory::new(env).ok();
    let mut names: Vec<String> = manifest
        .as_ref()
        .map(|m| {
            m.merged
                .apps
                .iter()
                .filter(|(_, c)| c.bundle.is_some())
                .map(|(n, _)| n.clone())
                .collect()
        })
        .unwrap_or_default();
    if let Some(f) = &factory {
        names.extend(f.names());
    }
    let mut bundles = BTreeMap::new();
    // Why an added app cannot start: its bundle is gone or refused.
    let mut blocked = BTreeMap::new();
    for name in names {
        let profile = factory.as_ref().and_then(|f| f.profile(&name));
        let cfg = cfg_of(&name);
        let added = cfg.as_ref().is_some_and(|c| c.bundle.is_some());
        let found = host_app::optional_bundle(app_root, env, &name, profile, cfg.as_ref());
        if let (true, Err(e)) = (added, &found) {
            all.entry((name.clone(), host::DEFAULT_HOME.to_string()))
                .or_insert(None);
            blocked.insert(name.clone(), format!("{e:#}"));
        }
        if let Ok(Some(b)) = found {
            all.entry((name.clone(), host::DEFAULT_HOME.to_string()))
                .or_insert(None);
            bundles.insert(name, b);
        }
    }
    let profiles: Vec<_> = all
        .into_iter()
        .map(|((name, home), dir)| {
            let status = (home == host::DEFAULT_HOME)
                .then(|| {
                    status::read(env, &name, paddock::macvm::instance::FIRST)
                        .map(|i| status::build(&i))
                })
                .flatten();
            let b = bundles.get(&name);
            let launcher = b.is_some_and(|b| {
                paddock::launchers::macapp::launcher_path(env, &b.display_name).exists()
            });
            serde_json::json!({
                "name": name,
                "home": home,
                "bytes": dir.as_deref().map_or(0, golden::used_bytes_in),
                "added": cfg_of(&name).is_some_and(|c| c.bundle.is_some()),
                "bundle": b.map(|b| &b.path),
                "display_name": b.map(|b| &b.display_name),
                "version": b.map(|b| &b.version),
                "icon": b.and_then(|b| b.icon.as_ref()),
                "launcher": launcher,
                "blocked": blocked.get(&name),
                "status": status,
            })
        })
        .collect();
    serde_json::json!({"v": 1, "profiles": profiles})
}
#[cfg(test)]
mod tests {
    #[test]
    fn empty_list_is_versioned_json() {
        let t = tempfile::tempdir().unwrap();
        let env = paddock::env::HostEnv::from_parts(
            t.path().into(),
            "test".into(),
            1,
            1,
            Default::default(),
        );
        let encoded = super::profiles_json_at(&env, t.path()).to_string();
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["profiles"], serde_json::json!([]));
    }

    #[test]
    fn an_added_bundle_and_a_profile_folder_are_listed() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        let env = paddock::env::HostEnv::from_parts(
            p.join("home"),
            "test".into(),
            1,
            1,
            [
                (
                    "XDG_DATA_HOME".to_string(),
                    p.join("data").display().to_string(),
                ),
                (
                    "XDG_CONFIG_HOME".to_string(),
                    p.join("config").display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let app = p.join("Notes.app/Contents");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("Info.plist"),
            "<plist><dict><key>CFBundleName</key><string>Notes</string><key>CFBundleShortVersionString</key><string>2.0</string></dict></plist>",
        )
        .unwrap();
        std::fs::create_dir_all(p.join("config/paddock")).unwrap();
        std::fs::write(
            p.join("config/paddock/config.toml"),
            format!(
                "[apps.notes]\nbundle = \"{}\"\n",
                p.join("Notes.app").display()
            ),
        )
        .unwrap();
        let layout = paddock::macvm::layout::Layout::new(&env);
        let other = layout.profile("old-app", "work");
        std::fs::create_dir_all(&other.dir).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&super::profiles_json_at(&env, t.path()).to_string()).unwrap();
        assert_eq!(v["v"], 1);
        let all = v["profiles"].as_array().unwrap();
        let notes = all
            .iter()
            .find(|x| x["name"] == "notes")
            .expect("the added app");
        assert_eq!(notes["home"], "default");
        assert_eq!(notes["added"], true);
        assert_eq!(notes["display_name"], "Notes");
        assert_eq!(notes["version"], "2.0");
        assert_eq!(notes["launcher"], false);
        assert_eq!(notes["bytes"], 0);
        assert_eq!(notes["status"]["state"], "never_started");
        assert!(notes["status"]["next_start"]
            .as_str()
            .unwrap()
            .starts_with("cold"));
        let old = all
            .iter()
            .find(|x| x["name"] == "old-app")
            .expect("a folder");
        assert_eq!(old["home"], "work");
        assert_eq!(old["status"], serde_json::Value::Null);
        assert_eq!(old["bundle"], serde_json::Value::Null);
        assert_eq!(notes["blocked"], serde_json::Value::Null);
        // The bundle goes: the app stays listed, blocked with the line.
        std::fs::remove_dir_all(p.join("Notes.app")).unwrap();
        let v = super::profiles_json_at(&env, t.path());
        let notes = v["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == "notes")
            .unwrap()
            .clone();
        assert!(
            notes["blocked"].as_str().unwrap().contains("Notes.app"),
            "{notes}"
        );
    }
}
