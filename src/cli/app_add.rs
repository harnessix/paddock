//! `paddock app add PATH` and `paddock app remove NAME`: an app bundle of
//! this Mac becomes an app profile of the macOS guest, and goes again.
//! `--dry-run --json` is the precheck of the launcher's Add sheet: the
//! bundle, the App Store refusal, the golden state and the space need as
//! fields.

use anyhow::{Context, Result};
use paddock::env::HostEnv;
use paddock::macvm::appcopy;
use paddock::macvm::golden;
use paddock::macvm::host;
use paddock::macvm::layout::Layout;
use paddock::macvm::manage::GoldenView;
use paddock::macvm::text;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug, Clone)]
pub(crate) struct AddArgs {
    /// The app bundle, for example /Applications/Notebook.app.
    path: PathBuf,
    /// The profile name. Default: made from the app's name.
    #[arg(long)]
    name: Option<String>,
    /// The memory of the guest: 4G, 6G, 8G. Default 4 GiB, at most half of
    /// the Mac.
    #[arg(long, value_name = "SIZE")]
    memory: Option<String>,
    /// Check only: change nothing.
    #[arg(long)]
    dry_run: bool,
    /// The result as JSON, with a top-level `v`.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub(crate) struct RemoveArgs {
    name: String,
    /// Delete the data disk now instead of moving it to the trash folder
    /// for 7 days.
    #[arg(long)]
    delete_data: bool,
    /// Remove without asking.
    #[arg(long, short = 'y')]
    yes: bool,
    /// The result as JSON, with the freed bytes. Needs --yes.
    #[arg(long, requires = "yes")]
    json: bool,
}

/// One reason why the app cannot be added now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Refusal {
    /// `not_a_bundle`, `app_store`, `bad_name`, `name_taken`,
    /// `bad_memory`, `no_golden` or `low_space`.
    code: &'static str,
    text: String,
}

