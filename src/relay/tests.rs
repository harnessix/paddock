use super::*;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

fn anc(pid: u32, comm: &str) -> Ancestor {
    Ancestor {
        pid,
        comm: comm.to_string(),
    }
}

#[test]
fn a_window_pid_in_the_chain_is_visible() {
    let chain = [anc(10, "sh"), anc(20, "foot"), anc(30, "Hyprland")];
    assert!(terminal_watched(Some(&[20, 99]), None, &chain));
}

#[test]
fn a_chain_without_a_window_is_not_visible() {
    let chain = [anc(10, "codex"), anc(11, "chatgpt")];
    assert!(!terminal_watched(Some(&[50]), None, &chain));
}

#[test]
fn an_empty_window_list_hides_a_plain_chain() {
    let chain = [anc(10, "codex"), anc(11, "chatgpt")];
    assert!(!terminal_watched(Some(&[]), None, &chain));
}

#[test]
fn without_a_window_list_any_terminal_is_watched() {
    let chain = [anc(10, "codex")];
    assert!(terminal_watched(None, None, &chain));
}

#[test]
fn a_background_server_under_an_app_window_is_not_visible() {
    // The ChatGPT desktop app (pid 30, a window) runs the Codex
    // app-server (pid 20), which holds the pty of paddock (pid 10).
    let chain = [anc(10, "paddock"), anc(20, "codex"), anc(30, "ChatGPT")];
    let server = anc(20, "codex");
    assert!(!terminal_watched(Some(&[30]), Some(&server), &chain));
    // A terminal emulator holds the pty and owns its window.
    let alacritty = anc(40, "alacritty");
    assert!(terminal_watched(Some(&[40]), Some(&alacritty), &chain));
    // A tmux server holds the pty of its panes.
    let tmux = anc(50, "tmux: server");
    assert!(terminal_watched(Some(&[]), Some(&tmux), &chain));
}

#[cfg(target_os = "linux")] // the pty index and /proc are Linux only
#[test]
fn the_holder_of_a_real_pty_is_found() {
    let (master, slave) = super::open_pty().unwrap();
    let name = std::fs::read_link(format!("/proc/self/fd/{slave}")).unwrap();
    let index: u32 = name
        .to_str()
        .unwrap()
        .strip_prefix("/dev/pts/")
        .unwrap()
        .parse()
        .unwrap();
    let holder = super::pty_master_holder(index).expect("holder");
    assert_eq!(holder.pid, std::process::id());
    // SAFETY: closing the two descriptors this test opened.
    unsafe { (libc::close(master), libc::close(slave)) };
}

#[test]
fn a_tmux_server_in_the_chain_is_visible() {
    let chain = [anc(10, "sh"), anc(11, "tmux: server")];
    assert!(terminal_watched(Some(&[]), None, &chain));
}

#[test]
fn screen_in_the_chain_is_visible() {
    let chain = [anc(10, "bash"), anc(12, "screen")];
    assert!(terminal_watched(Some(&[99]), None, &chain));
}

#[test]
fn the_parent_chain_starts_at_this_process() {
    let chain = parent_chain(std::process::id());
    assert_eq!(chain.first().map(|a| a.pid), Some(std::process::id()));
}

fn caller_pty() -> (RawFd, RawFd) {
    let mut master = 0;
    let mut slave = 0;
    // SAFETY: openpty writes two new fds for the test caller.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", io::Error::last_os_error());
    (master, slave)
}

fn set_size(fd: RawFd, rows: u16, cols: u16) {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ on the test pty.
    let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) };
    assert_eq!(rc, 0, "winsize: {}", io::Error::last_os_error());
}

