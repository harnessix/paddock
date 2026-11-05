//! The stand-in against a real tackd server in this process, with a fake
//! process table and fake values, and real programs: the arguments, the
//! environment, the streams (pipes and a pty), the signals both ways, the
//! exit codes and the deaths by signal.

use super::*;
use crate::credentials::secret::Secret;
use crate::tack::audit::{Log, Record};
use crate::tack::config::{self, Tack};
use crate::tack::daemon::{bind, Server, Users};
use crate::tack::procs::Fake;
use crate::tack::start::{Linux, Values, Who};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

const FAKE: &str = "fake-secret-value-123";

fn me() -> u32 {
    unsafe { libc::geteuid() }
}

struct OneUser(String);

impl Users for OneUser {
    fn config(&self, uid: u32) -> Result<Tack, String> {
        if uid != me() {
            return Ok(Tack::default());
        }
        config::parse(&self.0).map_err(|e| format!("{e:#}"))
    }
}

struct FakeValues;

impl Values for FakeValues {
    fn fetch(
        &self,
        who: &Who,
        _: &Tack,
        names: &[String],
    ) -> Result<Vec<(String, Secret)>, String> {
        assert_eq!(who.caller.uid, me());
        assert!(!who.drop_user, "a test runs as the caller's own user");
        Ok(names
            .iter()
            .map(|n| (n.clone(), Secret::from(FAKE)))
            .collect())
    }
}

struct Tackd {
    t: tempfile::TempDir,
    sock: PathBuf,
    prog: PathBuf,
    _server: Arc<Server>,
}

fn table() -> Fake {
    let mut t = Fake::default().add(1, 0, "/sbin/init", &[]).add(
        std::process::id(),
        1,
        "/usr/bin/paddock",
        &["paddock", "tack"],
    );
    t.0.get_mut(&std::process::id()).unwrap().uid = me();
    t
}

/// A tackd whose allowlist names `prog` (a script with `body`, or a copy
/// of the file `copy`) for GH_TOKEN.
fn tackd_with(body: Option<&str>, copy: Option<&str>, listed: bool) -> Tackd {
    let t = tempfile::tempdir().unwrap();
    let prog = t.path().join("tool");
    match (body, copy) {
        (Some(b), _) => std::fs::write(&prog, b).unwrap(),
        (_, Some(c)) => {
            std::fs::copy(c, &prog).unwrap();
        }
        _ => unreachable!(),
    }
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = if listed {
        format!(
            "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{}\"\n",
            prog.display(),
            crate::tack::program::sha256_file(&prog).unwrap()
        )
    } else {
        "[tack.secrets.GH_TOKEN]\n".to_string()
    };
    let audit = Log::new(t.path().join("log/tackd.log"), me());
    let mut server = Server::new(Box::new(table()), Box::new(OneUser(cfg)), audit);
    let mut l = Linux::new(Box::new(FakeValues));
    l.cache = server.cache.clone();
    // No cgroup moves in tests.
    l.cgroups = t.path().join("no-cgroups");
    server.starter = Some(Box::new(l));
    let server = Arc::new(server);
    let sock = t.path().join("run/tackd.sock");
    let lst = bind(&sock, me()).unwrap();
    let s = server.clone();
    std::thread::spawn(move || s.serve(lst));
    Tackd {
        t,
        sock,
        prog,
        _server: server,
    }
}

fn tackd(body: &str) -> Tackd {
    tackd_with(Some(body), None, true)
}

fn req(t: &Tackd, args: &[&str], stdio: [bool; 3]) -> Start {
    Start {
        program: t.prog.display().to_string(),
        argv0: "tool".into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: t.t.path().display().to_string(),
        caller_pid: std::process::id(),
        env: vec![
            "PATH=/usr/bin:/bin".into(),
            "GH_TOKEN=tack://GH_TOKEN".into(),
            "LD_PRELOAD=/nonexistent/evil.so".into(),
            "KEEP=kept".into(),
        ],
        stdio,
    }
}