/// The precheck of an add.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Precheck {
    v: u32,
    path: PathBuf,
    /// The profile name that the add uses.
    name: Option<String>,
    display_name: Option<String>,
    version: Option<String>,
    build: Option<String>,
    bundle_id: Option<String>,
    icon: Option<PathBuf>,
    /// The size of the bundle.
    bytes: Option<u64>,
    app_store: bool,
    /// The profile that has this bundle already.
    existing: Option<String>,
    /// `missing`, a build state (`fetch`, ... `seal`), `sealed` or
    /// `unreadable`.
    golden: String,
    space: Space,
    memory: Memory,
    refusals: Vec<Refusal>,
    ok: bool,
    dry_run: bool,
    /// True when the add wrote the profile.
    added: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Space {
    /// The clone of the golden and the copy of the app at the first start.
    need: u64,
    free: u64,
    ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Memory {
    /// What the add sets, or the default.
    mib: u64,
    default_mib: u64,
    /// Half of the Mac, when paddock knows its memory.
    max_mib: Option<u64>,
}

/// What the precheck reads besides the bundle.
pub(crate) struct Host {
    pub free: u64,
    pub host_gib: Option<u32>,
    pub taken: Vec<String>,
    pub existing: Option<String>,
}

/// The precheck. Pure over `host` and the bundle on disk.
pub(crate) fn precheck(layout: &Layout, a: &AddArgs, host: &Host) -> Precheck {
    let mut refusals = Vec::new();
    let mut refuse = |code, text| refusals.push(Refusal { code, text });
    let bundle = match appcopy::read(&a.path) {
        Ok(b) => Some(b),
        Err(e) => {
            refuse("not_a_bundle", format!("{e:#}"));
            None
        }
    };
    let app_store = bundle.as_ref().is_some_and(|b| b.mas);
    if app_store {
        refuse("app_store", text::mas_bundle(&a.path));
    }
    let name = match (&a.name, &host.existing, &bundle) {
        (Some(n), _, _) => Some(n.clone()),
        (None, Some(e), _) => Some(e.clone()),
        (None, None, Some(b)) => Some(appcopy::slug(&b.display_name, &host.taken)),
        (None, None, None) => None,
    };
    if let Some(n) = &name {
        if !host::valid_name(n) {
            refuse("bad_name", text::bad_app_name(n));
        } else if host.existing.as_deref() != Some(n) && host.taken.contains(n) {
            refuse("name_taken", text::add_name_taken(n));
        }
    }
    let cfg = a.memory.as_ref().map(|m| paddock::manifest::AppCfg {
        vm_memory: Some(m.clone()),
        ..Default::default()
    });
    let label = name.as_deref().unwrap_or("app");
    let mib = match paddock::macvm::host_app::memory_mib(label, cfg.as_ref(), host.host_gib) {
        Ok(m) => m,
        Err(e) => {
            refuse("bad_memory", format!("{e:#}"));
            paddock::macvm::host_app::DEFAULT_MEMORY_MIB
        }
    };
    let golden = match GoldenView::read(layout) {
        GoldenView::Missing => "missing".to_string(),
        GoldenView::Unreadable(_) => "unreadable".to_string(),
        GoldenView::Unsealed(m) => m.state.name().to_string(),
        GoldenView::Sealed { .. } => "sealed".to_string(),
    };
    if golden != "sealed" {
        refuse("no_golden", text::add_no_golden());
    }
    let need = paddock::macvm::session::CLONE_NEEDS + bundle.as_ref().map_or(0, |b| b.bytes);
    let space_ok = host.free >= need;
    if !space_ok {
        refuse("low_space", text::add_low_space(need, host.free));
    }
    let ok = refusals.is_empty();
    Precheck {
        v: 1,
        path: a.path.clone(),
        name,
        display_name: bundle.as_ref().map(|b| b.display_name.clone()),
        version: bundle.as_ref().map(|b| b.version.clone()),
        build: bundle.as_ref().map(|b| b.build.clone()),
        bundle_id: bundle.as_ref().map(|b| b.bundle_id.clone()),
        icon: bundle.as_ref().and_then(|b| b.icon.clone()),
        bytes: bundle.as_ref().map(|b| b.bytes),
        app_store,
        existing: host.existing.clone(),
        golden,
        space: Space {
            need,
            free: host.free,
            ok: space_ok,
        },
        memory: Memory {
            mib,
            default_mib: paddock::macvm::host_app::DEFAULT_MEMORY_MIB,
            max_mib: host.host_gib.map(|g| u64::from(g) * 1024 / 2),
        },
        refusals,
        ok,
        dry_run: a.dry_run,
        added: false,
    }
}

/// The config file of the user, where `[apps.NAME]` lives.
fn config_file(env: &HostEnv) -> PathBuf {
    env.paddock_config().join("config.toml")
}

/// The profile whose `[apps.NAME] bundle` is `path`.
fn existing_for(env: &HostEnv, path: &Path) -> Option<String> {
    let m = paddock::manifest::Manifest::load(env, &env.home.join(".paddock-no-project")).ok()?;
    m.merged.apps.iter().find_map(|(name, cfg)| {
        let named = cfg.bundle.as_deref()?;
        (env.expand(named, &env.home) == path).then(|| name.clone())
    })
}

/// Set or remove keys of `[apps.NAME]` in the user configuration, keeping
/// the rest of the file. A `None` value removes the key; a table left
/// empty goes.
fn edit_app(env: &HostEnv, name: &str, keys: &[(&str, Option<String>)]) -> Result<()> {
    let file = config_file(env);
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        existing.parse().with_context(|| text::cannot_read(&file))?;
    let apps = doc.entry("apps").or_insert(toml_edit::table());
    let apps = apps
        .as_table_mut()
        .with_context(|| text::not_a_table(&file, "apps"))?;
    apps.set_implicit(true);
    let app = apps.entry(name).or_insert(toml_edit::table());
    let app = app
        .as_table_mut()
        .with_context(|| text::not_a_table(&file, name))?;
    for (k, v) in keys {
        match v {
            Some(v) => {
                app.insert(k, toml_edit::value(v.clone()));
            }
            None => {
                app.remove(k);
            }
        }
    }
    if app.is_empty() {
        apps.remove(name);
    }
    paddock::privfs::dir(file.parent().expect("a config folder"))
        .with_context(|| text::cannot_make(&file))?;
    paddock::privfs::write_atomic(&file, doc.to_string())
        .with_context(|| text::cannot_write(&file))?;
    Ok(())
}

fn host_of(env: &HostEnv, path: &Path) -> Host {
    let layout = Layout::new(env);
    let root = paddock::eventstream::follow::existing_ancestor(&layout.root);
    Host {
        free: golden::free_bytes(&root, env.var("PADDOCK_MACVM_FAKE_FREE")).unwrap_or(0),
        host_gib: paddock::macvm::host_app::host_memory_gib(),
        taken: paddock::launchers::macapp::taken_names(env),
        existing: existing_for(
            env,
            &std::path::absolute(path).unwrap_or(path.to_path_buf()),
        ),
    }
}

/// `paddock app add PATH [--name] [--memory] [--dry-run] [--json]`.
pub(crate) fn add(env: &HostEnv, a: AddArgs) -> Result<i32> {
    let json = a.json;
    match add_inner(env, a) {
        Err(e) if json => {
            println!(
                "{}",
                serde_json::json!({ "v": 1, "error": format!("{e:#}") })
            );
            Ok(1)
        }
        result => result,
    }
}

fn add_inner(env: &HostEnv, mut a: AddArgs) -> Result<i32> {
    a.path = std::path::absolute(&a.path)?;
    let layout = Layout::new(env);
    let mut p = precheck(&layout, &a, &host_of(env, &a.path));
    if p.ok && !a.dry_run && p.existing.is_none() {
        let name = p.name.clone().expect("a bundle has a name");
        let mut keys = vec![("bundle", Some(a.path.display().to_string()))];
        if let Some(m) = &a.memory {
            keys.push(("vm_memory", Some(m.clone())));
        }
        edit_app(env, &name, &keys)?;
        p.added = true;
    }
    if a.json {
        println!("{}", serde_json::to_string(&p)?);
    } else if !p.ok {
        for r in &p.refusals {
            println!("{}", r.text);
        }
    } else if let (Some(name), Some(display)) = (&p.name, &p.display_name) {
        let line = match (&p.existing, a.dry_run) {
            (Some(e), _) => text::add_already(e, &a.path),
            (None, true) => text::add_would(display, name),
            (None, false) => text::add_done(display, name),
        };
        println!("{line}");
    }
    Ok(if p.ok { 0 } else { 1 })
}

/// What a removal freed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Removed {
    v: u32,
    name: String,
    /// The bytes that are gone from the disk now.
    freed: u64,
    /// The folder in the trash with the disk pair, unless --delete-data.
    trash: Option<PathBuf>,
    trash_bytes: u64,
    /// The launcher bundle in ~/Applications, when there was one.
    launcher: Option<PathBuf>,
    /// The `[apps.NAME]` keys of `paddock app add` went.
    config: bool,
}

