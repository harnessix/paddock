//! `paddock macvm`: the golden macOS guest of app mode. `setup` and
//! `update` build it in `paddock-vm macvm-setup`, the binary with the
//! Virtualization entitlement; `status`, `remove` and `rollback` work on
//! the files.

use anyhow::{Context, Result};
use clap::Subcommand;
use paddock::env::HostEnv;
use paddock::macvm::bench;
use paddock::macvm::bench_run;
use paddock::macvm::golden::{self, Keyring, Prev};
use paddock::macvm::layout::{self, Layout};
use paddock::macvm::manage::{self, GoldenView, RemovePlan};
use paddock::macvm::setup::{self, Opts};
use paddock::macvm::text;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum MacvmCmd {
    /// Build the golden macOS image: the restore image from Apple, the
    /// install, one guided step in a paddock window, the seal. It shows
    /// the URL and the sizes and asks first. A stopped setup continues
    /// where it was.
    Setup {
        /// A macOS restore image (.ipsw) instead of the download.
        #[arg(long, value_name = "PATH")]
        ipsw: Option<PathBuf>,
        /// Download without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Build a new golden next to the current one and swap them when it
    /// is sealed: the newest macOS this Mac runs, or --ipsw. The current
    /// golden stays as golden.prev for `rollback`. Each profile clones the
    /// new golden at its next start, after a backup of its data disk.
    Update {
        /// A macOS restore image (.ipsw) instead of the download.
        #[arg(long, value_name = "PATH")]
        ipsw: Option<PathBuf>,
        /// Download without asking.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Rebuild even when the image is the version of the current golden.
        #[arg(long)]
        force: bool,
    },
    /// Put the previous golden back while golden.prev exists. Profiles
    /// that ran on the new golden clone the previous one at their next
    /// start, after a backup of the data disk.
    Rollback {
        /// Roll back without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Show the golden, the host version, the profiles and the sessions.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Remove the golden image and the restore images. The profiles stay
    /// unless you add --profiles.
    Remove {
        /// Also remove every profile, after a copy of each data disk into
        /// the trash folder.
        #[arg(long)]
        profiles: bool,
        /// Remove without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Measure one case of the performance targets: warm, cold, first, quit,
    /// update, input, move, frames, idle, video or disk, in a session that
    /// started with --bench; or `report`, the table and the JSON of the cases
    /// in --dir. A development build, or PADDOCK_MACVM_BENCH=1.
    Bench {
        case: String,
        #[arg(long)]
        json: bool,
        /// Runs of the case: five for warm, cold, first, quit and update.
        #[arg(long)]
        runs: Option<u32>,
        /// The profile of the case. Default chatgpt-desktop; input and
        /// frames use PADDOCK_MACVM_BENCH_TEXT_APP (textedit), video
        /// PADDOCK_MACVM_BENCH_VIDEO_APP (quicktime).
        #[arg(long)]
        app: Option<String>,
        /// The folder of the per-case JSON files (`report`), or where
        /// a case writes its own.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// `report`: the JSON of the run goes here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(env: &HostEnv, cmd: MacvmCmd) -> Result<i32> {
    let layout = Layout::new(env);
    match cmd {
        MacvmCmd::Setup { ipsw, yes } => setup_cmd(env, &layout, ipsw, yes),
        MacvmCmd::Update { ipsw, yes, force } => update_cmd(env, &layout, ipsw, yes, force),
        MacvmCmd::Rollback { yes } => rollback_cmd(env, &layout, yes),
        MacvmCmd::Status { json } => {
            let host = host_version();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&manage::status_json(
                        &layout,
                        host.as_deref(),
                        &env.user
                    ))?
                );
            } else {
                for l in manage::status_lines(&layout, host.as_deref(), &env.user) {
                    println!("{l}");
                }
            }
            Ok(0)
        }
        MacvmCmd::Remove { profiles, yes } => remove_cmd(env, &layout, profiles, yes),
        MacvmCmd::Bench {
            case,
            json,
            runs,
            app,
            dir,
            out,
        } => bench_cmd(env, &case, json, runs, app, dir, out),
    }
}

/// The JSON of one case file: the measures of the case.
#[derive(serde::Serialize, serde::Deserialize)]
struct CaseFile {
    case: String,
    measures: Vec<bench::Measure>,
    #[serde(default)]
    misses: Vec<String>,
}

/// `paddock macvm bench`: one case into its JSON, or the report.
fn bench_cmd(
    env: &HostEnv,
    case: &str,
    json: bool,
    runs: Option<u32>,
    app: Option<String>,
    dir: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<i32> {
    if let Err(line) = bench::allowed(cfg!(debug_assertions), env.var(bench::ALLOW_VAR)) {
        println!("{line}");
        return Ok(1);
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from(".paddock").join("bench").join("cases"));
    let mut targets = bench::targets();
    let changed = bench::apply_env_targets(&mut targets, std::env::vars());
    if case == "report" {
        return bench_report(&dir, out.as_deref(), &targets, &changed, json);
    }
    let Some(case) = bench::Case::parse(case) else {
        println!("{}", text::bench_unknown_case(case, &bench::Case::names()));
        return Ok(1);
    };
    let app = app.unwrap_or_else(|| {
        let (var, default) = match case {
            bench::Case::Input | bench::Case::Frames => (bench_run::TEXT_APP_VAR, "textedit"),
            bench::Case::Video => (bench_run::VIDEO_APP_VAR, "quicktime"),
            _ => ("", "chatgpt-desktop"),
        };
        env.var(var).unwrap_or(default).to_string()
    });
    let runs = runs.unwrap_or_else(|| case.default_runs());
    let driver = bench_run::Driver::new(env, &app, runs, &dir.join("logs"));
    let measures = driver.run(case)?;
    let by_name = measures
        .iter()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();
    let misses: Vec<String> = bench::check_case(case, &targets, &by_name)
        .iter()
        .map(|m| m.to_string())
        .collect();
    let file = CaseFile {
        case: case.name().into(),
        measures,
        misses,
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", case.name()));
    paddock::privfs::write_atomic(&path, serde_json::to_vec_pretty(&file)?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&file)?);
    } else {
        for m in &file.measures {
            println!(
                "{}",
                text::bench_case_line(&m.name, m.p50, m.p95, m.samples, &m.unit)
            );
        }
        for miss in &file.misses {
            println!("{}", text::bench_miss_line(miss));
        }
    }
    Ok(0)
}

/// `bench report`: every case file of the folder into one run with the
/// Mac and the version, the table, and exit 1 with the misses named.
fn bench_report(
    dir: &std::path::Path,
    out: Option<&std::path::Path>,
    targets: &bench::Targets,
    changed: &[String],
    json: bool,
) -> Result<i32> {
    let mut run = bench::Run {
        measured: true,
        model: sysctl("hw.model").unwrap_or_default(),
        macos: host_version().unwrap_or_default(),
        paddock: env!("CARGO_PKG_VERSION").into(),
        date: paddock::waiting::format_rfc3339(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        measures: Default::default(),
    };
    for c in bench::Case::ALL {
        let path = dir.join(format!("{}.json", c.name()));
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let file: CaseFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("cannot read {}", path.display()))?;
        for m in file.measures {
            run.add(m);
        }
    }
    if let Some(out) = out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        paddock::privfs::write_atomic(out, serde_json::to_vec_pretty(&run)?)?;
    }
    let misses = bench::check(targets, &run.measures);
    if json {
        println!("{}", serde_json::to_string_pretty(&run)?);
    } else {
        print!("{}", bench::table(&run, targets));
    }
    if !changed.is_empty() {
        println!("{}", text::bench_env_target(changed));
    }
    if misses.is_empty() {
        println!("{}", text::bench_all_met(run.measures.len()));
        return Ok(0);
    }
    for m in &misses {
        println!("{}", text::bench_miss_line(m));
    }
    Ok(1)
}

