//! The driver of `paddock macvm bench CASE`. It starts and stops sessions
//! through this paddock binary, reads the event log of the profile, talks
//! to the control socket of the shim, and runs `ps`, `du` and the two Swift
//! helpers of `packaging/macos/bench/`. Only std and the pure `bench`
//! module: Linux type-checks it, a Mac runs it. The numbers go through
//! `bench::Measure` and the targets through `bench::check`.

use super::bench::{self, Case, Measure};
use super::host::{self, Session, DEFAULT_HOME};
use super::layout::Layout;
use super::proto::SHARE_ROOT;
use super::session::Profile;
use super::text;
use crate::env::HostEnv;
use anyhow::{anyhow, bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The folder with `keys.swift` and `frames.swift`, set by the script.
pub const TOOLS_VAR: &str = "PADDOCK_MACVM_BENCH_TOOLS";
/// The 1080p video of the `video` case, a host path.
pub const VIDEO_VAR: &str = "PADDOCK_MACVM_BENCH_VIDEO";
/// The profile with TextEdit, for `input` and `frames`.
pub const TEXT_APP_VAR: &str = "PADDOCK_MACVM_BENCH_TEXT_APP";
/// The profile with QuickTime Player, for `video`.
pub const VIDEO_APP_VAR: &str = "PADDOCK_MACVM_BENCH_VIDEO_APP";
/// The process that holds the guest memory.
pub const VM_PROCESS: &str = "com.apple.Virtualization.VirtualMachine";

/// How long a start may take to the first window.
const START_WAIT: Duration = Duration::from_secs(240);
/// How long a stop may take to the end of the shim.
const STOP_WAIT: Duration = Duration::from_secs(150);
/// How long a window of the text app may take after `bench.text`.
const WINDOW_WAIT: Duration = Duration::from_secs(60);
/// Keys of `input`, and the gap between two.
const INPUT_KEYS: u32 = 200;
const INPUT_GAP_MS: u32 = 100;
/// Seconds of the `frames` capture and of the counter around it.
const FRAMES_SECS: u32 = 3;
/// Samples of `idle` and `video`, one per second.
const CPU_SAMPLES: u32 = 60;

/// One bench of one app.
pub struct Driver<'a> {
    pub env: &'a HostEnv,
    pub layout: Layout,
    /// The profile of the case: `--app`, or the text or video app.
    pub app: String,
    pub runs: u32,
    /// Where the logs of the sessions go.
    pub log_dir: PathBuf,
}

impl<'a> Driver<'a> {
    pub fn new(env: &'a HostEnv, app: &str, runs: u32, log_dir: &Path) -> Self {
        Driver {
            env,
            layout: Layout::new(env),
            app: app.to_string(),
            runs,
            log_dir: log_dir.to_path_buf(),
        }
    }

    fn profile(&self) -> Profile {
        self.layout.profile(&self.app, DEFAULT_HOME)
    }

    fn running(&self) -> Option<Session> {
        host::running(&self.layout, &self.app, DEFAULT_HOME)
    }

    /// The session of the app, started with `--bench` when none runs. A
    /// session without `--bench` is refused with the line that says how.
    fn session(&self) -> Result<Session> {
        if let Some(s) = self.running() {
            if !s.bench {
                bail!(text::bench_needs_bench_session(&self.app));
            }
            return Ok(s);
        }
        self.start()?;
        self.running().ok_or_else(|| {
            anyhow!(text::bench_wait_timeout(
                "the session record",
                START_WAIT.as_secs()
            ))
        })
    }

    /// `paddock app run APP --backend macvm --network egress --bench`,
    /// detached, and the wait for the first window after this start. A
    /// bench measures speed with nobody at the screen: mode egress, so no
    /// firewall question waits for a person.
    fn start(&self) -> Result<()> {
        std::fs::create_dir_all(&self.log_dir)?;
        let log = std::fs::File::create(self.log_dir.join(format!("{}-run.log", self.app)))?;
        let events_before = super::events::read(&self.profile().events_log()).len();
        Command::new(std::env::current_exe()?)
            .args([
                "app",
                "run",
                &self.app,
                "--backend",
                "macvm",
                "--network",
                "egress",
                "--bench",
            ])
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .map(crate::reap::in_background)
            .with_context(|| text::bench_command_failed("paddock app run", "cannot start"))?;
        self.wait_for(START_WAIT, "the first window", || {
            after(
                &super::events::read(&self.profile().events_log()),
                events_before,
            )
            .iter()
            .any(|e| e.kind == "window" && e.text == "opened")
        })
    }