/// The removal of a profile folder: the parts that a start makes again
/// go, the rest moves to the trash, or everything goes with
/// `delete_data`. Refused while the profile runs.
pub(crate) fn remove_profile(
    env: &HostEnv,
    layout: &Layout,
    name: &str,
    delete_data: bool,
) -> Result<(u64, Option<PathBuf>, u64)> {
    use paddock::macvm::session::{self, What};
    let profile = layout.profile(name, host::DEFAULT_HOME);
    if !profile.dir.is_dir() {
        return Ok((0, None, 0));
    }
    if let Some(s) = host::running(layout, name, host::DEFAULT_HOME) {
        anyhow::bail!(text::profile_running(name, s.pid));
    }
    let before = golden::used_bytes_in(&profile.dir);
    let rules = paddock::macvm::firewall::rules_file_if_any(env, name);
    if !delete_data {
        // The saved state, the app copy and the drops: a start makes them
        // again. The disk pair keeps the login, so it goes to the trash.
        for what in [What::State, What::Apps, What::Drop] {
            session::remove(&profile, what)
                .with_context(|| text::cannot_remove_part(&format!("{what:?}"), &profile.dir))?;
        }
    }
    let kept = golden::used_bytes_in(&profile.dir);
    let r = session::reset(
        layout,
        name,
        host::DEFAULT_HOME,
        text::ResetTier::All,
        rules.as_deref(),
        paddock::state::now(),
        &mut |_| Ok(paddock::macvm::backup::Outcome::Nothing),
    )?;
    let trash = r.aside;
    if delete_data {
        if let Some(t) = &trash {
            std::fs::remove_dir_all(t).with_context(|| text::cannot_remove(t))?;
        }
        return Ok((before, None, 0));
    }
    Ok((before.saturating_sub(kept), trash, kept))
}