/// `sysctl -n NAME` on macOS, nothing elsewhere.
fn sysctl(name: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let o = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !s.is_empty()).then_some(s)
}

fn setup_cmd(env: &HostEnv, layout: &Layout, ipsw: Option<PathBuf>, yes: bool) -> Result<i32> {
    let opts = Opts {
        ipsw: ipsw.clone(),
        yes,
        ..Default::default()
    };
    // The cheap refusals first: the sandbox, the keyring, the --ipsw path.
    setup::preflight(env, &opts)?;
    if !cfg!(target_os = "macos") {
        println!("paddock macvm setup runs on macOS only: the macOS guest needs the Virtualization framework of a Mac with Apple silicon.");
        return Ok(1);
    }
    let helper = paddock::vm::host::helper_path(env)?;
    let mut cmd = std::process::Command::new(&helper);
    cmd.arg("macvm-setup").arg("--dir").arg(&layout.root);
    if let Some(p) = &ipsw {
        cmd.arg("--ipsw").arg(std::path::absolute(p)?);
    }
    if yes {
        cmd.arg("--yes");
    }
    let status = cmd
        .status()
        .with_context(|| format!("cannot start {}", helper.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// `paddock macvm update`: the build into `golden.new` in the helper, then
/// the swap.
fn update_cmd(
    env: &HostEnv,
    layout: &Layout,
    ipsw: Option<PathBuf>,
    yes: bool,
    force: bool,
) -> Result<i32> {
    paddock::update::refuse_in_sandbox(env, "paddock macvm update")?;
    let opts = Opts {
        ipsw: ipsw.clone(),
        yes,
        force,
        ..Default::default()
    };
    setup::preflight(env, &opts)?;
    manage::refuse_while_running(layout)?;
    if let Some(r) = golden::recover(layout)? {
        println!("{}", r.line());
    }
    let current = match GoldenView::read(layout) {
        GoldenView::Sealed { manifest, .. } => manifest,
        _ => {
            println!("{}", text::update_needs_golden());
            return Ok(1);
        }
    };
    // A sealed golden.new is the golden that a rollback set aside: a
    // build in progress is continued instead.
    if let Ok(Some(m)) = layout.golden_new.read_manifest() {
        if m.state == golden::State::Sealed {
            println!("{}", text::update_removes_stale_new(&layout.golden_new.dir));
            layout::remove_golden_dir(&layout.golden_new)?;
        }
    }
    let profiles = layout.profile_dirs().len();
    println!("{}", text::update_plan(&current.version_text(), profiles));
    if !cfg!(target_os = "macos") {
        println!("paddock macvm update runs on macOS only: the macOS guest needs the Virtualization framework of a Mac with Apple silicon.");
        return Ok(1);
    }
    let helper = paddock::vm::host::helper_path(env)?;
    let mut cmd = std::process::Command::new(&helper);
    cmd.arg("macvm-setup")
        .arg("--dir")
        .arg(&layout.root)
        .arg("--new");
    if let Some(p) = &ipsw {
        cmd.arg("--ipsw").arg(std::path::absolute(p)?);
    }
    if yes {
        cmd.arg("--yes");
    }
    if force {
        cmd.arg("--force");
    }
    let status = cmd
        .status()
        .with_context(|| format!("cannot start {}", helper.display()))?;
    let code = status.code().unwrap_or(1);
    if code != 0 {
        // The helper printed why: stopped, failed or the same version.
        return Ok(code);
    }
    // A session may have started during the build.
    manage::refuse_while_running(layout)?;
    golden::swap(layout)?;
    println!(
        "{}",
        text::update_done(
            &current.version_text(),
            &golden::version_text_of(&layout.golden),
            profiles
        )
    );
    Ok(0)
}

/// `paddock macvm rollback`: the previous golden comes back.
fn rollback_cmd(env: &HostEnv, layout: &Layout, yes: bool) -> Result<i32> {
    paddock::update::refuse_in_sandbox(env, "paddock macvm rollback")?;
    manage::refuse_while_running(layout)?;
    if let Some(r) = golden::recover(layout)? {
        println!("{}", r.line());
    }
    let prev = match golden::prev_state(layout) {
        Prev::None => {
            println!("{}", text::no_prev_golden(&layout.golden_prev.dir));
            return Ok(1);
        }
        Prev::Kept { version } | Prev::Removable { version } => version,
    };
    let current = golden::version_text_of(&layout.golden);
    println!("{}", text::rollback_question(&current, &prev));
    if !ask(yes, "Roll back?", "paddock macvm rollback --yes")? {
        return Ok(1);
    }
    golden::rollback(layout)?;
    println!(
        "{}",
        text::rollback_done(
            &golden::version_text_of(&layout.golden),
            &layout.golden_new.dir
        )
    );
    Ok(0)
}

fn remove_cmd(env: &HostEnv, layout: &Layout, profiles: bool, yes: bool) -> Result<i32> {
    paddock::update::refuse_in_sandbox(env, "paddock macvm remove")?;
    manage::refuse_while_running(layout)?;
    let plan = RemovePlan::read(layout);
    if plan.golden.is_none() && (!profiles || plan.profiles.is_empty()) {
        println!("No macOS guest in {}.", layout.root.display());
        return Ok(0);
    }
    if let Some((version, size)) = &plan.golden {
        println!("{}", text::remove_golden_question(version, *size));
    }
    if profiles && !plan.profiles.is_empty() {
        println!(
            "{}",
            text::remove_profiles_question(plan.profiles.len(), plan.profiles_bytes())
        );
        for (app, home, _, size) in &plan.profiles {
            println!("  {app}/{home} {}", text::gib(*size));
        }
    }
    let again = if profiles {
        "paddock macvm remove --profiles --yes"
    } else {
        "paddock macvm remove --yes"
    };
    if !ask(yes, "Remove?", again)? {
        return Ok(1);
    }
    let keyring = Keyring::open(env).ok();
    if plan.golden.is_some() {
        manage::remove_golden(layout, keyring.as_ref())?;
        println!("Removed the golden.");
    }
    if profiles && !plan.profiles.is_empty() {
        let made = manage::remove_profiles(layout)?;
        println!(
            "Removed {} profile(s). The data disks are in {} for 7 days.",
            plan.profiles.len(),
            made.first()
                .and_then(|p| p.parent())
                .unwrap_or(&layout.trash)
                .display()
        );
    }
    Ok(0)
}

/// `sw_vers -productVersion` on macOS, nothing elsewhere.
fn host_version() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let o = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !s.is_empty()).then_some(s)
}

/// Ask on the terminal. Without a terminal, print the command with `--yes`
/// and answer no.
fn ask(yes: bool, question: &str, again: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        println!("Run `{again}` to go on.");
        return Ok(false);
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut a = String::new();
    std::io::stdin().read_line(&mut a)?;
    if !matches!(a.trim(), "y" | "Y" | "yes") {
        println!("Nothing changed.");
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_with_a_missing_ipsw_fails_before_any_folder_is_made() {
        let t = tempfile::tempdir().unwrap();
        let env = paddock::env::HostEnv::from_parts(
            t.path().join("home"),
            "demo".into(),
            501,
            20,
            [
                (
                    "XDG_DATA_HOME".to_string(),
                    t.path().join("data").display().to_string(),
                ),
                (
                    "XDG_STATE_HOME".to_string(),
                    t.path().join("state").display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let layout = Layout::new(&env);
        let e = run(
            &env,
            MacvmCmd::Setup {
                ipsw: Some(t.path().join("none.ipsw")),
                yes: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("none.ipsw"), "{e}");
        assert!(!layout.root.exists());
        assert!(!t.path().join("data").exists());
        assert!(!t.path().join("state").exists());
    }

    #[test]
    fn remove_with_nothing_says_so() {
        let t = tempfile::tempdir().unwrap();
        let env = paddock::env::HostEnv::from_parts(
            t.path().join("home"),
            "demo".into(),
            501,
            20,
            [(
                "XDG_DATA_HOME".to_string(),
                t.path().join("data").display().to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            run(
                &env,
                MacvmCmd::Remove {
                    profiles: false,
                    yes: true
                }
            )
            .unwrap(),
            0
        );
        assert_eq!(run(&env, MacvmCmd::Status { json: true }).unwrap(), 0);
    }

    #[test]
    fn update_and_rollback_refuse_without_a_golden_or_a_prev() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        // A stand-in helper on PATH: on a Mac the last update reaches the
        // build, which this helper refuses after it wrote its arguments.
        let bin = t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let args = t.path().join("helper-args");
        let helper = bin.join("paddock-vm");
        std::fs::write(
            &helper,
            format!("#!/bin/sh\necho \"$@\" > '{}'\nexit 1\n", args.display()),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = paddock::env::HostEnv::from_parts(
            t.path().join("home"),
            "demo".into(),
            501,
            20,
            [
                ("PATH".to_string(), bin.display().to_string()),
                (
                    "XDG_DATA_HOME".to_string(),
                    t.path().join("data").display().to_string(),
                ),
                (
                    "XDG_STATE_HOME".to_string(),
                    t.path().join("state").display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let layout = Layout::new(&env);
        // No golden: update says setup, touches nothing.
        assert_eq!(
            run(
                &env,
                MacvmCmd::Update {
                    ipsw: None,
                    yes: true,
                    force: false
                }
            )
            .unwrap(),
            1
        );
        assert!(!layout.golden_new.dir.exists());
        // No prev: rollback refuses with the folder.
        assert_eq!(run(&env, MacvmCmd::Rollback { yes: true }).unwrap(), 1);
        // A sealed golden with a prev: the rollback swaps them.
        for (g, v) in [(&layout.golden, "15.3"), (&layout.golden_prev, "15.2")] {
            std::fs::create_dir_all(&g.dir).unwrap();
            std::fs::write(&g.disk, v).unwrap();
            let mut m = paddock::macvm::golden::Manifest::new("15.3", "24D1", "0.9.0");
            m.version = v.into();
            m.build = "B".into();
            m.sealed = Some("2026-11-01T10:00:00Z".into());
            m.at(golden::State::Sealed);
            g.write_manifest(&m).unwrap();
        }
        assert_eq!(run(&env, MacvmCmd::Rollback { yes: true }).unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(&layout.golden.disk).unwrap(),
            "15.2"
        );
        assert_eq!(
            std::fs::read_to_string(&layout.golden_new.disk).unwrap(),
            "15.3"
        );
        assert!(!layout.golden_prev.dir.exists());
        // An update now: the set-aside golden in golden.new goes before the
        // build, and on Linux the build itself is refused with one line.
        assert_eq!(
            run(
                &env,
                MacvmCmd::Update {
                    ipsw: None,
                    yes: true,
                    force: true
                }
            )
            .unwrap(),
            1
        );
        assert!(!layout.golden_new.dir.exists());
        assert_eq!(
            std::fs::read_to_string(&layout.golden.disk).unwrap(),
            "15.2"
        );
        // Only a Mac starts the helper, and a failed build swaps nothing.
        if cfg!(target_os = "macos") {
            let got = std::fs::read_to_string(&args).unwrap();
            assert!(
                got.starts_with("macvm-setup --dir ") && got.contains(" --new --yes --force"),
                "{got}"
            );
        } else {
            assert!(!args.exists());
        }
        let out = run(&env, MacvmCmd::Status { json: false }).unwrap();
        assert_eq!(out, 0);
    }
}
