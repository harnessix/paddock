//! `paddock gateway-serve NAME`: the supervisor of one sandboxed gateway.
//!
//! systemd (a transient user unit), launchd (a LaunchAgent) or a detached
//! paddock process runs it. It starts the gateway sandbox, waits for the
//! port, checks that only the loopback listens there, and writes
//! `state.json`. A gateway that ends without a stop starts again after 1
//! and 2 seconds. The third end within a minute makes the gateway
//! `failed`, and the supervisor exits with 1.
//!
//! SIGTERM or SIGINT stop the gateway (SIGTERM to its process group, then
//! SIGKILL after 3 seconds) and remove the state file.

use super::launch;
use super::state::{self, Phase, StartCfg, State};
use crate::env::HostEnv;
use crate::profile::HarnessFactory;
use anyhow::{bail, Context as _, Result};
use std::collections::VecDeque;
use std::io::Write as _;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Ends within this window count toward `failed`.
const WINDOW: Duration = Duration::from_secs(60);
/// The wait before a start after the first and the second end.
const BACKOFF: [u64; 2] = [1, 2];
/// The end that makes the gateway `failed`.
const MAX_DEATHS: usize = 3;
/// How long a start waits for the port.
pub const READY: Duration = Duration::from_secs(60);
/// How long a stop waits after SIGTERM.
const GRACE: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(100);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn stopping() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// A line of the supervisor in the gateway log.
fn note(env: &HostEnv, name: &str, text: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state::log_file(env, name))
    {
        let _ = writeln!(f, "paddock: {text}");
    }
}

/// The supervisor holds this lock while it runs: one per gateway. The
/// lock ends with the file.
struct Lock {
    _file: std::fs::File,
}

impl Lock {
    fn take(env: &HostEnv, name: &str) -> Result<Self> {
        let p = state::dir(env, name).join("serve.lock");
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&p)
            .with_context(|| format!("cannot open {}", p.display()))?;
        use std::os::fd::AsRawFd as _;
        // SAFETY: flock on a descriptor that this function owns.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("a supervisor of the {name} gateway runs already");
        }
        Ok(Self { _file: f })
    }
}

/// Stop a gateway process group: SIGTERM, then SIGKILL.
fn end(child: &mut Child) {
    let pid = child.id() as i32;
    // SAFETY: signals to the process group that this supervisor started.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let until = Instant::now() + GRACE;
    while Instant::now() < until {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(TICK);
    }
    // SAFETY: as above. What is left of the group goes too.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
}

/// How a gateway process ended.
fn exit_text(s: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;
    match (s.code(), s.signal()) {
        (Some(c), _) => format!("exit status {c}"),
        (None, Some(sig)) => format!("signal {sig}"),
        _ => "unknown".into(),
    }
}

/// The loopback check: an address other than the loopback that listens on
/// the port is refused. The text names it.
pub fn loopback_violation(port: u16) -> Option<String> {
    let addrs = launch::listeners(port)?;
    let bad: Vec<String> = addrs
        .into_iter()
        .filter(|a| !launch::is_loopback(a))
        .collect();
    (!bad.is_empty()).then(|| {
        format!(
            "the gateway listens on {} port {port}, not on the loopback only: paddock stopped it",
            bad.join(", ")
        )
    })
}

enum Outcome {
    /// A stop: exit 0.
    Stopped,
    /// The gateway ended by itself.
    Ended(String),
    /// A refusal that a restart cannot fix.
    Refused(String),
}

/// Run one gateway until it ends or a stop comes.
fn run_once(env: &HostEnv, st: &mut State, child: &mut Child) -> Outcome {
    let start = Instant::now();
    let mut ready = false;
    loop {
        if stopping() {
            end(child);
            return Outcome::Stopped;
        }
        if let Ok(Some(s)) = child.try_wait() {
            return Outcome::Ended(exit_text(s));
        }
        if !ready && launch::answers(st.port) {
            if let Some(why) = loopback_violation(st.port) {
                end(child);
                return Outcome::Refused(why);
            }
            ready = true;
            st.phase = Phase::Running;
            let _ = state::write(env, st);
            note(env, &st.name, &format!("the gateway listens on {}", st.url));
        }
        if !ready && start.elapsed() > READY {
            end(child);
            return Outcome::Ended(format!(
                "the port {} did not answer within {} s",
                st.port,
                READY.as_secs()
            ));
        }
        std::thread::sleep(TICK);
    }
}

