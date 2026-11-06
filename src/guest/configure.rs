//! The configure steps of the golden build, run in the guest on `configure
//! {password, steps}`. The host sends the list. A root step runs through
//! `sudo -S` with the password on stdin, never in an argv. Each step reports
//! its outcome and the output of its verify command, so the host and the Mac
//! check see what happened.
//!
//! The runner is split: `argv` builds the command line (pure, tested),
//! `run_plan` runs one in a process group of its own and kills the whole
//! group at [`STEP_TIMEOUT`]: a command that waits on a permission prompt
//! (or a `sudo` whose child waits) fails, never hangs the request. Each
//! step logs one line with its name, exit code and time, and its result
//! carries both, so the host prints which step failed.
//!
//! A step whose argv starts with [`AGENT_OP`] is not a command: the main
//! loop runs it on the main thread before the others (`agent_steps`), and
//! `run_all` puts its result in its place.

use crate::macvm::configure::{AGENT_OP, STEP_TIMEOUT_SECS};
use crate::macvm::proto::{ConfigureStep, StepResult};
use crate::macvm::text;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Each command of a step times out after this.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(STEP_TIMEOUT_SECS);
/// After the timeout: the wait between the SIGTERM and the SIGKILL of the
/// group, and the most that the output readers get after the kill.
pub const KILL_GRACE: Duration = Duration::from_secs(2);
/// The longest output kept per step.
pub const OUTPUT_MAX: usize = 4096;
/// The sudo of the guest.
pub const SUDO: &str = "/usr/bin/sudo";

/// What runs for one command: the argv and whether the password goes on
/// stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    pub password_on_stdin: bool,
}

/// The argv of a step. A root step: `sudo -S -p '' -- ARGV`, so sudo
/// reads the password from stdin without a prompt. A user step: ARGV,
/// with the password on stdin when ARGV asks for it with `-password -`
/// (`sysadminctl`). The network service placeholder becomes `service`
/// when it is known.
pub fn argv(step: &ConfigureStep, service: Option<&str>) -> Plan {
    let mut p = plan(step.as_root, &with_service(&step.argv, service));
    p.password_on_stdin |= asks_password(&step.argv);
    p
}

fn asks_password(argv: &[String]) -> bool {
    argv.windows(2).any(|w| w[0] == "-password" && w[1] == "-")
}

/// The argv of the verify command of a step, with the same root rule.
pub fn verify_argv(step: &ConfigureStep, service: Option<&str>) -> Option<Plan> {
    if step.verify.is_empty() {
        return None;
    }
    Some(plan(step.as_root, &with_service(&step.verify, service)))
}

fn with_service(argv: &[String], service: Option<&str>) -> Vec<String> {
    match service {
        Some(s) => crate::macvm::configure::resolve_service(argv, s),
        None => argv.to_vec(),
    }
}

/// The network service of the guest, from `networksetup`. `None` when
/// the listing has no service: the steps then fail with the placeholder
/// in their output, which names the problem.
pub fn network_service() -> Option<String> {
    let out = Command::new("/usr/sbin/networksetup")
        .arg("-listallnetworkservices")
        .output()
        .ok()?;
    crate::macvm::configure::pick_service(&String::from_utf8_lossy(&out.stdout))
}

fn plan(as_root: bool, argv: &[String]) -> Plan {
    if as_root {
        let mut v = vec![
            SUDO.to_string(),
            "-S".into(),
            "-p".into(),
            String::new(),
            "--".into(),
        ];
        v.extend(argv.iter().cloned());
        Plan {
            argv: v,
            password_on_stdin: true,
        }
    } else {
        Plan {
            argv: argv.to_vec(),
            password_on_stdin: false,
        }
    }
}

/// What one command did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ran {
    /// The exit code; `None` when a signal ended it or it timed out.
    pub code: Option<i32>,
    pub timed_out: bool,
    /// Both streams, the last [`OUTPUT_MAX`] characters.
    pub output: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Run the steps of the agent itself (argv[0] is [`AGENT_OP`]) with
/// `op`, which gives the success and the output of an argv without its
/// first word. The main loop calls this on its thread; the results go to
/// [`run_all`].
pub fn agent_steps(
    steps: &[ConfigureStep],
    mut op: impl FnMut(&[String]) -> (bool, String),
) -> Vec<StepResult> {
    let mut out = Vec::new();
    for step in steps.iter().filter(|s| is_agent_op(&s.argv)) {
        let start = Instant::now();
        let (ok, output) = op(&step.argv[1..]);
        let verify = if is_agent_op(&step.verify) {
            op(&step.verify[1..]).1
        } else {
            String::new()
        };
        out.push(StepResult {
            name: step.name.clone(),
            ok,
            output,
            verify,
            code: Some(if ok { 0 } else { 1 }),
            ms: start.elapsed().as_millis() as u64,
            timed_out: false,
        });
    }
    out
}