/// `paddock app remove NAME [--delete-data] [--yes] [--json]`.
pub(crate) fn remove(env: &HostEnv, a: RemoveArgs) -> Result<i32> {
    match remove_inner(env, &a) {
        Ok(Some(r)) => {
            if a.json {
                println!("{}", serde_json::to_string(&r)?);
            } else {
                println!(
                    "{}",
                    text::app_removed(&r.name, r.freed, r.trash.as_deref())
                );
            }
            Ok(0)
        }
        Ok(None) => Ok(1),
        Err(e) if a.json => {
            println!(
                "{}",
                serde_json::json!({ "v": 1, "error": format!("{e:#}") })
            );
            Ok(1)
        }
        Err(e) => Err(e),
    }
}

fn remove_inner(env: &HostEnv, a: &RemoveArgs) -> Result<Option<Removed>> {
    if !host::valid_name(&a.name) {
        anyhow::bail!(text::bad_app_name(&a.name));
    }
    let layout = Layout::new(env);
    let profile = layout.profile(&a.name, host::DEFAULT_HOME);
    let configured = existing_bundle(env, &a.name);
    if !profile.dir.is_dir() && configured.is_none() {
        anyhow::bail!(text::app_not_added(&a.name));
    }
    if !a.yes {
        println!(
            "{}",
            text::app_remove_question(&a.name, golden::used_bytes_in(&profile.dir), a.delete_data)
        );
        let again = format!(
            "paddock app remove {}{} --yes",
            a.name,
            if a.delete_data { " --delete-data" } else { "" }
        );
        if !crate::cli::app_guest::ask_yes(false, "Remove?", &again)? {
            return Ok(None);
        }
    }
    let (freed, trash, trash_bytes) = remove_profile(env, &layout, &a.name, a.delete_data)?;
    let launcher = remove_launcher(env, &a.name, configured.as_deref());
    let config = configured.is_some();
    if config {
        edit_app(env, &a.name, &[("bundle", None), ("vm_memory", None)])?;
    }
    Ok(Some(Removed {
        v: 1,
        name: a.name.clone(),
        freed,
        trash,
        trash_bytes,
        launcher,
        config,
    }))
}

/// The `[apps.NAME] bundle` of the user configuration.
fn existing_bundle(env: &HostEnv, name: &str) -> Option<PathBuf> {
    let m = paddock::manifest::Manifest::load(env, &env.home.join(".paddock-no-project")).ok()?;
    let b = m.merged.apps.get(name)?.bundle.clone()?;
    Some(env.expand(&b, &env.home))
}

/// The launcher bundle of the app in ~/Applications goes, when there is
/// one. Its name comes from the app's display name.
fn remove_launcher(env: &HostEnv, name: &str, bundle: Option<&Path>) -> Option<PathBuf> {
    use paddock::launchers::macapp;
    let mut labels = Vec::new();
    if let Some(b) = bundle.and_then(|b| appcopy::read(b).ok()) {
        labels.push(b.display_name);
    }
    if let Ok(p) = paddock::app::app_profile(env, name) {
        labels.push(paddock::app::app_label(&p));
    }
    for label in labels {
        let path = macapp::launcher_path(env, &label);
        if path.exists() && macapp::uninstall(env, &label).unwrap_or(false) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
#[path = "app_add_tests.rs"]
mod tests;