/// `paddock gateway-serve NAME`.
pub fn main(env: &HostEnv, name: &str) -> Result<i32> {
    crate::update::refuse_in_sandbox(env, "paddock gateway-serve")?;
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: a plain sigaction(2) with a handler that stores a flag.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    state::make(&state::dir(env, name))?;
    let _lock = Lock::take(env, name)?;
    let cfg: StartCfg = state::read_start(env, name).with_context(|| {
        format!("the {name} gateway has no start settings: paddock gateway {name} start")
    })?;
    let token = state::existing_token(env, name).with_context(|| {
        format!("the {name} gateway has no token: paddock gateway {name} start")
    })?;
    let factory = HarnessFactory::new(env)?;
    let harness = factory.create(name)?;
    let shape = super::shape_of(&harness.profile)?;
    let net: crate::spec::Network = cfg.network.parse()?;
    let secrets = state::read_secrets(env, name);
    let now = crate::state::now();
    let mut st = State {
        v: state::VERSION,
        name: name.to_string(),
        phase: Phase::Starting,
        pid: std::process::id(),
        child: 0,
        port: cfg.port,
        url: shape.url(&super::shape::Values {
            port: cfg.port,
            token: "",
        }),
        started: now,
        since: now,
        restarts: 0,
        last_exit: None,
        backend: String::new(),
        network: cfg.network.clone(),
        how: cfg.how.clone(),
        unit: cfg.unit.clone(),
        reason: None,
    };
    let mut deaths: VecDeque<Instant> = VecDeque::new();
    loop {
        let fail = |st: &mut State, why: String| -> Result<i32> {
            note(env, name, &why);
            st.phase = Phase::Failed;
            st.child = 0;
            st.reason = Some(why);
            state::write(env, st)?;
            Ok(1)
        };
        let prepared = super::spec::gateway(env, &harness, &shape, net, cfg.port, &token, &secrets)
            .and_then(|spec| launch::prepare(env, &spec, &cfg.backend, false));
        let mut p = match prepared {
            Ok(p) => p,
            Err(e) => return fail(&mut st, format!("cannot start the gateway: {e:#}")),
        };
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state::log_file(env, name))?;
        p.cmd
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        {
            use std::os::unix::process::CommandExt as _;
            p.cmd.process_group(0);
        }
        let mut child = match p.cmd.spawn() {
            Ok(c) => c,
            Err(e) => return fail(&mut st, format!("cannot start the gateway sandbox: {e}")),
        };
        st.child = child.id();
        st.since = crate::state::now();
        st.backend = p.backend.clone();
        st.reason = p.reason.clone();
        st.phase = Phase::Starting;
        state::write(env, &st)?;
        note(
            env,
            name,
            &format!(
                "started the gateway sandbox ({}), pid {}",
                p.backend, st.child
            ),
        );
        match run_once(env, &mut st, &mut child) {
            Outcome::Stopped => {
                note(env, name, "stopped");
                state::remove(env, name);
                return Ok(0);
            }
            Outcome::Refused(why) => return fail(&mut st, why),
            Outcome::Ended(how) => {
                note(env, name, &format!("the gateway ended: {how}"));
                st.last_exit = Some(how.clone());
                st.child = 0;
                let t = Instant::now();
                deaths.push_back(t);
                while deaths
                    .front()
                    .is_some_and(|d| t.duration_since(*d) > WINDOW)
                {
                    deaths.pop_front();
                }
                if deaths.len() >= MAX_DEATHS {
                    return fail(
                        &mut st,
                        format!(
                            "the gateway ended {MAX_DEATHS} times within {} s (last: {how}). paddock gateway {name} logs shows why; paddock gateway {name} start tries again",
                            WINDOW.as_secs()
                        ),
                    );
                }
                st.restarts += 1;
                st.phase = Phase::Restarting;
                state::write(env, &st)?;
                let wait = Duration::from_secs(BACKOFF[(deaths.len() - 1).min(BACKOFF.len() - 1)]);
                let until = Instant::now() + wait;
                while Instant::now() < until {
                    if stopping() {
                        state::remove(env, name);
                        return Ok(0);
                    }
                    std::thread::sleep(TICK);
                }
            }
        }
    }
}