fn is_agent_op(argv: &[String]) -> bool {
    argv.first().is_some_and(|a| a == AGENT_OP)
}

/// Run every step in order. `spawn` runs one plan with the text for its
/// stdin. `done` holds the results of the agent's own steps
/// ([`agent_steps`]); each goes in its place. Every step logs one line.
/// The configure steps whose settings macOS keeps per machine: the power
/// settings and the `-currentHost` preferences live in files named after
/// the hardware UUID, and each profile boots the golden with a machine
/// identifier of its own. Without them a profile's guest falls back to
/// the defaults and locks its screen after the default idle time.
pub const MACHINE_STEPS: &[&str] = &[
    "no-sleep",
    "no-screen-saver",
    "no-screen-lock",
    // Also the lock settings of every host: they are in the golden, and
    // set again here so an older golden gets them too, and each one is
    // read back into the log for the `locksettings` Mac check.
    "no-screen-saver-any-host",
    "no-saver-password",
    "no-saver-password-delay",
    "no-login-window-saver",
    "no-auto-logout",
];

/// The configure steps of [`MACHINE_STEPS`] from the golden's table.
pub fn machine_step_list() -> Vec<ConfigureStep> {
    crate::macvm::configure::STEPS
        .iter()
        .filter(|s| MACHINE_STEPS.contains(&s.name))
        .map(|s| s.wire())
        .collect()
}

/// Whether the read back of step `name` holds the value of the golden's
/// table.
fn proves(name: &str, verify: &str) -> bool {
    crate::macvm::configure::STEPS
        .iter()
        .find(|s| s.name == name)
        .is_some_and(|s| verify.contains(s.expect))
}

/// Run [`MACHINE_STEPS`] on this machine, after the data disk of a
/// profile is prepared. They are idempotent; a failure is a warning, not
/// a failed prepare.
pub fn machine_steps(password: &str) {
    for r in run_all(&machine_step_list(), password, None, Vec::new(), spawn_step) {
        // The read back value, for the `locksettings` check: these
        // settings carry no secret.
        let read: String = r.verify.trim().chars().take(80).collect();
        let proven = r.ok && proves(&r.name, &r.verify);
        log::info!("machine step {}: proven {proven}, reads {read:?}", r.name); // not user text
        if !proven {
            // The output of these three commands carries no secret.
            let out: String = r.output.trim().chars().take(300).collect();
            log::warn!(
                "{}: {out}",
                crate::macvm::text::machine_step_failed(&r.name)
            );
        }
    }
}

pub fn run_all(
    steps: &[ConfigureStep],
    password: &str,
    service: Option<&str>,
    done: Vec<StepResult>,
    mut spawn: impl FnMut(&Plan, Option<&str>) -> std::io::Result<Ran>,
) -> Vec<StepResult> {
    let mut out = Vec::new();
    log::info!("configure: {} steps", steps.len()); // not user text
    for step in steps {
        if is_agent_op(&step.argv) {
            let r = done
                .iter()
                .find(|r| r.name == step.name)
                .cloned()
                .unwrap_or_else(|| StepResult {
                    name: step.name.clone(),
                    output: text::agent_step_not_run(&step.name),
                    ..Default::default()
                });
            log_step(&r);
            out.push(r);
            continue;
        }
        let start = Instant::now();
        let p = argv(step, service);
        let stdin = p.password_on_stdin.then_some(password);
        let ran = match spawn(&p, stdin) {
            Ok(r) => r,
            Err(e) => Ran {
                output: text::cannot_run_command(&p.argv.join(" "), &e.to_string()),
                ..Default::default()
            },
        };
        let mut timed_out = ran.timed_out;
        let verify = match verify_argv(step, service) {
            Some(v) => {
                let stdin = v.password_on_stdin.then_some(password);
                match spawn(&v, stdin) {
                    Ok(r) => {
                        timed_out |= r.timed_out;
                        r.output
                    }
                    Err(e) => text::verify_no_run(&e.to_string()),
                }
            }
            None => String::new(),
        };
        let r = StepResult {
            name: step.name.clone(),
            ok: ran.ok(),
            output: ran.output,
            verify,
            code: ran.code,
            ms: start.elapsed().as_millis() as u64,
            timed_out,
        };
        log_step(&r);
        out.push(r);
    }
    out
}