    /// `paddock app stop APP` and the wait for the record to go.
    fn stop(&self) -> Result<()> {
        if self.running().is_none() {
            return Ok(());
        }
        let status = Command::new(std::env::current_exe()?)
            .args([
                "app",
                "stop",
                &self.app,
                "--wait",
                &STOP_WAIT.as_secs().to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            bail!(text::bench_command_failed(
                "paddock app stop",
                &status.to_string()
            ));
        }
        self.wait_for(STOP_WAIT, "the end of the session", || {
            self.running().is_none()
        })
    }

    fn wait_for(&self, limit: Duration, what: &str, mut done: impl FnMut() -> bool) -> Result<()> {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if done() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!(text::bench_wait_timeout(what, limit.as_secs()))
    }

    /// One line to the control socket of the session: the first answer
    /// line, and the stream for the lines that follow.
    fn control(&self, s: &Session, line: &str) -> Result<(String, BufReader<UnixStream>)> {
        let mut c = UnixStream::connect(&s.control)
            .with_context(|| text::bench_control_unreachable(&s.control))?;
        c.set_read_timeout(Some(Duration::from_secs(30)))?;
        c.write_all(format!("{line}\n").as_bytes())?; // not user text
        let mut r = BufReader::new(c);
        let mut answer = String::new();
        r.read_line(&mut answer)?;
        let answer = answer.trim_end().to_string();
        if let Some(e) = answer.strip_prefix("error ") {
            bail!(text::bench_command_failed(line, e));
        }
        Ok((answer, r))
    }

    /// Runs one case and returns its measures.
    pub fn run(&self, case: Case) -> Result<Vec<Measure>> {
        if !cfg!(target_os = "macos") {
            bail!(text::bench_macos_only());
        }
        match case {
            Case::Warm | Case::Cold | Case::First | Case::Quit | Case::Update => {
                let mut values = Vec::new();
                for _ in 0..self.runs {
                    values.push(self.event_case(case)?);
                }
                Ok(vec![Measure::from_samples(
                    case.name(),
                    case.unit(),
                    &values,
                )])
            }
            Case::Input => self.input(),
            Case::Move => self.moves(),
            Case::Frames => self.frames(),
            Case::Idle => self.cpu(case, false),
            Case::Video => self.cpu(case, true),
            Case::Disk => self.disk(),
        }
    }

    /// One run of an event log case: the state it needs, the step, and
    /// the seconds between the two lines of the log.
    fn event_case(&self, case: Case) -> Result<f64> {
        let profile = self.profile();
        match case {
            Case::Warm => {
                // A saved state first: a session that quit with save.
                self.session()?;
                self.stop()?;
                self.start()?;
            }
            Case::Cold => {
                self.stop()?;
                // No saved state: the start repairs and boots cold with
                // the app already copied (`Start::Repair`).
                let _ = std::fs::remove_file(profile.state());
                self.start()?;
            }
            Case::First => {
                self.stop()?;
                // The plain reset: the next start boots cold and copies
                // the app again.
                let status = Command::new(std::env::current_exe()?)
                    .args(["app", "reset", &self.app, "--yes"])
                    .stdin(Stdio::null())
                    .status()?;
                if !status.success() {
                    bail!(text::bench_command_failed(
                        "paddock app reset",
                        &status.to_string()
                    ));
                }
                self.start()?;
            }
            Case::Quit => {
                self.session()?;
                self.stop()?;
            }
            Case::Update => {
                let s = self.session()?;
                let before = super::events::read(&profile.events_log()).len();
                self.control(&s, "bench agent-update")?;
                self.wait_for(
                    Duration::from_secs(60),
                    "the hello after the update",
                    || {
                        after(&super::events::read(&profile.events_log()), before)
                            .iter()
                            .any(|e| e.kind == "hello")
                    },
                )?;
            }
            _ => unreachable!("not an event case"),
        }
        let events = super::events::read(&profile.events_log());
        bench::from_events(case, &events).map_err(|e| anyhow!(e))
    }

    /// The window of the text app, focused, with its host window number.
    fn text_window(&self, s: &Session) -> Result<(u32, i64)> {
        let mut found = None;
        self.wait_for(WINDOW_WAIT, "a window of the text app", || {
            found = self
                .control(s, "window list")
                .ok()
                .and_then(|(a, _)| first_window(&a));
            found.is_some()
        })?;
        let (id, number) = found.expect("found");
        let line = format!("bench op {{\"op\":\"bench.text\",\"window\":{id}}}"); // not user text
        let (answer, _) = self.control(s, &line)?;
        log::info!("bench.text: {answer}");
        self.control(s, &format!("window raise {id}"))?; // not user text
        std::thread::sleep(Duration::from_millis(500));
        Ok((id, number))
    }

    fn tool(&self, name: &str) -> Result<PathBuf> {
        let dir = self
            .env
            .var(TOOLS_VAR)
            .ok_or_else(|| anyhow!(text::bench_tool_missing(name, TOOLS_VAR)))?;
        let p = PathBuf::from(dir).join(name);
        if !p.is_file() {
            bail!(text::bench_tool_missing(
                &p.display().to_string(),
                TOOLS_VAR
            ));
        }
        Ok(p)
    }

    /// `input`: the key helper posts the keys into the host window,
    /// `bench axt` streams the `ax.value` lines with the host clock, and
    /// the pairing by the typed text gives each latency.
    fn input(&self) -> Result<Vec<Measure>> {
        let keys = self.tool("keys.swift")?;
        let s = self.session()?;
        let (_, number) = self.text_window(&s)?;
        let secs = (INPUT_KEYS * INPUT_GAP_MS) / 1000 + 10;
        let (_, reader) = self.control(&s, &format!("bench axt {secs}"))?; // not user text
        let out = Command::new("/usr/bin/swift")
            .arg(&keys)
            .args([
                number.to_string(),
                INPUT_KEYS.to_string(),
                INPUT_GAP_MS.to_string(),
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()?;
        if !out.status.success() {
            bail!(text::bench_command_failed(
                "keys.swift",
                &out.status.to_string()
            ));
        }
        let sent = parse_keys(&String::from_utf8_lossy(&out.stdout));
        let mut seen = Vec::new();
        for line in reader.lines().map_while(Result::ok) {
            if let Some(v) = parse_axt(&line) {
                seen.push(v);
            }
        }
        let values = bench::pair_input(&sent, &seen);
        Ok(vec![Measure::from_samples("input", "ms", &values)])
    }

    /// `move`: `runs` times the sample count, one injected `windows`
    /// event each, and the `ms` the shim wrote for each.
    fn moves(&self) -> Result<Vec<Measure>> {
        let s = self.session()?;
        let profile = self.profile();
        let (list, _) = self.control(&s, "window list")?;
        let (id, _) =
            first_window(&list).ok_or_else(|| anyhow!(text::bench_wait_timeout("a window", 0)))?;
        let before = super::events::read(&profile.events_log()).len();
        let count = 20 * self.runs.max(1);
        for i in 0..count {
            let x = 100 + (i % 10) as i64 * 20;
            let ev = format!(
                // not user text
                "{{\"order\":[{id}],\"changed\":[{{\"id\":{id},\"pid\":1,\"app\":\"bench\",\"title\":\"bench\",\"frame\":{{\"x\":{x},\"y\":100,\"w\":800,\"h\":600}},\"level\":\"normal\",\"parent\":null,\"minimized\":false,\"focused\":true,\"resizable\":true,\"movable\":true}}],\"removed\":[]}}"
            );
            self.control(&s, &format!("bench windows {ev}"))?; // not user text
            std::thread::sleep(Duration::from_millis(50));
        }
        let events = super::events::read(&profile.events_log());
        let values = bench::move_samples(after(&events, before));
        Ok(vec![Measure::from_samples("move", "ms", &values)])
    }

    /// `frames`: the counter in the guest, the capture of the host window
    /// by the frames helper, and the gaps between distinct frames.
    fn frames(&self) -> Result<Vec<Measure>> {
        let frames = self.tool("frames.swift")?;
        let s = self.session()?;
        let (id, number) = self.text_window(&s)?;
        let secs = FRAMES_SECS + 2;
        self.control(
            &s,
            &format!("bench op {{\"op\":\"bench.animate\",\"window\":{id},\"secs\":{secs}}}"), // not user text
        )?;
        std::thread::sleep(Duration::from_millis(500));
        let out = Command::new("/usr/bin/swift")
            .arg(&frames)
            .args([number.to_string(), FRAMES_SECS.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()?;
        if !out.status.success() {
            bail!(text::bench_command_failed(
                "frames.swift",
                &out.status.to_string()
            ));
        }
        let times = parse_frames(&String::from_utf8_lossy(&out.stdout));
        let gaps = bench::frame_gaps(&times);
        Ok(vec![Measure::from_samples("frames", "ms", &gaps)])
    }

    /// `idle` and `video`: `ps` of the VM process each second, and the
    /// guest load from the session record.
    fn cpu(&self, case: Case, video: bool) -> Result<Vec<Measure>> {
        let s = self.session()?;
        if video {
            let file = self
                .env
                .var(VIDEO_VAR)
                .map(PathBuf::from)
                .filter(|p| p.is_file())
                .ok_or_else(|| anyhow!(text::bench_tool_missing("the 1080p video", VIDEO_VAR)))?;
            let dir = self.profile().drop_dir().join("bench");
            std::fs::create_dir_all(&dir)?;
            let name = file
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            std::fs::copy(&file, dir.join(&name))?;
            let guest = Path::new(SHARE_ROOT).join("drop").join("bench").join(&name);
            let argv = serde_json::json!(["/usr/bin/open", "-a", "QuickTime Player", guest]);
            let line = format!("bench op {{\"op\":\"bench.spawn_child\",\"argv\":{argv}}}"); // not user text
            self.control(&s, &line)?;
            std::thread::sleep(Duration::from_secs(5));
        }
        let ps = Command::new("/bin/ps")
            .args(["-axo", "pid=,lstart=,comm="])
            .output()?;
        let pid = bench::newest_process(&String::from_utf8_lossy(&ps.stdout), VM_PROCESS)
            .ok_or_else(|| anyhow!(text::bench_tool_missing(VM_PROCESS, "a running session")))?;
        let mut cpu = Vec::new();
        let mut ram = Vec::new();
        let mut load = Vec::new();
        for i in 0..CPU_SAMPLES {
            let out = Command::new("/bin/ps")
                .args(["-o", "%cpu=,rss=", "-p", &pid.to_string()])
                .output()?;
            if let Some((c, r)) = bench::parse_ps_sample(&String::from_utf8_lossy(&out.stdout)) {
                cpu.push(c);
                ram.push(r);
            }
            if i % 5 == 0 {
                if let Some(h) = self.running().and_then(|s| s.health) {
                    load.push(h.load);
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        if video {
            return Ok(vec![Measure::from_samples("video", "pct", &cpu)]);
        }
        let _ = case;
        Ok(vec![
            Measure::from_samples("idle.cpu", "pct", &cpu),
            Measure::from_samples("idle.ram", "gib", &ram),
            Measure::from_samples("idle.load", "load", &load),
        ])
    }

    /// `disk`: `du` of the profile without its app copy, in GiB.
    fn disk(&self) -> Result<Vec<Measure>> {
        let profile = self.profile();
        let whole = du_gib(&profile.dir)?;
        let apps = du_gib(&profile.apps()).unwrap_or(0.0);
        Ok(vec![Measure::from_samples(
            "disk",
            "gib",
            &[(whole - apps).max(0.0)],
        )])
    }
}

/// The events from index `from`, or none when the log was trimmed
/// under it meanwhile.
fn after(events: &[super::events::Event], from: usize) -> &[super::events::Event] {
    events.get(from..).unwrap_or(&[])
}

fn du_gib(path: &Path) -> Result<f64> {
    let out = Command::new("/usr/bin/du")
        .args(["-sk"])
        .arg(path)
        .output()?;
    bench::parse_du_gib(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        anyhow!(text::bench_command_failed(
            "du",
            &String::from_utf8_lossy(&out.stderr)
        ))
    })
}

/// The id and the host window number of the first window of a `window
/// list` answer (`ok [...]`).
pub fn first_window(answer: &str) -> Option<(u32, i64)> {
    let json = answer.strip_prefix("ok ").unwrap_or(answer);
    let list: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    list.iter().find_map(|w| {
        let id = w.get("id")?.as_u64()? as u32;
        let number = w.get("number")?.as_i64()?;
        Some((id, number))
    })
}

/// The `KEY c T_MS` lines of the key helper.
pub fn parse_keys(out: &str) -> Vec<(char, u64)> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            if it.next()? != "KEY" {
                return None;
            }
            let c = it.next()?.chars().next()?;
            let t = it.next()?.parse().ok()?;
            Some((c, t))
        })
        .collect()
}

/// A `T_MS WINDOW TEXT` line of `bench axt`.
pub fn parse_axt(line: &str) -> Option<(u64, String)> {
    let mut it = line.splitn(3, ' ');
    let t = it.next()?.parse().ok()?;
    let _window = it.next()?;
    Some((t, it.next()?.to_string()))
}

/// The `FRAME T_MS` lines of the frames helper, as milliseconds.
pub fn parse_frames(out: &str) -> Vec<f64> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            if it.next()? != "FRAME" {
                return None;
            }
            it.next()?.parse().ok()
        })
        .collect()
}
