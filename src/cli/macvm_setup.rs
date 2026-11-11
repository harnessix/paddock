//! `paddock macvm setup`: the build in `paddock-vm macvm-setup`, and for the
//! launcher `--json` (the progress as `setup` lines of the event schema),
//! `--pause-download` and `--resume`.

use anyhow::{Context, Result};
use paddock::env::HostEnv;
use paddock::eventstream::schema::{Event, Setup};
use paddock::eventstream::Line;
use paddock::macvm::layout::Layout;
use paddock::macvm::setup::{self, Opts};
use paddock::macvm::setup_progress::{self, Pause};
use paddock::macvm::text;
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// The flags of `paddock macvm setup`.
#[derive(Debug, Clone, Default)]
pub(crate) struct SetupFlags {
    pub ipsw: Option<PathBuf>,
    pub yes: bool,
    pub json: bool,
    pub pause_download: bool,
    pub resume: bool,
}

pub(crate) fn setup_cmd(env: &HostEnv, layout: &Layout, f: SetupFlags) -> Result<i32> {
    if f.pause_download {
        let p = match setup_progress::request_pause(layout) {
            Ok(p) => p,
            Err(e) if f.json => {
                println!(
                    "{}",
                    serde_json::json!({ "v": 1, "error": format!("{e:#}") })
                );
                return Ok(1);
            }
            Err(e) => return Err(e.into()),
        };
        let code = i32::from(p != Pause::Requested);
        if f.json {
            println!("{}", pause_json(&p));
        } else {
            println!("{}", p.line());
        }
        return Ok(code);
    }
    if f.resume && setup_progress::resume(layout) {
        if f.json {
            println!(
                "{}",
                serde_json::json!({ "v": 1, "result": "resumed", "text": text::resume_running() })
            );
        } else {
            println!("{}", text::resume_running());
        }
        return Ok(0);
    }
    if !f.json {
        return build(env, layout, &f);
    }
    // JSON: every line on stdout is a `setup` line, the refusals too.
    match build(env, layout, &f) {
        Ok(code) => Ok(code),
        Err(e) => {
            print!("{}", failed_line("", &format!("{e:#}")));
            Ok(1)
        }
    }
}

/// The JSON of `--pause-download`.
fn pause_json(p: &Pause) -> serde_json::Value {
    let (result, step) = match p {
        Pause::Requested => ("requested", None),
        Pause::NoBuild => ("no_build", None),
        Pause::PastDownload(s) => ("past_download", Some(s.clone())),
    };
    serde_json::json!({ "v": 1, "result": result, "step": step, "text": p.line() })
}

fn failed_line(step: &str, reason: &str) -> String {
    Line::now(Event::Setup(Setup {
        step: step.to_string(),
        failed: Some(reason.to_string()),
        ..Default::default()
    }))
    .to_text()
}

/// One line of the helper under `--json`: a stream line passes as it is,
/// any other line becomes the `line` of a `setup` line of the last state.
/// The state of a stream line becomes the last state.
pub(crate) fn wrap_line(raw: &str, last_step: &mut String) -> String {
    if paddock::eventstream::schema::valid_line(raw) {
        if let Ok(Line {
            event: Event::Setup(s),
            ..
        }) = serde_json::from_str::<Line>(raw)
        {
            last_step.clone_from(&s.step);
        }
        return format!("{raw}\n");
    }
    Line::now(Event::Setup(Setup {
        step: last_step.clone(),
        line: Some(raw.to_string()),
        ..Default::default()
    }))
    .to_text()
}