fn read_until(fd: RawFd, needle: &[u8], timeout: Duration) -> Vec<u8> {
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    let start = Instant::now();
    while Instant::now().duration_since(start) < timeout {
        if got.windows(needle.len()).any(|w| w == needle) {
            break;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll on the test caller pty.
        let rc = unsafe { libc::poll(&mut pfd, 1, 50) };
        if rc < 0 {
            continue;
        }
        if rc == 0 {
            continue;
        }
        match read_fd(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }
    }
    got
}

#[test]
fn the_relay_copies_bytes_size_exit_code_and_the_log() {
    let _only = super::winch::TEST_SIGNAL
        .write()
        .unwrap_or_else(|e| e.into_inner());
    let (caller_m, caller_s) = caller_pty();
    set_size(caller_m, 24, 80);
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("view.log");
    let log = Arc::new(std::sync::Mutex::new(
        std::fs::File::create(&log_path).unwrap(),
    ));
    let log_thread = log.clone();
    let mut relay = Relay::start(
        caller_s,
        caller_s,
        move |data| {
            log_thread.lock().unwrap().write_all(data).unwrap();
        },
        None,
    )
    .unwrap();
    let script = "trap '' WINCH; stty -echo -onlcr; printf 'byte-exact\\n'; \
                  IFS= read -r line; printf 'got:%s\\n' \"$line\"; \
                  printf 'size:%s\\n' \"$(stty size)\"; exit 9";
    let (stdin, stdout, stderr) = relay.child_stdio().unwrap();
    let mut cmd = Command::new("sh");
    cmd.args(["-c", script])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    // SAFETY: take_controlling_tty uses setsid and ioctl, which are safe
    // in pre_exec. The slave is fd 0.
    unsafe {
        cmd.pre_exec(take_controlling_tty);
    }
    let mut child = cmd.spawn().unwrap();
    relay.close_slave();
    let head = read_until(caller_m, b"byte-exact\n", Duration::from_secs(5));
    assert!(
        head.windows(b"byte-exact\n".len())
            .any(|w| w == b"byte-exact\n"),
        "caller output: {}",
        String::from_utf8_lossy(&head)
    );
    set_size(caller_m, 30, 100);
    // SAFETY: raise SIGWINCH in this test process, which runs the relay.
    unsafe { libc::raise(libc::SIGWINCH) };
    assert!(write_all_fd(caller_m, b"hello\n").is_ok());
    let rest = read_until(caller_m, b"size:30 100\n", Duration::from_secs(5));
    let text = String::from_utf8_lossy(&rest);
    assert!(text.contains("got:hello\n"), "{text}");
    assert!(text.contains("size:30 100\n"), "{text}");
    let status = wait_child(&mut child, Duration::from_secs(5));
    assert_eq!(status.code(), Some(9));
    relay.finish();
    let mut logged = String::new();
    std::fs::File::open(&log_path)
        .unwrap()
        .read_to_string(&mut logged)
        .unwrap();
    assert!(logged.contains("byte-exact\n"), "{logged}");
    assert!(logged.contains("got:hello\n"), "{logged}");
    assert!(logged.contains("size:30 100\n"), "{logged}");
    // SAFETY: close the test caller pty.
    unsafe {
        libc::close(caller_m);
        libc::close(caller_s);
    }
}

fn wait_child(child: &mut std::process::Child, timeout: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now().duration_since(start) > timeout {
            let _ = child.kill();
            panic!("the harness did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The start screen watcher (screens.rs) on a live relay: a shell prints
/// the Codex hooks review screen and waits for one key.
#[test]
fn the_screen_watcher_answers_through_the_relay() {
    let _relay = super::winch::TEST_SIGNAL
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let (caller_m, caller_s) = caller_pty();
    set_size(caller_m, 24, 100);
    let profile: crate::profile::Profile =
        toml::from_str(include_str!("../../profiles/codex.toml")).unwrap();
    let rules = crate::screens::valid("codex", &profile.screens.rule);
    let w = crate::screens::live::Watcher::new(rules, || false, |_| {});
    let logged = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let l = logged.clone();
    let mut relay = Relay::start(
        caller_s,
        caller_s,
        move |data| l.lock().unwrap().extend_from_slice(data),
        Taps::of(None, Some(w.clone()), None),
    )
    .unwrap();
    let script = "printf '  Hooks need review\\n  1 hook is new or changed.\\n  Hooks can run outside the sandbox after you trust them.\\n› 1. Review hooks\\n  2. Trust all and continue\\n  3. Continue without trusting (hooks won'\\''t run)\\n  enter confirm · esc skip\\n'; \
                  stty -icanon -echo min 1; c=$(dd bs=1 count=7 2>/dev/null | od -An -tx1 | tr -d ' \n'); \
                  printf '\\033[2J\\033[Hgot:%s\\n' \"$c\"; sleep 1; exit 0";
    let (stdin, stdout, stderr) = relay.child_stdio().unwrap();
    let mut cmd = Command::new("sh");
    cmd.args(["-c", script])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    // SAFETY: take_controlling_tty uses setsid and ioctl, which are safe
    // in pre_exec. The slave is fd 0.
    unsafe {
        cmd.pre_exec(take_controlling_tty);
    }
    let mut child = cmd.spawn().unwrap();
    relay.close_slave();
    let out = read_until(caller_m, b"got:1b5b421b5b420a", Duration::from_secs(10));
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("got:1b5b421b5b420a"), "{text}");
    let status = wait_child(&mut child, Duration::from_secs(10));
    assert_eq!(status.code(), Some(0));
    relay.finish();
    let r = w.reports();
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].ok, "{r:?}");
    assert_eq!(
        r[0].text,
        "answered Codex hooks review: continued without trusting 1 hook"
    );
    let log = String::from_utf8_lossy(&logged.lock().unwrap()).into_owned();
    assert!(
        log.contains("[paddock: answered Codex hooks review: continued without trusting 1 hook]"),
        "{log}"
    );
    // SAFETY: close the test caller pty.
    unsafe {
        libc::close(caller_m);
        libc::close(caller_s);
    }
}

#[test]
fn folder_trust_fake_terminals_answer_only_the_project() {
    let _relay = super::winch::TEST_SIGNAL
        .read()
        .unwrap_or_else(|e| e.into_inner());
    for (name, outside) in [
        ("agy", false),
        ("codex", false),
        ("agy", true),
        ("codex", true),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let env = crate::privfs::tests::env_in(tmp.path());
        let profile = crate::profile::builtin(name).unwrap();
        let shown = if outside { &other } else { &project };
        let screen = if name == "agy" {
            format!("Accessing workspace: {}\nDo you trust the contents of this project?\nAntigravity CLI requires permission to read, edit, and execute files here.\n> Yes, I trust this folder\nNo, exit\n↑/↓ Navigate · enter Confirm", shown.display())
        } else {
            format!("Folder access\n{}\nTrust this folder? Codex can read, edit, and run files here, subject to your permission settings. Folder settings can run code automatically, even without a model request. Continue only if you trust these files. Your trust decision will be saved.\n› 1. Trust and continue\n2. Back to Agent Command Center\nenter continue · esc back", shown.display())
        };
        let screen_file = tmp.path().join("screen");
        std::fs::write(&screen_file, format!("{screen}\n")).unwrap();
        let spec = crate::spec::SandboxSpec::new(
            "fake-screen-123abc".into(),
            name,
            &project,
            tmp.path().into(),
            tmp.path(),
            env.uid,
            env.gid,
            "test",
        );
        crate::mail::create(&env, &spec.id, &crate::mail::Starter::host()).unwrap();
        let w = crate::screens::prepare(&env, &profile, &spec, true, true, false).unwrap();
        let (caller_m, caller_s) = caller_pty();
        set_size(caller_m, 30, 100);
        let mut relay = Relay::start(
            caller_s,
            caller_s,
            |_| {},
            Taps::of(None, Some(w.clone()), None),
        )
        .unwrap();
        let (stdin, stdout, stderr) = relay.child_stdio().unwrap();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", r#"stty -icanon -echo min 1; cat "$1"; c=$(perl -e 'my $v = ""; vec($v, 0, 1) = 1; if (select($v, undef, undef, 3) > 0) { sysread(STDIN, my $b, 1); print unpack("H*", $b); }'); printf '\033[2J\033[Hgot:%s\n' "$c"; sleep 1"#, "fake"])
            .arg(&screen_file).stdin(stdin).stdout(stdout).stderr(stderr);
        // SAFETY: the child creates a session and takes its slave tty.
        unsafe {
            cmd.pre_exec(take_controlling_tty);
        }
        let mut child = cmd.spawn().unwrap();
        relay.close_slave();
        let out = read_until(
            caller_m,
            if outside { b"got:\r\n" } else { b"got:0a" },
            Duration::from_secs(10),
        );
        let status = wait_child(&mut child, Duration::from_secs(10));
        assert!(status.success(), "{name}");
        relay.finish();
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains(if outside { "got:\r\n" } else { "got:0a" }),
            "{name}: {text}; reports: {:?}",
            w.reports()
        );
        if outside {
            assert!(w.reports().iter().any(|r| r.waiting), "{name}");
            let messages = crate::mail::read(&env, "fake-screen-123abc", crate::mail::Dir::Up);
            assert!(
                messages
                    .iter()
                    .any(|m| m.event.as_deref() == Some("trust-screen")),
                "{name}: {messages:?}"
            );
        }
        // SAFETY: these are the two caller descriptors opened above.
        unsafe {
            libc::close(caller_m);
            libc::close(caller_s);
        }
    }
}