/// The log line of a step: never its output, which a root step could
/// fill with anything.
fn log_step(r: &StepResult) {
    let code = r.code.map_or("none".to_string(), |c| c.to_string());
    let line = format!(
        // not user text
        "configure step {}: ok {}, exit {code}, {} ms, timed out {}",
        r.name, r.ok, r.ms, r.timed_out
    );
    if r.ok {
        log::info!("{line}");
    } else {
        log::warn!("{line}");
    }
}

/// Run one plan as [`run_plan`] does and give the success and the output,
/// with the timeout said in the output: for the other callers.
pub fn spawn(plan: &Plan, stdin: Option<&str>) -> std::io::Result<(bool, String)> {
    let r = run_plan(plan, stdin, STEP_TIMEOUT)?;
    let ok = r.ok();
    if r.timed_out {
        return Ok((false, text::step_timed_out(&r.output, STEP_TIMEOUT)));
    }
    Ok((ok, r.output))
}

/// [`run_plan`] with the step timeout, for [`run_all`].
pub fn spawn_step(plan: &Plan, stdin: Option<&str>) -> std::io::Result<Ran> {
    let mut r = run_plan(plan, stdin, STEP_TIMEOUT)?;
    if r.timed_out {
        r.output = text::step_timed_out(&r.output, STEP_TIMEOUT);
    }
    Ok(r)
}

/// Run one plan: the text goes to stdin with a newline, both output
/// streams are kept (the last [`OUTPUT_MAX`] characters). The command runs
/// in a process group of its own; after `limit` the group gets SIGTERM,
/// then SIGKILL after [`KILL_GRACE`], and the readers get [`KILL_GRACE`]
/// to finish: a grandchild that keeps a pipe open cannot hold the step.
pub fn run_plan(plan: &Plan, stdin: Option<&str>, limit: Duration) -> std::io::Result<Ran> {
    run_plan_cancel(plan, stdin, limit, &AtomicBool::new(false))
}