fn pipe() -> (OwnedFd, OwnedFd) {
    let mut p = [0; 2];
    assert_eq!(unsafe { libc::pipe2(p.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    unsafe { (OwnedFd::from_raw_fd(p[0]), OwnedFd::from_raw_fd(p[1])) }
}

fn devnull() -> OwnedFd {
    std::fs::File::open("/dev/null").unwrap().into()
}

/// Run a session with stdin from /dev/null and stdout and stderr into a
/// pipe; returns the outcome and the output.
fn run_piped(
    t: &Tackd,
    args: &[&str],
    signals: Option<RawFd>,
    on_stop: &mut dyn FnMut(i32),
    out: (OwnedFd, OwnedFd),
) -> (Outcome, String) {
    let (r, w) = out;
    let null = devnull();
    let s = UnixStream::connect(&t.sock).unwrap();
    let o = session(
        &s,
        &req(t, args, [true, true, true]),
        &[null.as_raw_fd(), w.as_raw_fd(), w.as_raw_fd()],
        signals,
        on_stop,
    )
    .unwrap();
    drop(w);
    let mut text = String::new();
    std::fs::File::from(r).read_to_string(&mut text).unwrap();
    (o, text)
}

fn audit(t: &Tackd) -> Vec<Record> {
    Log::new(t.t.path().join("log/tackd.log"), me())
        .read(None, 100)
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// tackd writes the "ended" line after it told the stand-in.
fn wait_ended(t: &Tackd) {
    for _ in 0..300 {
        if audit(t).iter().any(|r| r.decision == "ended") {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_program_gets_the_value_arguments_folder_and_its_exit_code() {
    let t = tackd(
        "#!/bin/sh\necho \"token=$GH_TOKEN keep=$KEEP preload=${LD_PRELOAD-unset} pwd=$(pwd) args=$*\"\necho to-stderr >&2\nexit 7\n",
    );
    let (o, out) = run_piped(&t, &["pr", "list x"], None, &mut |_| {}, pipe());
    assert_eq!(o, Outcome::Exited(7));
    wait_ended(&t);
    let want = format!(
        "token={FAKE} keep=kept preload=unset pwd={} args=pr list x\nto-stderr\n",
        std::fs::canonicalize(t.t.path()).unwrap().display()
    );
    assert_eq!(out, want);
    let recs = audit(&t);
    let kinds: Vec<&str> = recs.iter().map(|r| r.decision.as_str()).collect();
    assert_eq!(kinds, ["allow", "started", "ended"], "{recs:?}");
    assert!(
        recs[1].reason.contains("unprotected (phase 4)"),
        "{:?}",
        recs[1]
    );
    assert!(
        recs[1].reason.contains("dropped LD_PRELOAD"),
        "{:?}",
        recs[1]
    );
    assert!(recs[2].reason.contains("exit code 7"), "{:?}", recs[2]);
    // No line holds the value or an argument.
    let raw = std::fs::read_to_string(t.t.path().join("log/tackd.log")).unwrap();
    assert!(!raw.contains(FAKE) && !raw.contains("list x"), "{raw}");
}

#[test]
fn a_binary_starts_from_the_checked_file_with_its_name() {
    // A copy of a real program: started through execveat on the file that
    // tackd hashed. `true`/`false` may be one multi-call binary that
    // reads its name, which the start passes as argument 0.
    let t = tackd_with(None, Some("/bin/false"), true);
    let s = UnixStream::connect(&t.sock).unwrap();
    let mut r = req(&t, &[], [false, false, false]);
    r.argv0 = "false".into();
    let o = session(&s, &r, &[], None, &mut |_| {}).unwrap();
    assert_eq!(o, Outcome::Exited(1));
    let started = audit(&t)
        .into_iter()
        .find(|r| r.decision == "started")
        .unwrap();
    assert!(!started.reason.contains("a script"), "{started:?}");
}

#[test]
fn a_program_that_changed_after_the_check_does_not_start() {
    let t = tackd("#!/bin/sh\nexit 0\n");
    // The allowlist pins the old content: the decision denies.
    std::fs::write(&t.prog, "#!/bin/sh\necho other\n").unwrap();
    let s = UnixStream::connect(&t.sock).unwrap();
    let o = session(&s, &req(&t, &[], [false; 3]), &[], None, &mut |_| {}).unwrap();
    let Outcome::Denied(d) = o else {
        panic!("{o:?}")
    };
    assert!(
        d.reason.contains("changed since its sha256"),
        "{}",
        d.reason
    );
}

#[test]
fn a_signal_goes_to_the_program_and_its_exit_code_comes_back() {
    let t = tackd(
        "#!/bin/sh\ntrap 'echo got-term; exit 42' TERM\necho ready\nwhile :; do sleep 0.05; done\n",
    );
    let (sr, sw) = pipe();
    let (r, w) = pipe();
    // Send SIGTERM (from kill, not the terminal) once the program is ready.
    let rfd = r.as_raw_fd();
    let dup = unsafe { OwnedFd::from_raw_fd(libc::dup(rfd)) };
    let sender = std::thread::spawn(move || {
        let mut f = std::fs::File::from(dup);
        let mut b = [0u8; 6];
        f.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"ready\n");
        let mut s = std::fs::File::from(sw);
        s.write_all(&[libc::SIGTERM as u8, 0]).unwrap();
        // Keep the read side: the rest goes to the test.
        f
    });
    let null = devnull();
    let s = UnixStream::connect(&t.sock).unwrap();
    let o = session(
        &s,
        &req(&t, &[], [true, true, false]),
        &[null.as_raw_fd(), w.as_raw_fd()],
        Some(sr.as_raw_fd()),
        &mut |_| {},
    )
    .unwrap();
    drop(w);
    let mut f = sender.join().unwrap();
    let mut rest = String::new();
    f.read_to_string(&mut rest).unwrap();
    drop(r);
    assert_eq!(o, Outcome::Exited(42));
    assert_eq!(rest, "got-term\n");
}

#[test]
fn death_by_signal_is_reported_exactly() {
    let t = tackd("#!/bin/sh\nkill -KILL $$\n");
    let (o, _) = run_piped(&t, &[], None, &mut |_| {}, pipe());
    assert_eq!(
        o,
        Outcome::Killed {
            signal: libc::SIGKILL,
            core: false
        }
    );
    wait_ended(&t);
    let recs = audit(&t);
    assert!(
        recs.last().unwrap().reason.contains("ended by SIGKILL"),
        "{recs:?}"
    );
    let t = tackd("#!/bin/sh\nkill -TERM $$\n");
    let (o, _) = run_piped(&t, &[], None, &mut |_| {}, pipe());
    assert!(
        matches!(o, Outcome::Killed { signal, .. } if signal == libc::SIGTERM),
        "{o:?}"
    );
}

#[test]
fn a_stop_reaches_the_stand_in_and_a_continue_reaches_the_program() {
    let t = tackd("#!/bin/sh\necho before\nkill -STOP $$\necho after\nexit 3\n");
    let (sr, sw) = pipe();
    let mut sw = std::fs::File::from(sw);
    let mut stops = Vec::new();
    let (o, out) = run_piped(
        &t,
        &[],
        Some(sr.as_raw_fd()),
        &mut |sig| {
            stops.push(sig);
            // What the shell's fg does: SIGCONT to the stand-in.
            sw.write_all(&[libc::SIGCONT as u8, 0]).unwrap();
        },
        pipe(),
    );
    assert_eq!(o, Outcome::Exited(3));
    assert_eq!(stops, [libc::SIGSTOP]);
    assert_eq!(out, "before\nafter\n");
}

#[test]
fn the_program_reads_and_writes_the_callers_terminal() {
    let t = tackd(
        "#!/bin/sh\n[ -t 0 ] && [ -t 1 ] && echo is-a-tty\nstty size\ntrap 'stty size; exit 5' WINCH\nread line\necho \"got:$line\"\nwhile :; do sleep 0.05; done\n",
    );
    let mut master = 0;
    let mut slave = 0;
    let mut ws = libc::winsize {
        ws_row: 33,
        ws_col: 77,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let r = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &ws,
        )
    };
    assert_eq!(r, 0);
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    let (sr, sw) = pipe();
    let mfd = master.as_raw_fd();
    let reader = std::thread::spawn(move || {
        let mut m = std::fs::File::from(master);
        let mut seen = String::new();
        let mut step = 0;
        let mut sw = std::fs::File::from(sw);
        let mut buf = [0u8; 256];
        while let Ok(n) = m.read(&mut buf) {
            if n == 0 {
                break;
            }
            seen.push_str(&String::from_utf8_lossy(&buf[..n]));
            if step == 0 && seen.contains("33 77") {
                m.write_all(b"hello\n").unwrap();
                step = 1;
            }
            if step == 1 && seen.contains("got:hello") {
                // A new size, and the SIGWINCH that the terminal sends.
                ws.ws_row = 40;
                ws.ws_col = 100;
                unsafe { libc::ioctl(mfd, libc::TIOCSWINSZ, &ws) };
                sw.write_all(&[libc::SIGWINCH as u8, 1]).unwrap();
                step = 2;
            }
            if seen.contains("40 100") {
                break;
            }
        }
        seen
    });
    let s = UnixStream::connect(&t.sock).unwrap();
    let f = slave.as_raw_fd();
    let o = session(
        &s,
        &req(&t, &[], [true, true, true]),
        &[f, f, f],
        Some(sr.as_raw_fd()),
        &mut |_| {},
    )
    .unwrap();
    drop(slave);
    let seen = reader.join().unwrap();
    assert_eq!(o, Outcome::Exited(5), "{o:?} {seen}");
    assert!(seen.contains("is-a-tty"), "{seen}");
    assert!(seen.contains("got:hello"), "{seen}");
    assert!(seen.contains("40 100"), "{seen}");
}

#[test]
fn a_denial_starts_nothing_and_says_why() {
    let t = tackd_with(Some("#!/bin/sh\ntouch ran\n"), None, false);
    let s = UnixStream::connect(&t.sock).unwrap();
    let o = session(&s, &req(&t, &[], [false; 3]), &[], None, &mut |_| {}).unwrap();
    let Outcome::Denied(d) = &o else {
        panic!("{o:?}")
    };
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("not on the allowlist"), "{}", d.reason);
    assert!(!t.t.path().join("ran").exists());
    let m = message("tool", &o).unwrap();
    assert!(m.starts_with("tack: tool was not started: "), "{m}");
    // The exit code of a denial is its own.
    assert!(![0, 1, 2, 126, 127, UNAVAILABLE, LOST].contains(&DENIED));
}

#[test]
fn streams_that_do_not_match_the_request_start_nothing() {
    let t = tackd("#!/bin/sh\ntouch ran\n");
    let s = UnixStream::connect(&t.sock).unwrap();
    let null = devnull();
    // The request names three streams, one comes.
    let o = session(
        &s,
        &req(&t, &[], [true, true, true]),
        &[null.as_raw_fd()],
        None,
        &mut |_| {},
    )
    .unwrap();
    let Outcome::Failed(m) = &o else {
        panic!("{o:?}")
    };
    assert!(m.contains("3 standard streams and 1 came"), "{m}");
    assert!(!t.t.path().join("ran").exists());
    assert!(audit(&t).iter().any(|r| r.decision == "failed"));
}

#[test]
fn a_stand_in_that_dies_takes_the_program_with_it() {
    let t = tackd("#!/bin/sh\necho $$ > pid\nwhile :; do sleep 0.05; done\n");
    let s = UnixStream::connect(&t.sock).unwrap();
    let r = req(&t, &[], [false; 3]);
    let bytes = frame(&Request::Start(r)).unwrap();
    fdpass::send(&s, &bytes, &[]).unwrap();
    let a = read_answer(&s).unwrap();
    let Answer::Started {
        pid, protection, ..
    } = a
    else {
        panic!("{a:?}")
    };
    assert_eq!(protection, "unprotected (phase 4)");
    // Wait for the program to run, then go away as a SIGKILL would.
    let pidfile = t.t.path().join("pid");
    for _ in 0..200 {
        if pidfile.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(s);
    for _ in 0..300 {
        if audit(&t).iter().any(|r| r.decision == "ended") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let recs = audit(&t);
    let ended = recs.iter().find(|r| r.decision == "ended").expect("ended");
    assert!(
        ended
            .reason
            .contains(&format!("pid {pid} ended by SIGKILL")),
        "{ended:?}"
    );
}

#[test]
fn a_session_mark_says_a_note_once() {
    let t = tempfile::tempdir().unwrap();
    let key = session_key();
    assert!(key.contains('-'));
    once(t.path(), "rootless", "first");
    once(t.path(), "rootless", "second");
    let marks: Vec<_> = std::fs::read_dir(t.path()).unwrap().flatten().collect();
    assert_eq!(marks.len(), 1);
    assert_eq!(
        marks[0].file_name().to_string_lossy(),
        format!("rootless-{key}")
    );
    // A mark of a session that is gone is cleaned up at the next note.
    std::fs::write(t.path().join("rootless-999999999-1"), "").unwrap();
    once(t.path(), "sandbox", "third");
    assert!(!t.t_path_exists("rootless-999999999-1"));
}

trait Exists {
    fn t_path_exists(&self, n: &str) -> bool;
}

impl Exists for tempfile::TempDir {
    fn t_path_exists(&self, n: &str) -> bool {
        self.path().join(n).exists()
    }
}

#[test]
fn start_time_reads_past_a_name_with_spaces() {
    let stat =
        "1234 (my (odd) prog) S 1 1234 1234 0 -1 4194560 100 0 0 0 0 0 0 0 20 0 1 0 98765 1000 100";
    assert_eq!(start_time(stat), Some(98765));
    assert_eq!(start_time("garbage"), None);
}

fn env_with(home: &Path, path: &str, sandbox: bool) -> crate::env::HostEnv {
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("PATH".to_string(), path.to_string());
    if sandbox {
        vars.insert("PADDOCK".to_string(), "1".to_string());
    }
    crate::env::HostEnv::from_parts(home.to_path_buf(), "u".into(), me(), me(), vars)
}

#[test]
fn without_tackd_the_program_runs_with_references_and_a_note() {
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = env_with(t.path(), &bin.display().to_string(), false);
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        config::file(&env),
        format!(
            "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\n",
            gh.display()
        ),
    )
    .unwrap();
    let paths = crate::tack::service::Paths::under(&t.path().join("root"));
    let cmd = vec!["gh".to_string(), "pr".to_string()];
    match route(&env, true, None, &cmd, &paths) {
        Route::Direct {
            program,
            note: Some((kind, msg)),
        } => {
            assert_eq!(program, gh);
            assert_eq!(kind, "rootless");
            assert!(
                msg.contains("tackd is not installed") && msg.contains("GH_TOKEN"),
                "{msg}"
            );
        }
        r => panic!("{r:?}"),
    }
    // In a sandbox: the references, with the sandbox note.
    let senv = env_with(t.path(), &bin.display().to_string(), true);
    match route(&senv, true, Some("GH_TOKEN,NPM_TOKEN"), &cmd, &paths) {
        Route::Direct {
            note: Some((kind, msg)),
            ..
        } => {
            assert_eq!(kind, "sandbox");
            assert!(msg.contains("GH_TOKEN, NPM_TOKEN"), "{msg}");
        }
        r => panic!("{r:?}"),
    }
    // Installed (its unit file is there) but not running: no silent
    // fallback.
    std::fs::create_dir_all(paths.unit.parent().unwrap()).unwrap();
    std::fs::write(&paths.unit, crate::tack::service::unit_text(&paths)).unwrap();
    match route(&env, true, None, &cmd, &paths) {
        Route::Unavailable(m) => assert!(m.contains("not running"), "{m}"),
        r => panic!("{r:?}"),
    }
    // A program that no secret names runs as it is, quietly from a shim.
    let other = bin.join("other");
    std::fs::write(&other, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
    match route(&env, true, None, &["other".to_string()], &paths) {
        Route::Direct { note: None, .. } => {}
        r => panic!("{r:?}"),
    }
    assert!(matches!(
        route(&env, true, None, &["missing".to_string()], &paths),
        Route::NotFound(_)
    ));
}

#[test]
fn a_hostile_start_request_is_refused_before_anything_starts() {
    let t = tackd("#!/bin/sh\ntouch ran\n");
    for (mutate, want) in [
        (
            Box::new(|r: &mut Start| r.env.push("NOEQUALS".into())) as Box<dyn Fn(&mut Start)>,
            "no '='",
        ),
        (Box::new(|r: &mut Start| r.argv0.clear()), "argv0"),
        (
            Box::new(|r: &mut Start| r.caller_pid += 1),
            "kernel reports",
        ),
    ] {
        let mut r = req(&t, &[], [false; 3]);
        mutate(&mut r);
        let s = UnixStream::connect(&t.sock).unwrap();
        let got = session(&s, &r, &[], None, &mut |_| {});
        let text = match got {
            Err(e) => e.to_string(),
            Ok(Outcome::Denied(d)) => d.reason,
            Ok(o) => panic!("{o:?}"),
        };
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(!t.t.path().join("ran").exists());
}
