//! TMP1-F2 on a real Mac, end to end: a Seatbelt run claims a name in
//! `/private/tmp` and starts a server that leaves the process group
//! (setsid) with a helper under it, as codex does with its app server and
//! its MCP helpers. Every kind of end must stop both and free the name: a
//! normal exit, SIGTERM, SIGHUP, a closed terminal, and a kill -9 of paddock
//! (then the next paddock command cleans up), also when another paddock
//! process removes the run folder first (TMP1-F5). A start also stops a stale
//! server of an earlier run of the same home that lost its record. A host
//! process of the same program is never touched.
#![cfg(target_os = "macos")]

mod common;

use common::device_world::{alive, finish, wait_for, World};
use common::pty::spawn_on_pty;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The harness: its entry in the claimed folder, then the server (perl:
/// fork, setsid, a helper `sleep`), then `flags/started`. `flags/mode`:
/// `exit` ends it at once, `wait` waits for a signal.
const AGENT: &str = r#"
h=$(cd "$HOME" && pwd -P)
e=$(printf %s "$h/srv" | /usr/bin/shasum -a 256 | cut -c1-64)
printf %s "$e" > flags/entry
if : > "/tmp/@FOLDER@/$e"; then echo ENTRY_OK > flags/entry_ok; fi
rm -f flags/server
/usr/bin/perl -MPOSIX -e 'exit 0 if fork; POSIX::setsid(); my $k = fork; if (!$k) { exec "/bin/sleep", "300" } open(my $f, ">", "flags/server.tmp"); print $f "$$ $k\n"; close $f; rename "flags/server.tmp", "flags/server"; sleep 300'
await flags/server
: > flags/started
if [ "$(cat flags/mode)" = wait ]; then await flags/never; fi
"#;

struct Case {
    w: World,
    folder: String,
}

impl Case {
    fn new(tag: &str) -> Self {
        let w = World::new("");
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = format!("paddock-f2-{tag}-{}-{n:x}", std::process::id());
        std::fs::create_dir_all(w.root.join("project/flags")).unwrap();
        let c = Case { w, folder };
        c.profile();
        c
    }

    /// The fake harness, with a `[[macos.tmp]]` claim like codex's.
    fn profile(&self) {
        let script = format!(
            "{}{}",
            common_await(),
            AGENT.replace("@FOLDER@", &self.folder)
        );
        std::fs::write(
            self.w.root.join("cfg/paddock/harness.d/tmpend.toml"),
            format!(
                "name = \"tmpend\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n\
                 [[macos.tmp]]\nname = \"{}\"\nkind = \"shared\"\nentries = [\"{{sha256:{{home}}/srv}}\"]\n",
                serde_json::to_string(&script).unwrap(),
                self.folder
            ),
        )
        .unwrap();
    }

    fn flags(&self) -> PathBuf {
        self.w.root.join("project/flags")
    }

    fn command(&self, mode: &str) -> Command {
        std::fs::write(self.flags().join("mode"), mode).unwrap();
        let mut c = self.w.paddock();
        c.args([
            "run",
            "tmpend",
            "--backend",
            "seatbelt",
            "--network",
            "none",
            "--env",
            "clean",
            "-y",
            "--direct",
        ])
        .arg(self.w.root.join("project"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        c
    }

    fn start(&self, mode: &str) -> std::process::Child {
        self.command(mode).spawn().unwrap()
    }

    fn started(&self) {
        wait_for("the harness start", Duration::from_secs(60), || {
            self.flags().join("started").exists()
        });
    }

    /// The pids of the server and its helper.
    fn server(&self) -> Vec<u64> {
        std::fs::read_to_string(self.flags().join("server"))
            .unwrap()
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect()
    }

    fn entry(&self) -> PathBuf {
        let e = std::fs::read_to_string(self.flags().join("entry")).unwrap();
        Path::new("/private/tmp").join(&self.folder).join(e.trim())
    }

    /// Nothing of the run is left: no process, no name, no record.
    /// `paddock tmp debug`: what the end of a run sees (TMP1-F3).
    fn debug(&self) -> String {
        match self.w.paddock().args(["tmp", "debug"]).output() {
            Ok(o) => say(&o),
            Err(e) => format!("paddock tmp debug: {e}"),
        }
    }

    /// Wait for `f`. When it does not happen, the panic shows `paddock
    /// tmp debug` too.
    fn wait(&self, what: &str, within: Duration, f: impl Fn() -> bool) {
        let t = std::time::Instant::now();
        while !f() {
            if t.elapsed() >= within {
                panic!(
                    "{what} did not happen within {within:?}\n--- paddock tmp debug:\n{}",
                    self.debug()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn nothing_left(&self, pids: &[u64], what: &str) {
        self.wait(
            &format!("{what}: the server ends"),
            Duration::from_secs(15),
            || pids.iter().all(|&p| !alive(p)),
        );
        assert!(
            std::fs::symlink_metadata(self.entry()).is_err(),
            "{what}: the name goes"
        );
        assert!(
            !Path::new("/private/tmp").join(&self.folder).exists(),
            "{what}: paddock made the folder, so it goes"
        );
        let records = self.w.root.join("state/paddock/tmp");
        let left: Vec<_> = std::fs::read_dir(&records)
            .map(|d| {
                d.flatten()
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        assert!(left.is_empty(), "{what}: records left: {left:?}");
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        if let Ok(pids) = std::fs::read_to_string(self.flags().join("server")) {
            for p in pids
                .split_whitespace()
                .filter_map(|p| p.parse::<i32>().ok())
            {
                // SAFETY: SIGKILL to the test's own leftover, if any.
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_dir_all(Path::new("/private/tmp").join(&self.folder));
    }
}

fn common_await() -> &'static str {
    "await() { n=0; while [ ! -e \"$1\" ]; do sleep 0.1; n=$((n+1)); if [ $n -ge 3000 ]; then echo \"TIMEOUT: no $1\"; exit 124; fi; done; }\n"
}

/// A host process of the same program as the helper: it must survive.
struct Decoy(std::process::Child);

impl Decoy {
    fn new() -> Self {
        Decoy(Command::new("/bin/sleep").arg("300").spawn().unwrap())
    }
    fn assert_alive(&mut self) {
        assert!(
            self.0.try_wait().unwrap().is_none(),
            "a host process was stopped"
        );
    }
}

impl Drop for Decoy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn say(o: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn signal(child: &std::process::Child, sig: i32) {
    // SAFETY: a signal to our own child, which has not been reaped.
    unsafe { libc::kill(child.id() as i32, sig) };
}

#[test]
fn a_normal_exit_stops_the_server_and_frees_its_name() {
    let c = Case::new("exit");
    let mut decoy = Decoy::new();
    let out = finish(c.start("exit"), Duration::from_secs(120));
    assert!(c.flags().join("entry_ok").exists(), "{}", say(&out));
    let pids = c.server();
    assert_eq!(pids.len(), 2, "{}", say(&out));
    c.nothing_left(&pids, &format!("exit\n{}", say(&out)));
    decoy.assert_alive();
}

#[test]
fn sigterm_to_paddock_stops_the_server_and_frees_its_name() {
    let c = Case::new("term");
    let mut decoy = Decoy::new();
    let child = c.start("wait");
    c.started();
    let pids = c.server();
    signal(&child, libc::SIGTERM);
    let out = finish(child, Duration::from_secs(60));
    c.nothing_left(&pids, &format!("SIGTERM\n{}", say(&out)));
    decoy.assert_alive();
}

#[test]
fn sighup_to_paddock_stops_the_server_and_frees_its_name() {
    let c = Case::new("hup");
    let mut decoy = Decoy::new();
    let child = c.start("wait");
    c.started();
    let pids = c.server();
    signal(&child, libc::SIGHUP);
    let out = finish(child, Duration::from_secs(60));
    c.nothing_left(&pids, &format!("SIGHUP\n{}", say(&out)));
    decoy.assert_alive();
}

#[test]
fn a_closed_terminal_stops_the_server_and_frees_its_name() {
    use std::os::unix::process::CommandExt as _;
    let c = Case::new("tty");
    let mut decoy = Decoy::new();
    // A terminal that closes: SIGHUP to the whole process group, and the
    // output goes nowhere (the reading ends close first).
    let mut child = c.command("wait").process_group(0).spawn().unwrap();
    c.started();
    let pids = c.server();
    drop(child.stdout.take());
    drop(child.stderr.take());
    // SAFETY: SIGHUP to the process group of our own child.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGHUP) };
    let out = finish(child, Duration::from_secs(60));
    c.nothing_left(&pids, &format!("closed terminal\n{}", say(&out)));
    decoy.assert_alive();
}

#[test]
fn after_a_kill_9_of_paddock_the_next_command_cleans_up() {
    let c = Case::new("kill");
    let mut decoy = Decoy::new();
    let child = c.start("wait");
    c.started();
    let pids = c.server();
    signal(&child, libc::SIGKILL);
    let _ = finish(child, Duration::from_secs(30));
    // paddock had no end step: the server and its name are still there.
    assert!(pids.iter().all(|&p| alive(p)));
    assert!(std::fs::symlink_metadata(c.entry()).is_ok());
    let out = c.w.paddock().arg("ps").output().unwrap();
    assert!(out.status.success(), "{}", say(&out));
    c.nothing_left(&pids, &format!("the next command\n{}", say(&out)));
    decoy.assert_alive();
}

#[test]
fn a_start_stops_a_stale_server_of_the_same_home() {
    let c = Case::new("stale");
    let mut decoy = Decoy::new();
    let child = c.start("wait");
    c.started();
    let old = c.server();
    signal(&child, libc::SIGKILL);
    let _ = finish(child, Duration::from_secs(30));
    // The record is lost too (the state of TMP1 before this fix): only the
    // sandbox of the server tells whose it is.
    let _ = std::fs::remove_dir_all(c.w.root.join("state/paddock/tmp"));
    assert!(old.iter().all(|&p| alive(p)));
    for f in ["started", "server", "entry_ok"] {
        let _ = std::fs::remove_file(c.flags().join(f));
    }
    let out = finish(c.start("exit"), Duration::from_secs(120));
    assert!(
        old.iter().all(|&p| !alive(p)),
        "the stale server stops: {}\n--- paddock tmp debug:\n{}",
        say(&out),
        c.debug()
    );
    assert!(
        c.flags().join("entry_ok").exists(),
        "the new run gets the name again: {}",
        say(&out)
    );
    let new = c.server();
    c.nothing_left(&new, "the second run");
    decoy.assert_alive();
}

impl Case {
    /// The run folders that are there now (with their `info.json`).
    fn run_folders(&self) -> Vec<PathBuf> {
        ["run/paddock", "state/paddock/run"]
            .iter()
            .filter_map(|d| std::fs::read_dir(self.w.root.join(d)).ok())
            .flat_map(|rd| rd.flatten().map(|e| e.path()))
            .filter(|p| p.join("info.json").exists())
            .collect()
    }
}

/// TMP1-F5, the path of the Mac: `paddock run` runs in a terminal (tmux),
/// the terminal closes, and paddock dies without its end step. Another
/// paddock process lists the runs (the marker does it on the Mac, and
/// `paddock tmp debug` too) and so removes the run folder. The next
/// paddock command must still stop the server and its helper and free
/// the name.
#[test]
fn a_closed_terminal_and_the_run_folder_cleanup_still_stop_the_server() {
    let c = Case::new("ttyclean");
    let mut decoy = Decoy::new();
    let (child, terminal) = spawn_on_pty(c.command("wait"));
    c.started();
    let pids = c.server();
    assert!(!c.run_folders().is_empty(), "the run has its run folder");
    // paddock goes without its end step, as on the Mac, and the terminal
    // closes. The kill comes first, so no end step can start.
    signal(&child, libc::SIGKILL);
    terminal.close();
    let _ = finish(child, Duration::from_secs(30));
    assert!(pids.iter().all(|&p| alive(p)), "no end step ran");
    // The real cleanup of the run folders: a paddock process lists the runs.
    let debug = c.debug();
    assert!(
        c.run_folders().is_empty(),
        "the listing removes the run folder of a dead paddock: {:?}\n{debug}",
        c.run_folders()
    );
    let out = c.w.paddock().arg("ps").output().unwrap();
    assert!(out.status.success(), "{}", say(&out));
    c.nothing_left(
        &pids,
        &format!("closed terminal, run folder gone\n{debug}\n{}", say(&out)),
    );
    decoy.assert_alive();
}

/// CORE5, the path of the Mac without a kill: `paddock run` runs on a real
/// terminal (a tmux pane) and the terminal closes. paddock is the session
/// leader of the pty, as when the pane runs it directly. Its end step must
/// run: it stops the server and its helper and frees the name.
#[test]
fn a_closed_real_terminal_runs_the_end_step() {
    let c = Case::new("ttyreal");
    let mut decoy = Decoy::new();
    let (child, terminal) = spawn_on_pty(c.command("wait"));
    c.started();
    let pids = c.server();
    // The terminal closes. No kill: paddock must notice by itself.
    terminal.close();
    let out = finish(child, Duration::from_secs(60));
    let debug = c.debug();
    assert!(
        debug.contains("last end step: ") && !debug.contains("last end step: none"),
        "the end step ran. paddock status: {:?}\n{debug}",
        out.status
    );
    c.nothing_left(
        &pids,
        &format!("closed real terminal, status {:?}\n{debug}", out.status),
    );
    decoy.assert_alive();
}