/// A detached run on a desktop. The pane's pty belongs to paddock's tmux
/// server, so the terminal looks watched. The run still gets every start
/// screen rule and the screen tap. Codex asks the terminal for the cursor
/// position first, and tmux answers on the input: that answer is not a
/// user key, so the watcher still answers the screen after a slow start.
#[test]
fn a_detached_run_in_a_tmux_pane_still_answers_the_start_screen() {
    let _relay = super::winch::TEST_SIGNAL
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let server = anc(4242, "tmux: server");
    let chain = [anc(4300, "paddock"), server.clone()];
    let watched = || terminal_watched(Some(&[100]), Some(&server), &chain);
    let p = plan(true, watched, true);
    assert!(p.watched && !p.relay && p.unwatched, "{p:?}");
    // The same terminal in a run that is not detached: only `always` rules.
    assert!(!plan(true, watched, false).unwatched);
    // An invisible terminal is relayed and unwatched.
    let hidden = plan(true, || false, false);
    assert!(hidden.relay && hidden.unwatched);
    assert!(!plan(false, || panic!("no terminal to look at"), true).relay);

    let profile: crate::profile::Profile =
        toml::from_str(include_str!("../../profiles/codex.toml")).unwrap();
    let rules = crate::screens::rules_for(&profile, p.unwatched);
    assert!(rules.iter().any(|r| r.id == "codex-hooks-review-many"));
    assert!(crate::screens::rules_for(&profile, false)
        .iter()
        .all(|r| r.kind == crate::screens::RuleKind::FolderTrust));
    let (caller_m, caller_s) = caller_pty();
    set_size(caller_m, 24, 100);
    let w = crate::screens::live::Watcher::new(rules, || false, |_| {});
    let mut relay = Relay::start(
        caller_s,
        caller_s,
        |_| {},
        Taps::of(None, Some(w.clone()), None),
    )
    .unwrap();
    let script = "stty -icanon -echo min 1; printf '\\033[6n'; r=$(dd bs=1 count=6 2>/dev/null); \
                  printf 'starting MCP servers\\n'; sleep 1; \
                  printf '\\033[2J\\033[H  Hooks need review\\n  12 hooks are new or changed.\\n  Hooks can run outside the sandbox after you trust them.\\n› 1. Review hooks\\n  2. Trust all and continue\\n  3. Continue without trusting (hooks won'\\''t run)\\n  enter confirm · esc skip\\n'; \
                  c=$(dd bs=1 count=7 2>/dev/null | od -An -tx1 | tr -d ' \n'); \
                  printf '\\033[2J\\033[Hgot:%s\\n' \"$c\"; sleep 1; exit 0";
    let (stdin, stdout, stderr) = relay.child_stdio().unwrap();
    let mut cmd = Command::new("sh");
    cmd.args(["-c", script])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    // SAFETY: take_controlling_tty uses setsid and ioctl, which are safe
    // in pre_exec. The slave is fd 0.
    unsafe {
        cmd.pre_exec(take_controlling_tty);
    }
    let mut child = cmd.spawn().unwrap();
    relay.close_slave();
    // tmux answers the query on the input of the pane.
    let q = read_until(caller_m, b"\x1b[6n", Duration::from_secs(10));
    assert!(q.windows(4).any(|x| x == b"\x1b[6n"));
    assert!(write_all_fd(caller_m, b"\x1b[7;1R").is_ok());
    let out = read_until(caller_m, b"got:1b5b421b5b420a", Duration::from_secs(15));
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("got:1b5b421b5b420a"), "{text}");
    let status = wait_child(&mut child, Duration::from_secs(10));
    assert_eq!(status.code(), Some(0));
    relay.finish();
    let r = w.reports();
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(
        r[0].text,
        "answered Codex hooks review: continued without trusting 12 hooks"
    );
    // SAFETY: close the test caller pty.
    unsafe {
        libc::close(caller_m);
        libc::close(caller_s);
    }
}