/// [`run_plan`] that also ends the group when `cancel` turns true, as at
/// the limit: the result then says `timed_out`, and the caller, who set
/// the flag, knows why.
pub fn run_plan_cancel(
    plan: &Plan,
    stdin: Option<&str>,
    limit: Duration,
    cancel: &AtomicBool,
) -> std::io::Result<Ran> {
    use std::os::unix::process::CommandExt;
    let (program, args) = plan
        .argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("an empty command"))?;
    let mut child = Command::new(program)
        .args(args)
        .process_group(0)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pgid = child.id() as libc::pid_t;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // A command that reads no stdin closes the pipe: that is fine.
        let _ = pipe.write_all(text.as_bytes());
        let _ = pipe.write_all(b"\n");
    }
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let err_reader = std::thread::spawn(move || {
            let mut err = Vec::new();
            let _ = stderr.read_to_end(&mut err);
            err
        });
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let err = err_reader.join().unwrap_or_default();
        let _ = tx.send((out, err));
    });
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() > limit || cancel.load(Ordering::SeqCst) {
            timed_out = true;
            // SAFETY: signals to the process group that this call made.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };
            let grace = Instant::now();
            while grace.elapsed() < KILL_GRACE {
                if child.try_wait()?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // SAFETY: as above; the group may hold children of the child.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            let _ = child.kill();
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // The readers end when every holder of the pipes is gone; a holder
    // outside the group (a daemon that the command started) must not
    // hold the step.
    let wait = if timed_out {
        KILL_GRACE
    } else {
        Duration::from_secs(10)
    };
    let (out, err) = rx.recv_timeout(wait).unwrap_or_default();
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&err));
    }
    let text = text.trim_end().to_string();
    let text = match text.char_indices().nth_back(OUTPUT_MAX) {
        Some((i, _)) => text[i..].to_string(),
        None => text,
    };
    Ok(Ran {
        code: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        timed_out,
        output: text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<ConfigureStep> {
        vec![
            ConfigureStep {
                name: "proxy".into(),
                as_root: true,
                argv: vec![
                    "/usr/sbin/networksetup".into(),
                    "-setsecurewebproxy".into(),
                    "$SERVICE".into(),
                    "127.0.0.1".into(),
                    "3128".into(),
                ],
                verify: vec![
                    "/usr/sbin/networksetup".into(),
                    "-getsecurewebproxy".into(),
                    "$SERVICE".into(),
                ],
            },
            ConfigureStep {
                name: "reduce motion".into(),
                as_root: false,
                argv: vec![
                    "/usr/bin/defaults".into(),
                    "write".into(),
                    "com.apple.universalaccess".into(),
                    "reduceMotion".into(),
                    "-bool".into(),
                    "true".into(),
                ],
                verify: vec![],
            },
        ]
    }

    fn fine() -> Ran {
        Ran {
            code: Some(0),
            timed_out: false,
            output: "fine".into(),
        }
    }

    fn sh(script: &str) -> Plan {
        Plan {
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            password_on_stdin: false,
        }
    }

    #[test]
    fn a_hanging_command_is_killed_at_its_limit_with_its_group() {
        // A command that ignores SIGTERM, with a child in its group.
        let t = Instant::now();
        let r = run_plan(
            &sh("trap '' TERM; sleep 30 & echo started; wait"),
            None,
            Duration::from_millis(500),
        )
        .unwrap();
        assert!(r.timed_out && r.code.is_none() && !r.ok(), "{r:?}");
        assert!(r.output.contains("started"), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    }

    #[test]
    fn a_cancel_ends_the_command_at_once() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let t = Instant::now();
        let r = run_plan_cancel(&sh("sleep 30"), None, STEP_TIMEOUT, &cancel).unwrap();
        setter.join().unwrap();
        assert!(r.timed_out && !r.ok(), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    #[test]
    fn a_grandchild_outside_the_group_cannot_hold_the_step() {
        if Command::new("setsid").arg("--version").output().is_err() {
            eprintln!("skipped: no setsid"); // not user text
            return;
        }
        // The grandchild keeps stdout open in a session of its own.
        let t = Instant::now();
        let r = run_plan(
            &sh("setsid sleep 20 & sleep 20"),
            None,
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(r.timed_out, "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    }

    #[test]
    fn the_exit_code_and_the_timeout_text() {
        let r = run_plan(&sh("echo out; exit 3"), None, STEP_TIMEOUT).unwrap();
        assert_eq!(
            (r.code, r.timed_out, r.output.as_str()),
            (Some(3), false, "out")
        );
        let r = spawn_step(&sh("sleep 5"), None).map(|r| r.timed_out);
        assert!(!r.unwrap(), "five seconds is inside the limit");
        assert_eq!(STEP_TIMEOUT, Duration::from_secs(90));
    }

    #[test]
    fn a_step_that_times_out_fails_and_the_next_steps_still_run() {
        let mut calls = 0;
        let results = run_all(&steps(), "pw", Some("svc"), Vec::new(), |p, _| {
            calls += 1;
            if p.argv.iter().any(|a| a == "-setsecurewebproxy") {
                Ok(Ran {
                    code: None,
                    timed_out: true,
                    output: "waited for a prompt".into(),
                })
            } else {
                Ok(fine())
            }
        });
        assert_eq!(calls, 3);
        assert!(!results[0].ok && results[0].timed_out && results[0].code.is_none());
        assert!(results[1].ok && results[1].code == Some(0));
    }

    #[test]
    fn the_steps_of_the_agent_run_apart_and_land_in_their_place() {
        let mut list = steps();
        list.insert(
            1,
            ConfigureStep {
                name: "black-desktop".into(),
                as_root: false,
                argv: vec![
                    AGENT_OP.into(),
                    "desktop-picture".into(),
                    "/x/Black.png".into(),
                ],
                verify: vec![AGENT_OP.into(), "desktop-picture".into()],
            },
        );
        let mut ops = Vec::new();
        let done = agent_steps(&list, |argv| {
            ops.push(argv.to_vec());
            (true, "/x/Black.png".into())
        });
        assert_eq!(
            ops,
            [
                vec!["desktop-picture".to_string(), "/x/Black.png".to_string()],
                vec!["desktop-picture".to_string()]
            ]
        );
        assert_eq!(done.len(), 1);
        let mut spawned = Vec::new();
        let results = run_all(&list, "pw", Some("svc"), done, |p, _| {
            spawned.push(p.argv[0].clone());
            Ok(fine())
        });
        assert_eq!(
            results.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["proxy", "black-desktop", "reduce motion"]
        );
        assert!(results[1].ok && results[1].verify == "/x/Black.png");
        assert!(!spawned.iter().any(|a| a == AGENT_OP), "{spawned:?}");
        // Without its result, the step fails and says so.
        let results = run_all(&list, "pw", Some("svc"), Vec::new(), |_, _| Ok(fine()));
        assert!(!results[1].ok);
        assert!(
            results[1].output.contains("black-desktop"),
            "{}",
            results[1].output
        );
    }

    #[test]
    fn the_machine_steps_come_from_the_golden_table() {
        let list = machine_step_list();
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, MACHINE_STEPS);
        // The screen saver step is the one bound to the machine.
        let saver = list.iter().find(|s| s.name == "no-screen-saver").unwrap();
        assert!(
            saver.argv.iter().any(|a| a == "-currentHost"),
            "{:?}",
            saver.argv
        );
        // The power step runs as root: its password goes on stdin.
        assert!(list.iter().find(|s| s.name == "no-sleep").unwrap().as_root);
        // A read back proves a step only with the golden's value.
        assert!(proves("no-screen-saver-any-host", "0\n"));
        assert!(!proves("no-screen-lock", "screenLock delay is immediate"));
        assert!(proves("no-screen-lock", "screenLock is off"));
        assert!(!proves("no-such-step", "0"));
    }

    #[test]
    fn sysadminctl_gets_the_password_on_stdin() {
        let step = crate::macvm::configure::STEPS
            .iter()
            .find(|s| s.name == "no-screen-lock")
            .unwrap()
            .wire();
        let mut seen: Vec<(Vec<String>, Option<String>)> = Vec::new();
        run_all(&[step], "pw", None, Vec::new(), |p, stdin| {
            seen.push((p.argv.clone(), stdin.map(String::from)));
            Ok(fine())
        });
        assert_eq!(seen[0].0[0], "/usr/sbin/sysadminctl");
        assert_eq!(seen[0].1.as_deref(), Some("pw"));
        assert!(seen[0].0.iter().all(|a| a != "pw"), "{:?}", seen[0].0);
        // The status check asks for no password.
        assert_eq!(seen[1].1, None);
    }

    #[test]
    fn password_never_in_argv() {
        let password = "planted-password";
        let mut seen: Vec<(Vec<String>, Option<String>)> = Vec::new();
        let results = run_all(
            &steps(),
            password,
            Some("com.redhat.spice.0"),
            Vec::new(),
            |p, stdin| {
                seen.push((p.argv.clone(), stdin.map(String::from)));
                Ok(fine())
            },
        );
        // The root step: sudo -S with the password on stdin, then its verify.
        assert_eq!(
            seen[0].0,
            vec![
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "--",
                "/usr/sbin/networksetup",
                "-setsecurewebproxy",
                "com.redhat.spice.0",
                "127.0.0.1",
                "3128"
            ]
        );
        assert_eq!(seen[0].1.as_deref(), Some(password));
        assert_eq!(seen[1].0[..5], ["/usr/bin/sudo", "-S", "-p", "", "--"]);
        assert_eq!(
            seen[1].0[5..],
            [
                "/usr/sbin/networksetup",
                "-getsecurewebproxy",
                "com.redhat.spice.0"
            ]
        );
        // The user step: a plain argv, no stdin, no verify.
        assert_eq!(seen[2].0[0], "/usr/bin/defaults");
        assert_eq!(seen[2].1, None);
        assert_eq!(seen.len(), 3);
        for (argv, _) in &seen {
            assert!(argv.iter().all(|a| !a.contains(password)), "{argv:?}");
        }
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.ok));
        assert_eq!(results[0].verify, "fine");
        assert_eq!(results[1].verify, "");
    }

    #[test]
    fn a_failed_spawn_is_a_failed_step() {
        let results = run_all(&steps(), "pw", None, Vec::new(), |_, _| {
            Err(std::io::Error::other("no such file"))
        });
        assert!(results.iter().all(|r| !r.ok));
        assert!(results[0].output.contains("no such file"));
        assert!(
            results[0].output.contains("$SERVICE"),
            "without a service the placeholder stays and names the problem: {}",
            results[0].output
        );
    }

    #[test]
    fn spawn_runs_a_command_with_stdin_and_keeps_both_streams() {
        let p = Plan {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "read x; echo got $x; echo oops >&2; exit 3".into(),
            ],
            password_on_stdin: true,
        };
        let (ok, text) = spawn(&p, Some("secret")).unwrap();
        assert!(!ok);
        assert_eq!(text, "got secret\noops");
        let p = Plan {
            argv: vec!["/bin/sh".into(), "-c".into(), "exit 0".into()],
            password_on_stdin: false,
        };
        assert_eq!(spawn(&p, None).unwrap(), (true, String::new()));
        assert!(spawn(
            &Plan {
                argv: vec![],
                password_on_stdin: false
            },
            None
        )
        .is_err());
    }
}