fn build(env: &HostEnv, layout: &Layout, f: &SetupFlags) -> Result<i32> {
    let opts = Opts {
        ipsw: f.ipsw.clone(),
        yes: f.yes || f.resume,
        ..Default::default()
    };
    // The cheap refusals first: the sandbox, the keyring, the --ipsw path.
    setup::preflight(env, &opts)?;
    if !cfg!(target_os = "macos") {
        let line = "paddock macvm setup runs on macOS only: the macOS guest needs the Virtualization framework of a Mac with Apple silicon.";
        if f.json {
            print!("{}", failed_line("", line));
        } else {
            println!("{line}");
        }
        return Ok(1);
    }
    let helper = paddock::vm::host::helper_path(env)?;
    let mut cmd = std::process::Command::new(&helper);
    cmd.arg("macvm-setup").arg("--dir").arg(&layout.root);
    if let Some(p) = &f.ipsw {
        cmd.arg("--ipsw").arg(std::path::absolute(p)?);
    }
    if opts.yes {
        cmd.arg("--yes");
    }
    if !f.json {
        let status = cmd
            .status()
            .with_context(|| format!("cannot start {}", helper.display()))?;
        return Ok(status.code().unwrap_or(1));
    }
    cmd.env(setup_progress::JSON_VAR, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot start {}", helper.display()))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut last_step = String::new();
    let mut out = std::io::stdout().lock();
    for raw in std::io::BufReader::new(stdout).lines() {
        let Ok(raw) = raw else { break };
        if raw.is_empty() {
            continue;
        }
        // The reader may go; the build goes on.
        let _ = out.write_all(wrap_line(&raw, &mut last_step).as_bytes());
        let _ = out.flush();
    }
    let status = child.wait()?;
    let code = status.code().unwrap_or(1);
    let last = setup_progress::read(layout);
    let ended = last
        .as_ref()
        .is_some_and(|s| s.failed.is_some() || s.stopped || s.step == "sealed");
    if code != 0 && !ended {
        let _ = out.write_all(failed_line(&last_step, &text::setup_helper_exit(code)).as_bytes());
    }
    Ok(code)
}

/// The bytes that a removal of the golden frees: the golden folders and
/// the restore images.
pub(crate) fn golden_bytes(layout: &Layout) -> u64 {
    use paddock::macvm::golden::used_bytes_in;
    [&layout.golden, &layout.golden_new, &layout.golden_prev]
        .iter()
        .map(|g| used_bytes_in(&g.dir))
        .sum::<u64>()
        + used_bytes_in(&layout.ipsw)
}

/// The `golden removed` event.
pub(crate) fn publish_removed(env: &HostEnv, freed: u64) {
    use paddock::eventstream::schema::{Golden, GoldenState};
    paddock::eventstream::publish(
        env,
        Event::Golden(Golden {
            state: GoldenState::Removed,
            version: None,
            bytes: freed,
        }),
    );
}

/// `paddock macvm remove --yes --json`: the removal and what it freed.
/// The profiles go to the trash folder, so they free nothing until it is
/// emptied; their bytes are under `trash`.
pub(crate) fn remove_json(
    env: &HostEnv,
    layout: &Layout,
    profiles: bool,
) -> Result<serde_json::Value> {
    use paddock::macvm::manage::{self, RemovePlan};
    paddock::update::refuse_in_sandbox(env, "paddock macvm remove")?;
    manage::refuse_while_running(layout)?;
    let plan = RemovePlan::read(layout);
    let mut freed = 0;
    let mut golden = serde_json::Value::Null;
    if let Some((version, _)) = &plan.golden {
        let before = golden_bytes(layout);
        let keyring = paddock::macvm::golden::Keyring::open(env).ok();
        manage::remove_golden(layout, keyring.as_ref())?;
        freed = before.saturating_sub(golden_bytes(layout));
        publish_removed(env, freed);
        golden = serde_json::json!({ "version": version, "bytes": freed });
    }
    let mut trash = serde_json::Value::Null;
    if profiles && !plan.profiles.is_empty() {
        let made = manage::remove_profiles(layout)?;
        trash = serde_json::json!({
            "dir": layout.trash,
            "bytes": plan.profiles_bytes(),
            "profiles": plan.profiles.iter().map(|(a, h, _, b)| serde_json::json!({ "app": a, "home": h, "bytes": b })).collect::<Vec<_>>(),
            "folders": made,
            "days": manage::TRASH_DAYS,
        });
    }
    Ok(
        serde_json::json!({ "v": 1, "dir": layout.root, "golden": golden, "freed": freed, "trash": trash }),
    )
}

/// The build fields of `paddock macvm status --json`: `paused` at the
/// top, and `setup` with the last progress line and whether a build runs.
pub(crate) fn add_setup_status(j: &mut serde_json::Value, layout: &Layout) {
    let running = setup_progress::running(layout);
    let last = setup_progress::read(layout);
    let paused = running
        && last.as_ref().is_some_and(|s| s.step == "fetch")
        && setup_progress::paused(layout);
    j["paused"] = paused.into();
    j["setup"] = serde_json::json!({ "running": running, "paused": paused, "last": last });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_lines_become_setup_lines() {
        let mut step = String::new();
        let progress = Line::at(
            5,
            Event::Setup(Setup {
                step: "fetch".into(),
                bytes: Some(1),
                total: Some(2),
                ..Default::default()
            }),
        )
        .to_text();
        assert_eq!(wrap_line(progress.trim_end(), &mut step), progress);
        assert_eq!(step, "fetch");
        let wrapped = wrap_line("download 5%, about 9 min left", &mut step);
        let l: Line = serde_json::from_str(&wrapped).unwrap();
        assert_eq!(l.v, 1);
        let Event::Setup(s) = l.event else {
            panic!("{wrapped}")
        };
        assert_eq!(s.step, "fetch");
        assert_eq!(s.line.as_deref(), Some("download 5%, about 9 min left"));
        assert!(!s.paused && s.failed.is_none());
        // A JSON line of another kind passes and keeps the step.
        let disk = r#"{"v":1,"t":"x","kind":"disk","dir":"/m","free":1}"#;
        assert_eq!(wrap_line(disk, &mut step), format!("{disk}\n"));
        assert_eq!(step, "fetch");
    }

    #[test]
    fn pause_answers_are_json_with_a_version() {
        for (p, word) in [
            (Pause::Requested, "requested"),
            (Pause::NoBuild, "no_build"),
            (Pause::PastDownload("install".into()), "past_download"),
        ] {
            let j: serde_json::Value = serde_json::from_str(&pause_json(&p).to_string()).unwrap();
            assert_eq!(j["v"], 1);
            assert_eq!(j["result"], word);
            assert!(!j["text"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn a_refusal_in_json_is_a_failed_setup_line() {
        let l: Line = serde_json::from_str(&failed_line("", "no keyring")).unwrap();
        assert_eq!(
            l.event,
            Event::Setup(Setup {
                failed: Some("no keyring".into()),
                ..Default::default()
            })
        );
    }

    fn env(t: &std::path::Path) -> HostEnv {
        HostEnv::from_parts(
            t.join("home"),
            "demo".into(),
            501,
            20,
            [
                (
                    "XDG_DATA_HOME".to_string(),
                    t.join("data").display().to_string(),
                ),
                (
                    "XDG_RUNTIME_DIR".to_string(),
                    t.join("run").display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    #[test]
    fn remove_json_reports_the_freed_bytes_and_the_trash() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        let layout = Layout::new(&env);
        // Nothing there: nothing freed, still a versioned answer.
        let j = remove_json(&env, &layout, true).unwrap();
        assert_eq!((j["v"].clone(), j["freed"].clone()), (1.into(), 0.into()));
        assert_eq!(j["golden"], serde_json::Value::Null);
        // A golden, a restore image and one profile.
        std::fs::create_dir_all(&layout.golden.dir).unwrap();
        std::fs::write(&layout.golden.disk, vec![1u8; 64 << 10]).unwrap();
        std::fs::create_dir_all(&layout.ipsw).unwrap();
        std::fs::write(layout.ipsw.join("15.1.ipsw"), vec![1u8; 32 << 10]).unwrap();
        let p = layout.profile("slack", "default");
        std::fs::create_dir_all(&p.dir).unwrap();
        std::fs::write(p.data(), vec![1u8; 16 << 10]).unwrap();
        let before = golden_bytes(&layout);
        assert!(before >= 96 << 10, "{before}");
        // A follower sees the golden go.
        let bus = paddock::eventstream::Bus::of(&env);
        let f = paddock::eventstream::bus::Follower::bind(&bus, std::process::id()).unwrap();
        let (tx, rx) = paddock::eventstream::bus::queue();
        f.serve(tx).unwrap();
        let text = remove_json(&env, &layout, true).unwrap().to_string();
        let j: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(j["v"], 1);
        assert_eq!(j["freed"], before);
        assert_eq!(j["golden"]["bytes"], before);
        assert_eq!(j["trash"]["profiles"][0]["app"], "slack");
        assert!(j["trash"]["bytes"].as_u64().unwrap() >= 16 << 10);
        assert!(!layout.golden.dir.exists() && !layout.ipsw.exists());
        assert!(!p.dir.exists());
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let l: Line = serde_json::from_str(&line).unwrap();
        assert_eq!(
            l.event,
            Event::Golden(paddock::eventstream::schema::Golden {
                state: paddock::eventstream::schema::GoldenState::Removed,
                version: None,
                bytes: before,
            })
        );
    }

    #[test]
    fn status_json_carries_paused_and_the_last_setup_line() {
        let t = tempfile::tempdir().unwrap();
        let layout = Layout::at(t.path().join("macvm"));
        let mut j = serde_json::json!({ "v": 1 });
        add_setup_status(&mut j, &layout);
        assert_eq!(j["paused"], false);
        assert_eq!(j["setup"]["running"], false);
        assert_eq!(j["setup"]["last"], serde_json::Value::Null);
        std::fs::create_dir_all(&layout.root).unwrap();
        use std::os::fd::AsRawFd;
        let lock = std::fs::File::create(&layout.setup_lock).unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        let mut r = setup_progress::Reporter::file_only(&layout);
        r.download(paddock::macvm::download::Tick::Paused {
            bytes: 6,
            total: Some(13),
        });
        std::fs::write(setup_progress::pause_file(&layout), "").unwrap();
        let mut j = serde_json::json!({ "v": 1 });
        add_setup_status(&mut j, &layout);
        let mut j: serde_json::Value = serde_json::from_str(&j.to_string()).unwrap();
        assert_eq!(j["paused"], true);
        assert_eq!(j["setup"]["last"]["step"], "fetch");
        assert_eq!(j["setup"]["last"]["bytes"], 6);
        assert_eq!(j["setup"]["last"]["paused"], true);
        r.step("install");
        add_setup_status(&mut j, &layout);
        assert_eq!(j["paused"], false);
        drop(lock);
        r.step("fetch");
        add_setup_status(&mut j, &layout);
        assert_eq!(j["paused"], false);
    }

    #[test]
    fn pause_json_returns_an_error_when_the_request_cannot_be_written() {
        use std::os::fd::AsRawFd;
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        let layout = Layout::new(&env);
        std::fs::create_dir_all(&layout.root).unwrap();
        let lock = std::fs::File::create(&layout.setup_lock).unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        let mut reporter = setup_progress::Reporter::file_only(&layout);
        reporter.step("fetch");
        std::fs::create_dir(setup_progress::pause_file(&layout)).unwrap();
        let flags = SetupFlags {
            pause_download: true,
            json: true,
            ..Default::default()
        };
        assert_eq!(setup_cmd(&env, &layout, flags).unwrap(), 1);
    }

    #[test]
    fn pause_and_resume_without_a_build() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        let layout = Layout::new(&env);
        let f = SetupFlags {
            pause_download: true,
            json: true,
            ..Default::default()
        };
        assert_eq!(setup_cmd(&env, &layout, f).unwrap(), 1);
        assert!(!setup_progress::paused(&layout));
    }
}
